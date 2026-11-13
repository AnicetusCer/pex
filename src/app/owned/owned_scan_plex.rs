use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use tracing::warn;

use crate::app::cache;
use crate::app::types::{OwnedDone, OwnedMsg};
use crate::app::PexApp;
use crate::config::local_library_db_path;

pub struct OwnedScanPlex;

impl OwnedScanPlex {
    pub(crate) fn spawn_scan(tx: Sender<OwnedMsg>, force_copy: bool) {
        thread::spawn(move || {
            use OwnedMsg::{Done, Error, Info};

            let _ = tx.send(Info(
                "Stage 3/4 - Loading owned titles from Plex library database.".into(),
            ));

            let db_path = local_library_db_path();
            // Refresh and read the same mirror on one worker. The UI remains
            // usable during a slow NAS copy, including an explicit refresh.
            match crate::app::prep::sync_library_db_from_source(force_copy, |s| {
                let _ = tx.send(Info(s));
            }) {
                Ok(message) => {
                    let _ = tx.send(OwnedMsg::Snapshot(message));
                }
                Err(err) => {
                    let _ = tx.send(Error(err));
                    return;
                }
            }
            let _ = tx.send(OwnedMsg::Scheduled(
                crate::app::scheduled::load_scheduled_index(),
            ));
            if !db_path.exists()
                && crate::config::load_config()
                    .plex_library_db_source
                    .is_none()
            {
                let _ = tx.send(Done(Box::default()));
                return;
            }
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
                    let index = index_owned_entries(entries);
                    let cache_dir = cache::cache_dir();
                    if let Err(err) =
                        persist_owned_sidecar(&cache_dir.join(super::ALL_SIDECAR), &index.keys)
                    {
                        warn!("Failed to persist owned sidecar: {err}");
                    }
                    if let Err(err) =
                        persist_owned_sidecar(&cache_dir.join(super::HD_SIDECAR), &index.hd_keys)
                    {
                        warn!("Failed to persist owned HD sidecar: {err}");
                    }

                    let count = index.keys.len();
                    let _ = tx.send(Info(format!(
                        "Stage 3/4 - Plex library owned scan complete ({count} keys)."
                    )));
                    let _ = tx.send(Done(Box::new(index)));
                }
                Err(err) => {
                    let _ = tx.send(Error(err));
                }
            }
        });
    }
}

#[allow(dead_code)]
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
            COALESCE(mp.size, 0) DESC
    "#;

    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| format!("Failed to prepare Plex library query: {err}"))?;

    let mut seen_ids: HashSet<i64> = HashSet::new();

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

        if !seen_ids.insert(metadata_id) {
            continue;
        }

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

        results.push(PlexOwnedEntry {
            metadata_id,
            guid,
            title,
            original_title,
            year,
            width,
            height,
            updated_at,
            file_path,
        });
    }

    Ok(results)
}

fn index_owned_entries(entries: Vec<PlexOwnedEntry>) -> OwnedDone {
    let mut index = OwnedDone::default();
    let mut ambiguous = HashSet::new();
    for entry in entries {
        let hd = is_hd(entry.width, entry.height);
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
        let mut keys = HashSet::new();
        for title in titles
            .into_iter()
            .flatten()
            .chain(stem)
            .chain(path.file_name().and_then(|s| s.to_str()))
        {
            keys.extend(PexApp::library_title_keys(title, year));
        }
        // Only metadata titles may stand in for an omitted subtitle. If different
        // films claim the same abbreviation, do not use it to establish ownership.
        if let Some(year) = year {
            let mut abbreviations = HashSet::new();
            for title in titles
                .into_iter()
                .flatten()
                .filter(|t| !t.trim().is_empty())
            {
                let (kind, name) = match title.split_once(':') {
                    Some((head, _)) => ("prefix", head),
                    None => ("bare", title),
                };
                if !crate::app::utils::normalize_title(name).is_empty() {
                    abbreviations.insert(PexApp::short_title_key(kind, name, year));
                }
                if let Some(remainder) = PexApp::credited_title_remainder(title) {
                    abbreviations.insert(PexApp::short_title_key("credited", remainder, year));
                }
            }
            for key in abbreviations {
                if index.keys.contains(&key) {
                    ambiguous.insert(key.clone());
                }
                keys.insert(key);
            }
        }
        for key in keys {
            if hd {
                index.hd_keys.insert(key.clone());
            }
            index.modified.insert(key.clone(), entry.updated_at);
            index.keys.insert(key);
        }
        if let Some(guid) = entry
            .guid
            .as_deref()
            .and_then(crate::app::utils::canonicalize_guid)
        {
            index.guid_modified.insert(guid.clone(), entry.updated_at);
            index.guids.insert(guid.clone());
            if hd {
                index.hd_guids.insert(guid);
            }
        }
    }
    for key in ambiguous {
        index.keys.remove(&key);
        index.hd_keys.remove(&key);
        index.modified.remove(&key);
    }
    index
}

fn is_hd(width: Option<u32>, height: Option<u32>) -> bool {
    width.map(|w| w >= 1280).unwrap_or(false) || height.map(|h| h >= 720).unwrap_or(false)
}

fn persist_owned_sidecar(path: &Path, owned_keys: &HashSet<String>) -> Result<(), String> {
    let mut keys: Vec<&String> = owned_keys.iter().collect();
    keys.sort_unstable();
    fs::write(
        path,
        keys.iter().map(|k| format!("{k}\n")).collect::<String>(),
    )
    .map_err(|err| format!("Failed to write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(films: &[(&str, Option<i32>)]) -> OwnedDone {
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

    fn matches(index: &OwnedDone, title: &str, year: Option<i32>) -> bool {
        PexApp::match_owned_title(title, year, &index.keys).is_some()
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
