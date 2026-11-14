use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use tracing::warn;

use crate::app::cache;
use crate::app::types::{
    OwnedCandidate, OwnedCandidates, OwnedField, OwnedLibrary, OwnedMovie, OwnedMsg, OwnedQuality,
};
use crate::app::PexApp;
use crate::config::local_library_db_path;

pub struct OwnedScanPlex;

impl OwnedScanPlex {
    pub(super) fn spawn_scan(
        tx: Sender<OwnedMsg>,
        force_copy: bool,
        source: super::LibrarySource,
        configured_databases: Vec<std::path::PathBuf>,
    ) {
        thread::spawn(move || {
            use OwnedMsg::{Done, Error, Info};

            let _ = tx.send(Info(
                "Stage 3/4 - Loading owned titles from Plex library database.".into(),
            ));

            let db_path = local_library_db_path();
            // Refresh and read the same mirror on one worker. The UI remains
            // usable during a slow NAS copy, including an explicit refresh.
            let snapshot = match crate::app::prep::sync_library_db_from_source(
                &source,
                &configured_databases,
                force_copy,
                |s| {
                    let _ = tx.send(Info(s));
                },
            ) {
                Ok(message) => {
                    let _ = tx.send(OwnedMsg::Snapshot(message.clone()));
                    message
                }
                Err(err) => {
                    let _ = tx.send(Error(err));
                    return;
                }
            };
            let _ = tx.send(OwnedMsg::Scheduled(
                crate::app::scheduled::load_scheduled_index(),
            ));
            let conn = match Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            ) {
                Ok(conn) => conn,
                Err(err) => {
                    let _ = tx.send(Error(format!(
                        "Opening Plex library {} failed: {err}",
                        db_path.display()
                    )));
                    return;
                }
            };
            let _ = conn.busy_timeout(Duration::from_secs(5));

            match collect_plex_owned_entries(&conn) {
                Ok(entries) => {
                    let mut index = index_owned_entries(entries);
                    index.snapshot = Some(snapshot);
                    if let Err(e) = super::title_cache::save(&cache::cache_dir(), &source, &index) {
                        warn!("Could not save the owned title cache: {e}");
                    }

                    let _ = tx.send(Done(Box::new(index)));
                }
                Err(err) => {
                    let _ = tx.send(Error(err));
                }
            }
        });
    }
}

#[derive(Debug, Clone)]
struct PlexOwnedEntry {
    metadata_id: i64,
    guid: Option<String>,
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    updated_at: Option<u64>,
    file_path: String,
}

fn collect_plex_owned_entries(conn: &Connection) -> Result<Vec<PlexOwnedEntry>, String> {
    let sql = r#"
        SELECT
            m.id            AS metadata_id,
            m.guid          AS guid,
            m.title         AS title,
            m.original_title AS original_title,
            m.year          AS year,
            m.updated_at    AS meta_updated_at,
            m.added_at      AS meta_added_at,
            mi.id           AS media_item_id,
            mi.width        AS width,
            mi.height       AS height,
            mi.updated_at   AS media_updated_at,
            mp.id           AS media_part_id,
            mp.file         AS file_path,
            mp.size         AS file_size,
            mp.updated_at   AS part_updated_at
        FROM metadata_items m
        JOIN media_items mi ON mi.metadata_item_id = m.id
        JOIN media_parts mp ON mp.media_item_id = mi.id
        WHERE m.metadata_type = 1
          AND mp.file IS NOT NULL
          AND mp.file <> ''
        ORDER BY
            m.id ASC,
            COALESCE(mi.width, 0) DESC,
            COALESCE(mi.height, 0) DESC,
            COALESCE(mp.size, 0) DESC,
            mp.id ASC
    "#;

    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| format!("Failed to prepare Plex library query: {err}"))?;

    let rows = stmt
        .query_map([], |row| {
            let metadata_id: i64 = row.get("metadata_id")?;
            let guid: Option<String> = row.get("guid")?;
            let title: String = row.get("title")?;
            let original_title: Option<String> = row.get("original_title")?;
            let year: Option<i32> = row.get("year")?;
            let width: Option<i64> = row.get("width")?;
            let height: Option<i64> = row.get("height")?;
            let part_updated_at: Option<i64> = row.get("part_updated_at")?;
            let media_updated_at: Option<i64> = row.get("media_updated_at")?;
            let meta_updated_at: Option<i64> = row.get("meta_updated_at")?;
            let meta_added_at: Option<i64> = row.get("meta_added_at")?;
            let file_path: String = row.get("file_path")?;
            let _size: Option<i64> = row.get("file_size")?;

            Ok((
                metadata_id,
                guid,
                title,
                original_title,
                year,
                width,
                height,
                part_updated_at,
                media_updated_at,
                meta_updated_at,
                meta_added_at,
                file_path,
            ))
        })
        .map_err(|err| format!("Failed to iterate Plex library rows: {err}"))?;

    let mut results: Vec<PlexOwnedEntry> = Vec::new();

    for row in rows {
        let (
            metadata_id,
            guid,
            title,
            original_title,
            year,
            width,
            height,
            part_updated_at,
            media_updated_at,
            meta_updated_at,
            meta_added_at,
            file_path,
        ) = row.map_err(|err| format!("Failed to read Plex library row: {err}"))?;

        if title.trim().is_empty() {
            continue;
        }

        let width = width.map(|v| v.max(0) as u32);
        let height = height.map(|v| v.max(0) as u32);
        let updated_at = part_updated_at
            .or(media_updated_at)
            .or(meta_updated_at)
            .or(meta_added_at)
            .map(|ts| ts.max(0) as u64);

        let entry = PlexOwnedEntry {
            metadata_id,
            guid,
            title,
            original_title,
            year,
            width,
            height,
            updated_at,
            file_path,
        };
        // Copies of one movie are adjacent in SQL quality/size order. Retain
        // a representative path for the strongest evidence across all copies:
        // one HD file suffices, and any unmeasured copy prevents an SD-only claim.
        if let Some(previous) = results
            .last_mut()
            .filter(|prior| prior.metadata_id == metadata_id)
        {
            if file_quality(entry.width, entry.height)
                > file_quality(previous.width, previous.height)
            {
                *previous = entry;
            }
        } else {
            results.push(entry);
        }
    }

    Ok(results)
}

fn index_owned_entries(entries: Vec<PlexOwnedEntry>) -> OwnedLibrary {
    let mut index = OwnedLibrary::default();
    index.movies.reserve(entries.len());
    let mut ambiguous = HashSet::new();
    for (movie, entry) in entries.into_iter().enumerate() {
        let path = Path::new(&entry.file_path);
        let stem = path.file_stem().and_then(|s| s.to_str());
        let titles = [Some(entry.title.as_str()), entry.original_title.as_deref()];
        let year = entry.year.or_else(|| {
            titles
                .into_iter()
                .flatten()
                .chain(stem)
                .find_map(|s| PexApp::trailing_title_year(s).map(|(_, year)| year))
        });
        // Prefer metadata titles when a filename or alias also supplies the same key.
        let mut keys = HashMap::new();
        for (title, field) in [
            (Some(entry.title.as_str()), OwnedField::Title),
            (entry.original_title.as_deref(), OwnedField::OriginalTitle),
            (stem, OwnedField::Filename),
            (
                path.file_name().and_then(|s| s.to_str()),
                OwnedField::Filename,
            ),
        ] {
            if let Some(title) = title {
                for key in PexApp::library_title_keys(title, year) {
                    keys.entry(key).or_insert(field);
                }
            }
        }
        // Only metadata titles may stand in for an omitted subtitle. If different
        // films claim the same abbreviation, do not use it to establish ownership.
        if let Some(year) = year {
            for (title, field) in titles
                .into_iter()
                .zip([OwnedField::Title, OwnedField::OriginalTitle])
            {
                let Some(title) = title.filter(|t| !t.trim().is_empty()) else {
                    continue;
                };
                let (kind, name) = match title.split_once(':') {
                    Some((head, _)) => ("prefix", head),
                    None => ("bare", title),
                };
                let short = (!crate::app::utils::normalize_title(name).is_empty())
                    .then(|| PexApp::short_title_key(kind, name, year));
                let credit = PexApp::credited_title_remainder(title)
                    .map(|name| PexApp::short_title_key("credited", name, year));
                for key in short.into_iter().chain(credit) {
                    if index.by_title.contains_key(&key) {
                        ambiguous.insert(key.clone());
                    }
                    keys.entry(key).or_insert(field);
                }
            }
        }
        for (key, field) in keys {
            OwnedCandidates::add(&mut index.by_title, key, OwnedCandidate { movie, field });
        }
        if let Some(guid) = entry
            .guid
            .as_deref()
            .and_then(crate::app::utils::canonicalize_guid)
        {
            OwnedCandidates::add(
                &mut index.by_guid,
                guid,
                OwnedCandidate {
                    movie,
                    field: OwnedField::Guid,
                },
            );
        }
        index.movies.push(OwnedMovie {
            title: entry.title,
            year: entry.year,
            path: entry.file_path,
            quality: file_quality(entry.width, entry.height),
            updated_at: entry.updated_at,
        });
    }
    for key in ambiguous {
        index.by_title.remove(&key);
    }
    index
}

fn file_quality(width: Option<u32>, height: Option<u32>) -> OwnedQuality {
    if width.is_some_and(|w| w >= 1280) || height.is_some_and(|h| h >= 720) {
        return OwnedQuality::Hd;
    }
    if width.is_some_and(|w| w > 0) && height.is_some_and(|h| h > 0) {
        return OwnedQuality::Sd;
    }
    OwnedQuality::Unknown
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::app::owned) fn library(films: &[(&str, Option<i32>)]) -> OwnedLibrary {
        let entries = films
            .iter()
            .enumerate()
            .map(|(i, &(title, year))| PlexOwnedEntry {
                metadata_id: i as i64,
                guid: None,
                title: title.into(),
                original_title: None,
                year,
                width: Some(1920),
                height: Some(1080),
                updated_at: None,
                file_path: match year {
                    Some(year) => format!("library/{title} ({year}).mkv"),
                    None => format!("library/{title}.mkv"),
                },
            });
        index_owned_entries(entries.collect())
    }

    fn matches(index: &OwnedLibrary, title: &str, year: Option<i32>) -> bool {
        PexApp::match_owned_title(title, year, |key| index.by_title.contains_key(key)).is_some()
    }

    #[test]
    fn both_adjacent_years_are_equal_candidates_and_share_hd_evidence() {
        use super::super::{lookup, title_cache, LibrarySource, MatchMethod, Titles};
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../test_fixtures.sql"))
            .unwrap();
        db.execute_batch("
            UPDATE metadata_items SET title='The Return', year=2024;
            UPDATE media_items SET width=720, height=576;
            INSERT INTO metadata_items(id,title,year,metadata_type) VALUES(2,'The Return',2026,1);
            INSERT INTO media_items(id,metadata_item_id,width,height) VALUES(2,2,1920,1080);
            INSERT INTO media_parts(id,media_item_id,file) VALUES(2,2,'/media/The Return (2026).mkv');
        ").unwrap();
        let scan = index_owned_entries(collect_plex_owned_entries(&db).unwrap());
        let find = |y, titles| lookup("The Return", Some(y), None, titles).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let source = LibrarySource::Live(tmp.path().join("library.db"));
        title_cache::save(tmp.path(), &source, &scan).unwrap();
        let preload = title_cache::preload(tmp.path(), Some(&source)).unwrap();
        let matched = find(2025, Titles::Verified(&scan));
        assert!(matched.method == MatchMethod::AdjacentYear);
        assert_eq!(
            matched
                .candidates
                .iter()
                .map(|c| scan.movies[c.movie].year)
                .collect::<Vec<_>>(),
            [Some(2024), Some(2026)]
        );
        assert_eq!(matched.quality, OwnedQuality::Hd);
        assert_eq!(
            find(2025, Titles::Preloaded(&preload)).quality,
            matched.quality
        );
        let exact = find(2024, Titles::Verified(&scan));
        assert!(exact.method == MatchMethod::ExactYear);
        assert_eq!(exact.candidates.len(), 1);
        assert_eq!(exact.quality, OwnedQuality::Sd);
        assert_eq!(
            find(2024, Titles::Preloaded(&preload)).quality,
            exact.quality
        );
        db.execute(
            "UPDATE media_items SET height=NULL, width=720 WHERE id=2",
            [],
        )
        .unwrap();
        let mixed = index_owned_entries(collect_plex_owned_entries(&db).unwrap());
        title_cache::save(tmp.path(), &source, &mixed).unwrap();
        let preload = title_cache::preload(tmp.path(), Some(&source)).unwrap();
        for titles in [Titles::Verified(&mixed), Titles::Preloaded(&preload)] {
            assert_eq!(
                lookup("The Return", Some(2025), None, titles)
                    .unwrap()
                    .quality,
                OwnedQuality::Unknown
            );
        }
    }

    #[test]
    fn partial_dimensions_cannot_establish_an_sd_only_library_copy() {
        for dimensions in [
            (None, Some(540)),
            (Some(720), None),
            (Some(0), Some(576)),
            (None, None),
        ] {
            assert_eq!(
                file_quality(dimensions.0, dimensions.1),
                OwnedQuality::Unknown
            );
        }
        assert_eq!(file_quality(Some(1280), None), OwnedQuality::Hd);
        assert_eq!(file_quality(None, Some(720)), OwnedQuality::Hd);
        assert_eq!(file_quality(Some(720), Some(576)), OwnedQuality::Sd);
    }

    #[test]
    fn scan_counts_distinct_movies_and_keeps_the_file_and_field_behind_each_match() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../test_fixtures.sql"))
            .unwrap();
        conn.execute_batch(
            "
            UPDATE media_parts SET file='/media/Evening Film.mkv';
            UPDATE metadata_items SET original_title='Film du soir';
            INSERT INTO media_items(id,metadata_item_id,width,height) VALUES(2,1,720,576);
            INSERT INTO media_parts(id,media_item_id,file) VALUES(2,2,'/media/SD copy.mkv');
            INSERT INTO metadata_items(id,title,year,metadata_type,guid)
              VALUES(2,'Fixture Movie',1999,1,'imdb://tt9999999');
            INSERT INTO media_items(id,metadata_item_id,width,height) VALUES(3,2,1920,1080);
            INSERT INTO media_parts(id,media_item_id,file) VALUES(3,3,'/archive/older film.mkv');
        ",
        )
        .unwrap();
        let scan = index_owned_entries(collect_plex_owned_entries(&conn).unwrap());
        assert_eq!(scan.movies.len(), 2);
        assert!(scan.by_title.len() > 2);
        let find = |title, year, guid| {
            super::super::lookup(title, year, guid, super::super::Titles::Verified(&scan)).unwrap()
        };
        // GUID wins over the conflicting title/year. The chosen part is the HD copy.
        let by_id = find("Fixture Movie", Some(1999), Some("IMDB://tt1234567"));
        assert!(by_id.method == super::super::MatchMethod::Id);
        for (found, field) in [
            (by_id, OwnedField::Guid),
            (
                find("Film du soir", Some(2020), None),
                OwnedField::OriginalTitle,
            ),
            (find("Evening Film", Some(2021), None), OwnedField::Filename),
            (find("Fixture Movie", Some(2020), None), OwnedField::Title),
        ] {
            let [candidate] = found.candidates.as_ref() else {
                panic!("expected one movie")
            };
            assert_eq!(candidate.field, field);
            let movie = &scan.movies[candidate.movie];
            assert_eq!(movie.title, "Fixture Movie");
            assert_eq!(movie.year, Some(2020));
            assert_eq!(movie.path, "/media/Evening Film.mkv");
        }
        let ambiguous = find("Fixture Movie", None, None);
        assert_eq!(ambiguous.candidates.len(), 2);
        assert_ne!(ambiguous.candidates[0].movie, ambiguous.candidates[1].movie);
    }

    #[test]
    fn hunger_games_films_keep_their_identity() {
        let owned = library(&[
            ("The Hunger Games", Some(2012)),
            ("The Hunger Games: Catching Fire", Some(2013)),
            ("The Hunger Games: Mockingjay - Part 1", Some(2014)),
        ]);
        let prequel = "The Hunger Games: The Ballad of Songbirds & Snakes";
        assert!(!matches(&owned, prequel, Some(2023)));
        assert!(!matches(&owned, prequel, None));
        assert!(!matches(
            &owned,
            "The Hunger Games: Mockingjay, Part 2",
            Some(2015)
        ));
        assert!(matches(
            &owned,
            "The Hunger Games: Mockingjay, Part 1",
            Some(2014)
        ));
    }

    #[test]
    fn full_titles_allow_one_year_of_release_date_variation() {
        let owned = library(&[("Dune", Some(1984))]);
        for year in [1983, 1984, 1985] {
            assert!(matches(&owned, "Dune", Some(year)), "{year}");
        }
        for year in [1982, 1986, 2021] {
            assert!(!matches(&owned, "Dune", Some(year)), "{year}");
        }
        assert!(matches(&owned, "Dune", None));
        let undated = library(&[("Dune", None)]);
        assert!(matches(&undated, "Dune", Some(1984)));
    }

    #[test]
    fn possessive_titles_do_not_claim_a_generic_remainder() {
        let bean = "Mr. Bean's Holiday";
        let owned = library(&[(bean, Some(2007))]);
        for year in [Some(2006), Some(2007), None] {
            assert!(!matches(&owned, "The Holiday", year));
            assert!(!matches(&owned, "Holiday", year));
        }
        assert!(matches(&owned, "Mr Bean’s Holiday", Some(2007)));
        assert!(matches(
            &library(&[("The Holiday", Some(2006))]),
            "The Holiday",
            Some(2006)
        ));
    }

    #[test]
    fn omitted_possessive_credits_need_a_unique_multiword_title_and_exact_year() {
        let credited = "Lemony Snicket’s A Series of Unfortunate Events";
        let bare = "A Series of Unfortunate Events";
        for (owned, guide) in [(bare, credited), (credited, bare)] {
            let index = library(&[(owned, Some(2004))]);
            assert!(matches(&index, guide, Some(2004)));
            assert!(!matches(&index, guide, Some(2005)));
            assert!(!matches(&index, guide, None));
        }
        let other = "Another Writer's A Series of Unfortunate Events";
        let ambiguous = library(&[(credited, Some(2004)), (other, Some(2004))]);
        assert!(!matches(&ambiguous, bare, Some(2004)));
        assert!(!matches(
            &library(&[(credited, Some(2004))]),
            other,
            Some(2004)
        ));
    }

    #[test]
    fn an_omitted_subtitle_needs_the_same_known_year_on_both_sides() {
        let bare = "Biggles";
        let full = "Biggles: Adventures in Time";
        for (owned_title, airing_title) in [(bare, full), (full, bare)] {
            let owned = library(&[(owned_title, Some(1986))]);
            assert!(matches(&owned, airing_title, Some(1986)));
            assert!(!matches(&owned, airing_title, Some(1987)));
            assert!(!matches(&owned, airing_title, None));
            assert!(!matches(
                &library(&[(owned_title, None)]),
                airing_title,
                Some(1986)
            ));
        }
    }

    #[test]
    fn a_franchise_prefix_cannot_resolve_conflicting_or_ambiguous_subtitles() {
        let first = "Aurora Teagarden Mysteries: A Game of Cat and Mouse";
        let second = "Aurora Teagarden Mysteries: A Very Foul Play";
        let year = Some(2019);
        let one = library(&[(first, year)]);
        assert!(!matches(&one, second, year));
        let both = library(&[(first, year), (second, year)]);
        assert!(!matches(&both, "Aurora Teagarden Mysteries", year));
        assert!(matches(&both, first, year));
        assert!(matches(&both, second, year));
    }
}
