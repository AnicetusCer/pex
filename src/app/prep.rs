// src/app/prep.rs
mod snapshots;
mod source_guard;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io};
use tracing::{info, warn};

use rusqlite::{Connection, OpenFlags};

use crate::app::cache::url_to_cache_key;
use crate::app::{PrepItem, PrepMsg}; // <- use the re-export from app::types
use crate::config::{load_config, local_db_path, local_library_db_path};
use eframe::egui as eg; // <- gives us eg::Context

// ---- helpers only used in this module ----
fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_row| Ok(()),
    )
    .is_ok()
}

pub(super) fn table_columns(
    db: &Connection,
    table: &str,
) -> rusqlite::Result<std::collections::HashSet<String>> {
    db.prepare("SELECT name FROM pragma_table_info(?1)")?
        .query_map([table], |row| row.get(0))?
        .collect()
}

pub(super) fn optional_column(
    columns: &std::collections::HashSet<String>,
    alias: &str,
    name: &str,
) -> String {
    if columns.contains(name) {
        format!("{alias}.{name}")
    } else {
        "NULL".into()
    }
}

#[derive(Default, Clone, Debug)]
struct ChannelMeta {
    guide_channel: super::scheduled::GuideChannel,
    call_sign: Option<String>,
    title: Option<String>,
    thumb: Option<String>,
}

/// Extract channel metadata from `media_items.extra_data`
fn parse_channel_meta(extra: &[u8]) -> ChannelMeta {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(extra) else {
        return ChannelMeta::default();
    };
    let text = |key: &str| {
        value
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    ChannelMeta {
        call_sign: text("at:channelCallSign"),
        title: text("at:channelTitle"),
        thumb: text("at:channelThumb"),
        guide_channel: super::scheduled::GuideChannel::read(&value),
    }
}

fn movie_subscription_id(extra: &[u8]) -> Option<i64> {
    #[derive(serde::Deserialize)]
    struct Subscription {
        #[serde(rename = "at:subscriptionType")]
        kind: String,
        #[serde(rename = "at:subscriptionID")]
        id: serde_json::Value,
    }

    let subscription: Subscription = serde_json::from_slice(extra).ok()?;
    if !subscription.kind.eq_ignore_ascii_case("movie") {
        return None;
    }
    subscription
        .id
        .as_i64()
        .or_else(|| subscription.id.as_str()?.trim().parse().ok())
        .filter(|id| *id > 0)
}

/// A film can have many media_items (airings). Artwork is optional and belongs
/// to the metadata item, so neither its URL nor its absence identifies the film.
fn read_guide_airings(
    db: &Connection,
    progress: impl Fn(usize),
) -> rusqlite::Result<Vec<PrepItem>> {
    // Discover optional columns once. Old guides may omit both artwork and video dimensions.
    let metadata = table_columns(db, "metadata_items")?;
    let media = table_columns(db, "media_items")?;
    let preferred = optional_column(&metadata, "m", "user_thumb_url");
    let secondary = optional_column(&metadata, "m", "thumb_url");
    // An unanalysed EPG entry can repeat nominal dimensions for every programme
    // on a station. Only pass measured dimensions to the quality classifier.
    let analyzed_dimension = |name: &str| {
        if media.contains("media_analysis_version") && media.contains(name) {
            format!("CASE WHEN CAST(mi.media_analysis_version AS INTEGER) > 0 THEN mi.{name} END")
        } else {
            "NULL".to_owned()
        }
    };
    let width = analyzed_dimension("width");
    let height = analyzed_dimension("height");
    let resolution = optional_column(&media, "mi", "video_resolution");
    let extra_data = optional_column(&media, "mi", "extra_data");
    let metadata_extra = optional_column(&metadata, "m", "extra_data");
    let sql = format!(
        "SELECT m.id, m.title, mi.begins_at, m.year, m.guid, m.tags_genre,
        {extra_data}, m.summary, m.audience_rating, m.rating, {preferred}, {secondary},
        {width} AS airing_width, {height} AS airing_height, CAST({resolution} AS TEXT) AS airing_resolution,
        {metadata_extra} AS movie_metadata
        FROM metadata_items m JOIN media_items mi ON mi.metadata_item_id = m.id
        WHERE m.metadata_type = 1 AND mi.begins_at >= 0 AND trim(m.title) <> ''
        ORDER BY mi.begins_at, m.id, mi.id"
    );
    let mut statement = db.prepare(&sql)?;
    let mut rows = statement.query([])?;
    let mut list = Vec::new();
    let mut last_emit = Instant::now();
    while let Some(row) = rows.next()? {
        let metadata_id: i64 = row.get(0)?;
        let (Ok(title), Ok(begins_at)) = (row.get::<_, String>(1), row.get::<_, i64>(2)) else {
            continue;
        };
        let title = title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        let year: Option<i32> = row.get(3).ok();
        let guid: Option<String> = row.get(4).ok();
        let film_key = if let Some(id) = guid
            .as_deref()
            .and_then(crate::app::utils::canonicalize_guid)
        {
            format!("guid:{id}")
        } else if let Some(year) = year.filter(|year| *year > 0) {
            format!("title:{year}:{}", title.to_lowercase())
        } else {
            // Avoid merging two undated, same-title films.
            format!("metadata:{metadata_id}")
        };
        let artwork: [Option<String>; 2] = [row.get(10).ok(), row.get(11).ok()];
        let thumb_url = artwork
            .into_iter()
            .flatten()
            .map(|url| url.trim().to_owned())
            .find(|url| super::prefetch::usable_poster_url(url))
            .unwrap_or_default();
        let key = if thumb_url.is_empty() {
            String::new()
        } else {
            format!("{}__s", url_to_cache_key(&thumb_url))
        };
        let channel = row
            .get_ref(6)?
            .as_bytes()
            .ok()
            .map(parse_channel_meta)
            .unwrap_or_default();
        let resolution: Option<String> = row.get("airing_resolution")?;
        let airing_quality = crate::app::utils::airing_quality(
            row.get("airing_width").ok(),
            row.get("airing_height").ok(),
            resolution.as_deref(),
            channel.call_sign.as_deref(),
            channel.title.as_deref(),
        );
        list.push(PrepItem {
            airing_quality,
            film_key,
            movie_subscription_id: row
                .get_ref("movie_metadata")?
                .as_bytes()
                .ok()
                .and_then(movie_subscription_id),
            title,
            thumb_url,
            key,
            cached_path: None,
            begins_at: Some(begins_at),
            year,
            guid,
            tags_genre: row.get(5).ok(),
            channel_call_sign: channel.call_sign,
            channel_title: channel.title,
            channel_thumb: channel.thumb,
            guide_channel: channel.guide_channel,
            summary: row.get(7).ok(),
            audience_rating: row.get::<_, f64>(8).ok().map(|value| value as f32),
            critic_rating: row.get::<_, f64>(9).ok().map(|value| value as f32),
        });
        if last_emit.elapsed() >= Duration::from_millis(600) {
            progress(list.len());
            last_emit = Instant::now();
        }
    }
    Ok(list)
}

const MIN_COPY_INTERVAL_HOURS: u64 = 24;

fn last_sync_marker_path(local_db: &Path) -> PathBuf {
    PathBuf::from(format!("{}.last_sync", local_db.display()))
}

fn sqlite_sidecar_path(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(format!("-{suffix}"));
    PathBuf::from(name)
}

fn mirror_due(dst: &Path) -> bool {
    // A leftover marker must not suppress recovery after deleting a local DB.
    if !fs::metadata(dst).is_ok_and(|m| m.len() > 0) {
        return true;
    }
    let age = fs::metadata(last_sync_marker_path(dst))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok());
    age.is_none_or(|age| age >= Duration::from_secs(MIN_COPY_INTERVAL_HOURS * 3600))
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FileState {
    len: u64,
    modified: SystemTime,
    header: Vec<u8>,
}

fn file_state(path: &Path, header_bytes: u64) -> io::Result<FileState> {
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    let mut header = Vec::new();
    file.take(header_bytes).read_to_end(&mut header)?;
    Ok(FileState {
        len: meta.len(),
        modified: meta.modified()?,
        header,
    })
}

fn source_state(src: &Path) -> io::Result<(FileState, Option<FileState>)> {
    // A nonempty rollback journal can mean uncommitted pages are in the main file.
    if fs::metadata(sqlite_sidecar_path(src, "journal")).map_or_else(
        |e| {
            if e.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(e)
            }
        },
        |m| Ok(m.len() != 0),
    )? {
        return Err(io::Error::other(
            "source has a rollback journal; try again when idle",
        ));
    }
    let main = file_state(src, 100)?;
    let wal = match file_state(&sqlite_sidecar_path(src, "wal"), 32) {
        Ok(state) => Some(state),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    Ok((main, wal))
}

const COPY_CHUNK_BYTES: u64 = 16 * 1024 * 1024;

fn copy_contents(
    src: &Path,
    dst: &Path,
    mut remaining: u64,
    mut after_chunk: impl FnMut(u64) -> io::Result<()>,
) -> io::Result<()> {
    // Keep the staging files writable when the NAS files are read-only.
    let mut input = fs::File::open(src)?;
    let mut output = fs::File::create(dst)?;
    // Copy only the length captured before the transfer. A writer can keep
    // appending to its WAL faster than a client can read it over a slow share.
    while remaining > 0 {
        let amount = remaining.min(COPY_CHUNK_BYTES);
        let copied = io::copy(&mut input.by_ref().take(amount), &mut output)?;
        if copied != amount {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{} became shorter during copy", src.display()),
            ));
        }
        remaining -= copied;
        after_chunk(copied)?;
    }
    Ok(())
}

fn copy_sqlite_snapshot(src: &Path, dst: &Path) -> io::Result<()> {
    copy_validated_snapshot(src, dst, false, false).map(|_| ())
}

// Stock SQLite cannot open Plex's ICU/naturalsort indexes or its FTS4 tokenizer.
// Walk the underlying table and index pages, decode the records PEX reads, and
// check the schema and freelist without invoking Plex's search extensions.
fn check_plex_pages(db: &Connection, library: bool) -> io::Result<String> {
    let damaged: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.dbstat('main') WHERE pagetype = 'corrupted')",
            [],
            |r| r.get(0),
        )
        .map_err(io::Error::other)?;
    if damaged {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SQLite B-tree contains a damaged page",
        ));
    }
    let tables: &[&str] = if library {
        &["metadata_items", "media_items", "media_parts"]
    } else {
        &["metadata_items", "media_items"]
    };
    for table in tables {
        // Like quick_check, ask for the last column's type to decode each row
        // header without materialising large text or blob values.
        let column: String = db
            .query_row(
                "SELECT name FROM pragma_table_info(?1, 'main') ORDER BY cid DESC LIMIT 1",
                [table],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        let quoted = column.replace('"', "\"\"");
        let scan = format!("SELECT count(typeof(\"{quoted}\")) FROM main.{table} NOT INDEXED");
        db.query_row(&scan, [], |_| Ok(()))
            .map_err(io::Error::other)?;
    }
    db.query_row("PRAGMA main.quick_check(sqlite_schema)", [], |r| r.get(0))
        .map_err(io::Error::other)
}

fn validate_snapshot(db: &Connection, library: bool) -> io::Result<()> {
    let check: String = match db.query_row("PRAGMA main.quick_check", [], |r| r.get(0)) {
        Ok(check) => check,
        Err(err) if missing_plex_extension(&err) => check_plex_pages(db, library)?,
        Err(err) => return Err(io::Error::other(err)),
    };
    if check != "ok" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SQLite validation failed: {check}"),
        ));
    }
    db.prepare("SELECT m.id, mi.id FROM metadata_items m, media_items mi LIMIT 0")
        .map_err(io::Error::other)?;
    if library {
        db.prepare(
            "SELECT m.id, m.guid, m.title, m.original_title, m.year, m.updated_at, m.added_at,
            mi.id, mi.updated_at, mp.id, mp.file, mp.size, mp.updated_at
            FROM metadata_items m JOIN media_items mi ON mi.metadata_item_id=m.id
            JOIN media_parts mp ON mp.media_item_id=mi.id WHERE m.metadata_type=1 LIMIT 0",
        )
        .map_err(io::Error::other)?;
    }
    Ok(())
}

fn missing_plex_extension(err: &rusqlite::Error) -> bool {
    let message = err.to_string();
    message.starts_with("no such collation sequence: ")
        || message.starts_with("unknown tokenizer: ")
}

fn usable_library(path: &Path) -> bool {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .is_some_and(|db| validate_snapshot(&db, true).is_ok())
}

fn copy_validated_snapshot(
    src: &Path,
    dst: &Path,
    library: bool,
    completed_backup: bool,
) -> io::Result<FileState> {
    copy_snapshot_with_hooks(
        src,
        dst,
        library,
        completed_backup,
        || std::thread::sleep(Duration::from_millis(250)),
        || {},
    )
}

fn copy_snapshot_with_hooks(
    src: &Path,
    dst: &Path,
    library: bool,
    completed_backup: bool,
    wait_for_quiet: impl FnOnce(),
    after_copy: impl FnOnce(),
) -> io::Result<FileState> {
    source_guard::check(dst, [src])?;
    let before = source_state(src)?;
    let live_library = library && !completed_backup;
    if live_library {
        wait_for_quiet();
        if before != source_state(src)? {
            return Err(io::Error::other(
                "live library changed before transfer; skipped the full copy",
            ));
        }
    }
    if completed_backup && before.1.as_ref().is_some_and(|wal| wal.len > 0) {
        return Err(io::Error::other("backup still has an active WAL"));
    }
    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let scratch = tempfile::tempdir_in(parent)?;
    let staged = tempfile::NamedTempFile::new_in(scratch.path())?.into_temp_path();
    let staged_path: &Path = staged.as_ref();
    let verify_source = || {
        if before != source_state(src)? {
            return Err(io::Error::other(
                "source changed during copy; no new snapshot installed",
            ));
        }
        Ok(())
    };
    let mut last_probe = Instant::now();
    let mut bytes_since_probe = 0;
    let mut check_progress = |bytes| {
        bytes_since_probe += bytes;
        if live_library
            && bytes_since_probe >= COPY_CHUNK_BYTES
            && last_probe.elapsed() >= Duration::from_secs(1)
        {
            verify_source()?;
            last_probe = Instant::now();
            bytes_since_probe = 0;
        }
        Ok(())
    };
    copy_contents(src, staged_path, before.0.len, &mut check_progress)?;
    if let Some(wal) = &before.1 {
        copy_contents(
            &sqlite_sidecar_path(src, "wal"),
            &sqlite_sidecar_path(staged_path, "wal"),
            wal.len,
            check_progress,
        )?;
    }
    after_copy();
    verify_source()?;
    // Rebuild SHM locally, validate the guide/library schema and let SQLite replay
    // committed WAL frames. Never copy a live SHM file or touch SQLite on the NAS.
    let db = Connection::open(staged_path).map_err(io::Error::other)?;
    validate_snapshot(&db, library)?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata_items), EXISTS(SELECT 1 FROM media_items)",
        [],
        |_| Ok(()),
    )
    .map_err(io::Error::other)?;
    db.pragma_update(None, "journal_mode", "DELETE")
        .map_err(io::Error::other)?;
    if !dst.try_exists()? && !has_sidecars(dst)? {
        drop(db);
        staged.persist_noclobber(dst).map_err(|err| err.error)?;
        return Ok(before.0);
    }
    // SQLite serializes installation with readers and writers, including WAL readers.
    // Only a confirmed corrupt standalone destination needs filesystem replacement.
    match db.backup(
        rusqlite::MAIN_DB,
        dst,
        None::<fn(rusqlite::backup::Progress)>,
    ) {
        Ok(()) => Ok(()),
        Err(e) if is_corruption(&e) && !has_sidecars(dst)? => {
            let probe = Connection::open_with_flags(dst, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .and_then(|local| {
                    local.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))
                });
            if !probe.is_err_and(|err| is_corruption(&err)) {
                return Err(io::Error::other(e));
            }
            drop(db);
            staged.persist(dst).map_err(|err| err.error)
        }
        Err(e) => Err(io::Error::other(e)),
    }?;
    Ok(before.0)
}

fn has_sidecars(path: &Path) -> io::Result<bool> {
    for suffix in ["wal", "shm", "journal"] {
        if sqlite_sidecar_path(path, suffix).try_exists()? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_corruption(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

fn sync_mirror(src: &Path, dst: &Path, force: bool) -> io::Result<bool> {
    if !force && !mirror_due(dst) {
        return Ok(false);
    }
    // Committed WAL frames can change without any change to the main DB file.
    copy_sqlite_snapshot(src, dst)?;
    fs::write(last_sync_marker_path(dst), b"ok")?;
    Ok(true)
}

pub(super) fn sync_library_db_from_source(
    source: &super::owned::LibrarySource,
    configured_databases: &[PathBuf],
    force: bool,
    report: impl Fn(String),
) -> Result<String, String> {
    use super::owned::LibrarySource;
    let dst = local_library_db_path();
    source_guard::check(&dst, configured_databases.iter().map(PathBuf::as_path))
        .map_err(|e| e.to_string())?;
    match source {
        LibrarySource::Snapshots(dir) => snapshots::refresh(dir, &dst, report),
        LibrarySource::Live(src) => recover_library(src, &dst, force, report),
        LibrarySource::Manual(_) => {
            for marker in [provenance_path(&dst), last_sync_marker_path(&dst)] {
                if marker.try_exists().map_err(|e| e.to_string())? {
                    return Err("The local library mirror belongs to a previous connection. Choose its live library or completed snapshot folder in Connect to Plex, or select Guide only.".into());
                }
            }
            Ok("Using the manually supplied local library snapshot.".into())
        }
    }
}

fn provenance_path(dst: &Path) -> PathBuf {
    PathBuf::from(format!("{}.snapshot.json", dst.display()))
}

// Older builds wrote the human-readable provenance directly. A snapshot-folder receipt
// (a JSON object), or a message for a different server, must not satisfy live refresh.
fn previous_live_snapshot(src: &Path, dst: &Path) -> Option<String> {
    let bytes = match fs::read(provenance_path(dst)) {
        Ok(bytes) => bytes,
        // Older portable installs and user-supplied mirrors may have no provenance.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Some("Previous local library snapshot (source and date unknown).".into());
        }
        Err(_) => return None,
    };
    let message = serde_json::from_slice::<String>(&bytes).ok()?;
    if message.starts_with(&format!(
        "Library snapshot: live source {}, copied ",
        src.display()
    )) {
        return Some(message);
    }
    let (date, detail) = message
        .strip_prefix("Library snapshot: Plex backup dated ")?
        .split_once(", ")?;
    let (path, _) = detail
        .rsplit_once(". Owned and recording badges reflect that date.")
        .or_else(|| detail.rsplit_once(". Library-based badges reflect that date."))?;
    (chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
        && path == format!("{}-{date}", src.display()))
    .then_some(message)
}

fn backup_candidates(src: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let Some(parent) = src.parent() else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    let prefix = format!("{}-", src.file_name().unwrap_or_default().to_string_lossy());
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(date) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(parsed) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
            continue;
        };
        if parsed.format("%Y-%m-%d").to_string() != date
            || parsed > chrono::Local::now().date_naive()
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        // Completed Plex backups have an exact date suffix, and no active SQLite journal.
        if ["wal", "journal"].iter().any(|suffix| {
            match fs::metadata(sqlite_sidecar_path(&entry.path(), suffix)) {
                Ok(m) => m.len() > 0,
                Err(e) => e.kind() != io::ErrorKind::NotFound,
            }
        }) {
            continue;
        }
        candidates.push((date.to_string(), entry.path()));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(candidates)
}

/// Read the live library once per refresh. A busy DVR can be retried by the user later.
/// A backup never advances the live-source freshness marker.
fn recover_library(
    src: &Path,
    dst: &Path,
    force: bool,
    report: impl Fn(String),
) -> Result<String, String> {
    source_guard::check(dst, [src]).map_err(|e| e.to_string())?;
    if !force && !mirror_due(dst) {
        if let Some(previous) = previous_live_snapshot(src, dst).filter(|_| usable_library(dst)) {
            return Ok(previous);
        }
    }
    report("Checking the live Plex library before copying...".into());
    let failure = match copy_validated_snapshot(src, dst, true, false) {
        Ok(_) => {
            let message = format!(
                "Library snapshot: live source {}, copied {}.",
                src.display(),
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            );
            let _ = fs::write(provenance_path(dst), serde_json::to_vec(&message).unwrap());
            let _ = fs::write(last_sync_marker_path(dst), b"ok");
            return Ok(message);
        }
        Err(error) => error,
    };
    if let Some(previous) = previous_live_snapshot(src, dst).filter(|_| usable_library(dst)) {
        return Ok(format!("Live library unavailable ({failure}). Keeping {previous} Use Retry owned scan when Plex is idle."));
    }
    report("Live copy unavailable; checking completed dated Plex library backups...".into());
    let mut rejected = Vec::new();
    match backup_candidates(src) {
        Ok(candidates) => {
            for (date, backup) in candidates {
                match copy_validated_snapshot(&backup, dst, true, true) {
                    Ok(_) => {
                        let message = format!("Library snapshot: Plex backup dated {date}, {}. Owned and recording badges reflect that date. Live source is still configured; use Retry owned scan to update.", backup.display());
                        // Keep this mirror due so the next launch tries the live source again.
                        let _ = fs::remove_file(last_sync_marker_path(dst));
                        let _ =
                            fs::write(provenance_path(dst), serde_json::to_vec(&message).unwrap());
                        return Ok(message);
                    }
                    Err(e) => rejected.push(format!("{}: {e}", backup.display())),
                }
            }
        }
        Err(e) => rejected.push(e.to_string()),
    }
    Err(format!("No usable library snapshot. Live file copy failed: {failure}. No completed, valid dated backup was available. {} Check the share, or use Retry owned scan after the recording finishes. You can also choose Guide only in Connect to Plex.", rejected.join("; ")))
}

fn attach_cached_posters(items: &mut [PrepItem]) {
    if !items.iter().any(|item| !item.key.is_empty()) {
        return;
    }
    let hotset = super::prefs::load_hotset_manifest().unwrap_or_default();
    let files = super::cache::PosterFiles::read(super::cache::poster_cache_dir()).ok();
    let mut paths = std::collections::HashMap::new();
    for item in items.iter_mut().filter(|item| !item.key.is_empty()) {
        let path = paths.entry(item.key.as_str()).or_insert_with(|| {
            hotset
                .get(&item.key)
                .filter(|p| p.is_file())
                .cloned()
                .or_else(|| {
                    match &files {
                        Some(files) => files.find(&item.key),
                        // A folder can allow opening known files while denying enumeration.
                        None => super::cache::find_any_by_key(&item.key),
                    }
                })
        });
        item.cached_path.clone_from(path);
    }
}

/// Spawn the background thread that prepares the poster list (no downloads here).
pub(crate) fn spawn_poster_prep(tx: Sender<PrepMsg>) {
    std::thread::spawn(move || {
        let send = |m: PrepMsg| {
            let _ = tx.send(m);
        };

        // Resolve DB paths from config
        let cfg = load_config();
        let db_path = local_db_path();
        let sources = cfg
            .plex_epg_db_source
            .iter()
            .chain(&cfg.plex_library_db_source);
        if let Err(err) = source_guard::check(&db_path, sources.map(PathBuf::as_path)) {
            send(PrepMsg::Error(err.to_string()));
            return;
        }
        if let Some(parent) = db_path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                send(PrepMsg::Error(format!(
                    "Failed to create database directory {}: {err}",
                    parent.display()
                )));
                return;
            }
        }

        // Tell both the UI and the terminal which DB we're using
        let msg = format!(
            "Stage 2/4 – Opening Plex EPG database\n{}",
            db_path.display()
        );
        send(PrepMsg::Info(msg.clone()));
        info!("prep: {msg}");

        // Optional daily copy from source to local
        if let Some(src_path) = cfg.plex_epg_db_source.as_ref() {
            let src = src_path.as_path();
            match sync_mirror(src, &db_path, false) {
                Ok(true) => send(PrepMsg::Info(
                    "Stage 2/4 – Copied Plex EPG database.".into(),
                )),
                Ok(false) => send(PrepMsg::Info(
                    "Stage 2/4 – Using fresh local EPG database.".into(),
                )),
                Err(err) => {
                    let message = format!("Copying Plex EPG from {} failed: {err}", src.display());
                    warn!("{message}");
                    if !db_path.exists() {
                        send(PrepMsg::Error(message));
                        return;
                    }
                    send(PrepMsg::Info(format!("{message}; using the local copy.")));
                }
            }
        }

        // Open DB read-only
        let flags_common = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        #[cfg(not(windows))]
        let flags = flags_common | OpenFlags::SQLITE_OPEN_URI;
        #[cfg(windows)]
        let flags = flags_common;

        let conn = match Connection::open_with_flags(&db_path, flags) {
            Ok(c) => c,
            Err(e) => {
                send(PrepMsg::Error(format!("open db failed: {e}")));
                return;
            }
        };
        let _ = conn.busy_timeout(Duration::from_secs(10));
        let _ = conn.pragma_update(None, "temp_store", "MEMORY");

        let have_meta = table_exists(&conn, "metadata_items");
        let have_media = table_exists(&conn, "media_items");
        info!("prep: table check metadata_items={have_meta}, media_items={have_media}");
        send(PrepMsg::Info(format!(
            "Tables present → metadata_items: {have_meta}, media_items: {have_media}"
        )));
        if !(have_meta && have_media) {
            send(PrepMsg::Error(
                "required tables missing (metadata_items, media_items)".into(),
            ));
            return;
        }

        send(PrepMsg::Info(
            "Stage 2/4 - Reading film airings from the local guide.".into(),
        ));
        match read_guide_airings(&conn, |count| {
            send(PrepMsg::Info(format!(
                "Stage 2/4 - Read {count} film airings…"
            )));
        }) {
            Ok(mut list) => {
                drop(conn);
                attach_cached_posters(&mut list);
                info!("prep: read {} film airings", list.len());
                send(PrepMsg::Done(list));
            }
            Err(error) => send(PrepMsg::Error(format!(
                "Reading guide airings failed: {error}"
            ))),
        }
    });
}

impl crate::app::PexApp {
    /// Phase 2+3: poster prep warm-up (one-shot on app launch)
    pub(crate) fn start_poster_prep(&mut self) {
        if self.prep_started {
            return;
        }
        self.prep_started = true;
        self.boot_phase = super::BootPhase::CheckingNew;
        self.set_startup_progress_floor(crate::app::STARTUP_STAGE2_STARTED);
        self.set_status("Stage 2/4 - Preparing Plex guide data (scans the EPG so the grid knows what's airing).");

        let (tx, rx) = std::sync::mpsc::channel::<crate::app::PrepMsg>();
        self.prep_rx = Some(rx);

        // Hand off all the work to the prep module
        crate::app::prep::spawn_poster_prep(tx);
    }

    pub(crate) fn poll_prep(&mut self, ctx: &eg::Context) {
        use std::sync::mpsc::TryRecvError;
        const MAX_MSGS: usize = 16;

        let mut seen_any = false;
        let mut processed = 0;

        if let Some(rx) = self.prep_rx.take() {
            let mut keep: Option<std::sync::mpsc::Receiver<crate::app::PrepMsg>> = Some(rx);

            while let Some(r) = keep.as_ref() {
                if processed >= MAX_MSGS {
                    break;
                }
                match r.try_recv() {
                    Ok(crate::app::PrepMsg::Info(s)) => {
                        self.set_status(s);
                        if !matches!(
                            self.boot_phase,
                            crate::app::BootPhase::Caching | crate::app::BootPhase::Ready
                        ) {
                            self.boot_phase = crate::app::BootPhase::Caching;
                        }
                        processed += 1;
                        seen_any = true;
                    }
                    Ok(crate::app::PrepMsg::Done(list)) => {
                        // Convert manifest rows into UI rows
                        self.rating_states.clear();
                        self.channel_icon_textures.clear();
                        self.rows = list
                            .into_iter()
                            .map(|item| {
                                let airing = item.begins_at.and_then(|ts| {
                                    SystemTime::UNIX_EPOCH
                                        .checked_add(Duration::from_secs(u64::try_from(ts).ok()?))
                                });

                                let channel_raw = item
                                    .channel_call_sign
                                    .or_else(|| item.channel_title.clone())
                                    .or_else(|| crate::app::utils::host_from_url(&item.thumb_url));

                                let channel_title_original =
                                    item.channel_title.filter(|s| !s.trim().is_empty());

                                let channel_display = channel_title_original
                                    .as_ref()
                                    .or(channel_raw.as_ref())
                                    .map(|s| crate::app::utils::humanize_channel(s));
                                let state = if item.cached_path.is_some() {
                                    crate::app::PosterState::Cached
                                } else {
                                    crate::app::PosterState::Pending
                                };
                                let genres = item
                                    .tags_genre
                                    .as_deref()
                                    .map(crate::app::utils::parse_genres)
                                    .unwrap_or_default();
                                let summary = item.summary.and_then(|s| {
                                    let trimmed = s.trim();
                                    if trimmed.is_empty() {
                                        None
                                    } else {
                                        Some(trimmed.to_string())
                                    }
                                });

                                crate::app::PosterRow {
                                    film_key: item.film_key,
                                    movie_subscription_id: item.movie_subscription_id,
                                    title: item.title,
                                    url: item.thumb_url,
                                    key: item.key,
                                    airing,
                                    year: item.year,
                                    channel: channel_display,
                                    channel_raw,
                                    channel_title: channel_title_original,
                                    channel_thumb: item.channel_thumb,
                                    guide_channel: item.guide_channel,
                                    genres,
                                    guid: item.guid,
                                    summary,
                                    audience_rating: item.audience_rating,
                                    critic_rating: item.critic_rating,
                                    path: item.cached_path,
                                    state,
                                    owned: false, // filled in by apply_owned_flags()
                                    owned_modified: None,
                                    owned_quality: Default::default(),
                                    airing_quality: item.airing_quality,
                                    scheduled: Default::default(),
                                }
                            })
                            .collect();

                        let mut seen_icons = std::collections::HashSet::new();
                        let icon_urls: Vec<String> = self
                            .rows
                            .iter()
                            .filter_map(|row| row.channel_thumb.clone())
                            .filter(|url| !url.is_empty() && seen_icons.insert(url.clone()))
                            .collect();
                        if !icon_urls.is_empty() {
                            for url in &icon_urls {
                                self.channel_icon_pending.insert(url.clone());
                            }
                            Self::spawn_channel_icon_prefetch(icon_urls, ctx.clone());
                        }

                        // Scheduled recordings (from Plex library DB)
                        self.apply_scheduled_flags();

                        // Owned flags (if ready)
                        self.apply_owned_flags();
                        self.set_startup_progress_floor(crate::app::STARTUP_STAGE2_DONE);
                        let poster_done_status =
                            format!("Poster prep complete. {} items ready.", self.rows.len());
                        self.boot_phase = crate::app::BootPhase::Ready;
                        if self.owned_preload.is_some()
                            || self.owned_library.is_some()
                            || !self.owned_scan_in_progress
                        {
                            self.set_status(poster_done_status);
                        } else {
                            self.set_status("Poster prep complete. Scanning owned library...");
                        }

                        self.start_prefetch(
                            ctx,
                            std::env::var_os("PEX_DISABLE_PREFETCH").is_some(),
                        );

                        keep = None;
                        seen_any = true;
                    }
                    Ok(crate::app::PrepMsg::Error(e)) => {
                        self.setup_errors = vec![format!("Could not load the guide: {e}. Choose a Plex Databases folder below, or retry after checking the share.")];
                        self.connection.open = true;
                        self.set_status(format!("Poster prep error: {e}"));
                        keep = None;
                        seen_any = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        keep = None;
                        break;
                    }
                }
            }

            if let Some(rx_back) = keep {
                self.prep_rx = Some(rx_back);
            }
        }

        if seen_any {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod guide_tests {
    use super::*;

    #[test]
    fn each_airing_uses_its_own_quality_and_preserves_unknown() {
        use crate::app::types::AiringQuality::{Hd, Sd, Unknown};
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("test_fixtures.sql")).unwrap();
        db.execute_batch(r#"
            ALTER TABLE media_items ADD COLUMN media_analysis_version INTEGER;
            ALTER TABLE media_items ADD COLUMN video_resolution TEXT;
            UPDATE metadata_items SET tags_genre='HD|Drama';
            INSERT INTO media_items(id,metadata_item_id,begins_at,width,height,video_resolution,extra_data) VALUES
                (2,1,2,720,576,'1080p','{"at:channelTitle":"Cinema HD"}'),
                (3,1,3,0,0,'1080i',NULL),
                (4,1,4,NULL,NULL,NULL,'{"at:channelCallSign":"StationSD"}'),
                (5,1,5,NULL,NULL,NULL,'{"at:channelTitle":"Cinema 720"}'),
                (6,1,6,1280,720,NULL,'{"at:channelCallSign":"Cinema","at:channelTitle":"Cinema HD"}'),
                (7,1,7,NULL,NULL,'SD',NULL),
                (8,1,8,1280,NULL,NULL,NULL);
            UPDATE media_items SET media_analysis_version=8 WHERE id IN (1,2,8);
        "#).unwrap();
        let qualities = || {
            read_guide_airings(&db, |_| {})
                .unwrap()
                .into_iter()
                .map(|airing| airing.airing_quality)
                .collect::<Vec<_>>()
        };
        assert_eq!(qualities(), [Hd, Sd, Hd, Sd, Sd, Hd, Sd, Hd]);
        // Older guides without an analysis marker still have useful resolution
        // and station labels. Dimensions alone cannot confirm an upgrade.
        db.execute_batch("ALTER TABLE media_items DROP COLUMN media_analysis_version;")
            .unwrap();
        assert_eq!(qualities(), [Unknown, Hd, Hd, Sd, Sd, Hd, Sd, Unknown]);
        db.execute_batch(
            "ALTER TABLE media_items DROP COLUMN width;
                          ALTER TABLE media_items DROP COLUMN height;
                          ALTER TABLE media_items DROP COLUMN video_resolution;",
        )
        .unwrap();
        assert_eq!(
            qualities(),
            [Unknown, Hd, Unknown, Sd, Sd, Hd, Unknown, Unknown]
        );
        db.execute_batch("ALTER TABLE media_items DROP COLUMN extra_data;")
            .unwrap();
        assert_eq!(qualities(), [Unknown; 8]);
    }

    #[test]
    fn nominal_dimensions_need_independent_quality_evidence() {
        use crate::app::types::AiringQuality::{Hd, Sd, Unknown};
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("test_fixtures.sql")).unwrap();
        db.execute_batch(
            "ALTER TABLE media_items ADD COLUMN media_analysis_version INTEGER DEFAULT 0;
            ALTER TABLE media_items ADD COLUMN video_resolution TEXT;",
        )
        .unwrap();
        for (station, resolution, expected) in [
            ("011 Provider", None, Sd),
            ("Provider SD", None, Sd),
            ("ProviderHD", None, Hd),
            ("041 ProviderHD (The Network)", None, Hd),
            ("Unbranded", Some("1080i"), Hd),
            ("Provider", Some("576i"), Sd),
            ("Provider SD", Some("1080i"), Unknown),
            ("Provider HD / SD", None, Unknown),
            ("ProviderHD (ProviderSD)", None, Unknown),
            ("", None, Unknown),
            ("1080", None, Unknown),
            ("2.1", None, Unknown),
            ("Unknown channel", None, Unknown),
            ("N/A", None, Unknown),
        ] {
            db.execute(
                "UPDATE media_items SET width=1280,height=720,extra_data=?1,video_resolution=?2",
                rusqlite::params![
                    serde_json::json!({"at:channelTitle":station}).to_string(),
                    resolution
                ],
            )
            .unwrap();
            let airings = read_guide_airings(&db, |_| {}).unwrap();
            assert_eq!(
                airings[0].airing_quality, expected,
                "{station} {resolution:?}"
            );
        }
    }

    #[test]
    fn reads_airings_with_either_artwork_column_or_neither() {
        for (drop_columns, expected) in [
            ("", "https://fixture.invalid/user.jpg"),
            ("ALTER TABLE metadata_items DROP COLUMN user_thumb_url", "https://fixture.invalid/secondary.jpg"),
            ("ALTER TABLE metadata_items DROP COLUMN thumb_url", "https://fixture.invalid/user.jpg"),
            ("ALTER TABLE metadata_items DROP COLUMN user_thumb_url; ALTER TABLE metadata_items DROP COLUMN thumb_url", ""),
        ] {
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch(include_str!("test_fixtures.sql")).unwrap();
            db.execute_batch("UPDATE metadata_items SET guid=NULL, user_thumb_url='https://fixture.invalid/user.jpg', thumb_url='https://fixture.invalid/secondary.jpg';
                INSERT INTO metadata_items (id, metadata_type, title, year) VALUES
                (2, 1, 'Fixture Movie', 1980), (3, 1, 'Fixture Movie', NULL), (4, 1, 'Fixture Movie', NULL);
                INSERT INTO media_items (id,metadata_item_id,begins_at) VALUES (2,2,1), (3,3,2), (4,4,3)").unwrap();
            db.execute_batch(drop_columns).unwrap();
            let films = read_guide_airings(&db, |_| {}).unwrap();
            assert_eq!(films.len(), 4);
            assert_eq!(films[0].thumb_url, expected);
            assert_ne!(films[0].film_key, films[1].film_key);
            assert_ne!(films[2].film_key, films[3].film_key);
        }
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    fn library(path: &Path) {
        crate::app::test_fixtures::database(path);
    }

    fn plex_search_schema(path: &Path, collation: &str, tokenizer: bool) {
        library(path);
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE INDEX title_sort ON metadata_items(title COLLATE NOCASE);
             PRAGMA writable_schema=ON;",
        )
        .unwrap();
        db.execute(
            "UPDATE sqlite_schema SET sql=replace(sql, 'NOCASE', ?1) WHERE name='title_sort'",
            [collation],
        )
        .unwrap();
        if tokenizer {
            // Build the real FTS shadow tables before simulating the tokenizer
            // Plex used when it created this file on another machine.
            db.execute_batch(
                "CREATE VIRTUAL TABLE fts4_metadata_titles_icu USING fts4(title, tokenize=simple);
                 UPDATE sqlite_schema SET sql=replace(sql, 'tokenize=simple', 'tokenize=collating')
                   WHERE name='fts4_metadata_titles_icu';",
            )
            .unwrap();
        }
    }

    #[test]
    fn plex_search_extensions_allow_both_live_and_completed_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        for (collation, tokenizer) in [
            ("naturalsort", false),
            ("icu_root", false),
            ("icu_root", true),
        ] {
            let src = dir
                .path()
                .join(format!("source-{collation}-{tokenizer}.db"));
            let dst = dir
                .path()
                .join(format!("mirror-{collation}-{tokenizer}.db"));
            plex_search_schema(&src, collation, tokenizer);
            let db = Connection::open(&src).unwrap();
            let err = db
                .query_row::<String, _, _>("PRAGMA quick_check", [], |r| r.get(0))
                .unwrap_err();
            assert!(missing_plex_extension(&err), "{err}");
            drop(db);
            copy_validated_snapshot(&src, &dst, true, tokenizer).unwrap();
            assert!(usable_library(&dst));
            let db = Connection::open(&dst).unwrap();
            let title: String = db
                .query_row("SELECT title FROM metadata_items WHERE id=1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(title, "Fixture Movie");
        }
    }

    #[test]
    fn damaged_index_page_in_plex_snapshot_preserves_the_existing_mirror() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("local.db");
        plex_search_schema(&src, "icu_root", true);
        library(&dst);
        let previous = fs::read(&dst).unwrap();
        let db = Connection::open(&src).unwrap();
        let root: u64 = db
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name='title_sort'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let page_size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        drop(db);
        let mut file = fs::OpenOptions::new().write(true).open(&src).unwrap();
        file.seek(SeekFrom::Start((root - 1) * page_size)).unwrap();
        file.write_all(&[0xff]).unwrap();
        drop(file);

        assert!(copy_validated_snapshot(&src, &dst, true, true).is_err());
        assert_eq!(fs::read(&dst).unwrap(), previous);
    }

    #[test]
    fn malformed_plex_record_header_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        plex_search_schema(&src, "icu_root", true);
        let db = Connection::open(&src).unwrap();
        let (root, page_size): (usize, usize) = db.query_row(
            "SELECT rootpage, page_size FROM sqlite_schema, pragma_page_size WHERE name='media_parts'",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        drop(db);
        let mut bytes = fs::read(&src).unwrap();
        let page = (root - 1) * page_size;
        let cell = page + u16::from_be_bytes([bytes[page + 8], bytes[page + 9]]) as usize;
        // The fixture's single row has a one-byte payload length and rowid.
        assert!(bytes[cell] < 127);
        assert_eq!(bytes[cell + 1], 1);
        bytes[cell + 2] = 127; // Declared header exceeds the entire payload.
        fs::write(&src, bytes).unwrap();
        let db = Connection::open(&src).unwrap();
        assert!(validate_snapshot(&db, true).is_err());
    }

    #[test]
    fn busy_first_scan_uses_newest_valid_completed_backup_then_returns_to_live() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("com.plexapp.plugins.library.db");
        let dst = dir.path().join("local/library.db");
        library(&src);
        fs::write(sqlite_sidecar_path(&src, "journal"), b"busy").unwrap();
        library(&dir.path().join("com.plexapp.plugins.library.db-2020-01-01"));
        library(&dir.path().join("com.plexapp.plugins.library.db-2020-01-02"));
        // Reject corrupt, wrong-schema, unfinished and unrelated candidates, even if newer.
        fs::write(
            dir.path().join("com.plexapp.plugins.library.db-2020-01-03"),
            b"corrupt",
        )
        .unwrap();
        drop(source(
            &dir.path().join("com.plexapp.plugins.library.db-2020-01-04"),
        ));
        let unfinished = dir.path().join("com.plexapp.plugins.library.db-2020-01-05");
        library(&unfinished);
        fs::write(sqlite_sidecar_path(&unfinished, "wal"), b"in progress").unwrap();
        library(
            &dir.path()
                .join("com.plexapp.plugins.library.db-2020-01-06.tmp"),
        );
        library(&dir.path().join("com.plexapp.plugins.library.db-2020-13-01"));
        let attempts = std::cell::Cell::new(0);
        let message = recover_library(&src, &dst, false, |s| {
            if s.starts_with("Checking the live Plex library") {
                attempts.set(attempts.get() + 1);
            }
        })
        .unwrap();
        assert_eq!(attempts.get(), 1);
        assert!(message.contains("2020-01-02"));
        assert!(usable_library(&dst));
        assert!(mirror_due(&dst));
        assert!(!last_sync_marker_path(&dst).exists());
        // The DVR preview used different wording in receipts for the same backup.
        let preview = message.replace("Owned and recording badges", "Library-based badges");
        fs::write(provenance_path(&dst), serde_json::to_vec(&preview).unwrap()).unwrap();
        assert_eq!(previous_live_snapshot(&src, &dst), Some(preview));
        assert!(previous_live_snapshot(&dir.path().join("another.db"), &dst).is_none());
        fs::remove_file(sqlite_sidecar_path(&src, "journal")).unwrap();
        let live = recover_library(&src, &dst, false, |_| {}).unwrap();
        assert!(live.contains("live source"));
        assert!(!mirror_due(&dst));
        assert!(fs::read_to_string(provenance_path(&dst))
            .unwrap()
            .contains("live source"));
    }

    #[test]
    fn busy_without_backup_is_actionable_and_does_not_install_a_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("com.plexapp.plugins.library.db");
        let dst = dir.path().join("local.db");
        library(&src);
        fs::write(sqlite_sidecar_path(&src, "journal"), b"busy").unwrap();
        let error = recover_library(&src, &dst, false, |_| {}).unwrap_err();
        assert!(error.contains("Live file copy failed"));
        assert!(error.contains("Retry owned scan"));
        assert!(!dst.exists());
        assert!(!last_sync_marker_path(&dst).exists());
    }

    #[test]
    fn corrupt_local_mirror_can_recover_from_a_completed_backup() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("com.plexapp.plugins.library.db");
        let dst = dir.path().join("local.db");
        library(&src);
        fs::write(sqlite_sidecar_path(&src, "journal"), b"busy").unwrap();
        library(&dir.path().join("com.plexapp.plugins.library.db-2020-01-01"));
        fs::write(&dst, b"corrupt old mirror").unwrap();
        recover_library(&src, &dst, false, |_| {}).unwrap();
        assert!(usable_library(&dst));
    }

    #[test]
    fn locked_local_mirror_keeps_its_committed_data() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("mirror.db");
        library(&src);
        library(&dst);
        let writer = Connection::open(&dst).unwrap();
        writer
            .execute_batch(
                "UPDATE metadata_items SET title='Original mirror';
             BEGIN EXCLUSIVE;
             UPDATE metadata_items SET title='Uncommitted';",
            )
            .unwrap();
        assert!(copy_validated_snapshot(&src, &dst, true, false).is_err());
        writer.execute_batch("ROLLBACK").unwrap();
        drop(writer);
        let mirror = Connection::open(&dst).unwrap();
        let title: String = mirror
            .query_row("SELECT title FROM metadata_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Original mirror");
    }

    #[test]
    fn busy_source_is_read_once_then_a_later_user_refresh_can_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("com.plexapp.plugins.library.db");
        let dst = dir.path().join("local.db");
        library(&src);
        let journal = sqlite_sidecar_path(&src, "journal");
        fs::write(&journal, b"busy").unwrap();
        let attempts = std::cell::Cell::new(0);
        let first = recover_library(&src, &dst, true, |message| {
            if message.starts_with("Checking the live Plex library") {
                attempts.set(attempts.get() + 1);
            }
        });
        assert!(first.is_err());
        assert_eq!(attempts.get(), 1);
        assert_eq!(fs::read(&journal).unwrap(), b"busy");
        assert!(!dst.exists());
        fs::remove_file(&journal).unwrap();
        recover_library(&src, &dst, true, |_| {}).unwrap();
        assert!(usable_library(&dst));
        assert!(!mirror_due(&dst));
    }

    #[test]
    fn busy_refresh_keeps_usable_mirror_and_its_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("com.plexapp.plugins.library.db");
        let dst = dir.path().join("local.db");
        library(&src);
        let initial = recover_library(&src, &dst, true, |_| {}).unwrap();
        let old = fs::read(&dst).unwrap();
        fs::write(sqlite_sidecar_path(&src, "journal"), b"busy").unwrap();
        let message = recover_library(&src, &dst, true, |_| {}).unwrap();
        assert!(message.contains(&initial));
        assert!(message.contains("Keeping"));
        assert_eq!(old, fs::read(&dst).unwrap());
    }

    #[test]
    fn changed_source_is_rejected_without_replacing_existing_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("local.db");
        library(&src);
        copy_validated_snapshot(&src, &dst, true, false).unwrap();
        let old = fs::read(&dst).unwrap();
        let error = copy_snapshot_with_hooks(
            &src,
            &dst,
            true,
            false,
            || {},
            || {
                Connection::open(&src)
                    .unwrap()
                    .execute("UPDATE metadata_items SET title='changed'", [])
                    .unwrap();
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("source changed during copy"));
        assert_eq!(old, fs::read(&dst).unwrap());
    }

    #[test]
    fn wal_only_commit_in_preflight_skips_transfer_then_a_quiet_refresh_replays_it() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("library.db");
        // Staging is created beside the destination only when the transfer starts.
        let dst = dir.path().join("local/plex_library.db");
        library(&src);
        let writer = Connection::open(&src).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        writer
            .execute("UPDATE metadata_items SET title='First commit'", [])
            .unwrap();
        let main = file_state(&src, 100).unwrap();
        let err = copy_snapshot_with_hooks(
            &src,
            &dst,
            true,
            false,
            || {
                writer
                    .execute("UPDATE metadata_items SET title='Later commit'", [])
                    .unwrap();
            },
            || panic!("copied an active library"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("skipped the full copy"));
        assert_eq!(main, file_state(&src, 100).unwrap());
        assert!(!dst.parent().unwrap().exists());
        assert!(source_state(&src).unwrap().1.unwrap().len > 0);
        recover_library(&src, &dst, true, |_| {}).unwrap();
        let local = Connection::open_with_flags(&dst, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let title: String = local
            .query_row("SELECT title FROM metadata_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Later commit");
    }

    #[test]
    fn bounded_transfer_does_not_follow_growth_and_detects_truncation_between_chunks() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("payload");
        let dst = dir.path().join("staged");
        let length = COPY_CHUNK_BYTES + 8;
        let writer = fs::File::create(&src).unwrap();
        writer.set_len(length).unwrap();
        drop(writer);
        let mut bytes_copied = 0;
        copy_contents(&src, &dst, length, |bytes| {
            bytes_copied += bytes;
            if bytes_copied == COPY_CHUNK_BYTES {
                fs::OpenOptions::new()
                    .append(true)
                    .open(&src)?
                    .write_all(b"new frames")?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(bytes_copied, length);
        assert_eq!(fs::metadata(&dst).unwrap().len(), length);
        assert!(fs::metadata(&src).unwrap().len() > length);

        let err = copy_contents(&src, &dst, length, |bytes| {
            assert_eq!(bytes, COPY_CHUNK_BYTES);
            fs::OpenOptions::new()
                .write(true)
                .open(&src)?
                .set_len(bytes)
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(fs::metadata(&dst).unwrap().len(), COPY_CHUNK_BYTES);
    }

    fn source(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE metadata_items(id INTEGER);
             CREATE TABLE media_items(id INTEGER);
             INSERT INTO metadata_items VALUES (1);
             PRAGMA journal_mode=WAL;
             PRAGMA wal_autocheckpoint=0;",
        )
        .unwrap();
        db
    }

    fn count(path: &Path) -> i64 {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("SELECT count(*) FROM metadata_items", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn committed_wal_frames_reach_new_and_existing_mirrors() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("mirror.db");
        let db = source(&src);
        db.execute("INSERT INTO metadata_items VALUES (2)", [])
            .unwrap();
        assert!(sync_mirror(&src, &dst, false).unwrap());
        assert_eq!(count(&dst), 2);
        db.execute("INSERT INTO metadata_items VALUES (3)", [])
            .unwrap();
        assert!(!sync_mirror(&src, &dst, false).unwrap());
        fs::remove_file(last_sync_marker_path(&dst)).unwrap();
        // Simulate a prior version's WAL-mode mirror, open during refresh.
        let local = Connection::open(&dst).unwrap();
        local.pragma_update(None, "journal_mode", "WAL").unwrap();
        local
            .execute_batch("BEGIN; SELECT * FROM metadata_items;")
            .unwrap();
        assert!(sync_mirror(&src, &dst, false).unwrap());
        assert_eq!(count(&dst), 3);
        let old_rows: i64 = local
            .query_row("SELECT count(*) FROM metadata_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(old_rows, 2);
    }

    #[test]
    fn missing_mirror_is_recovered_even_with_a_fresh_marker() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("mirror.db");
        drop(source(&src));
        fs::write(last_sync_marker_path(&dst), b"ok").unwrap();
        assert!(sync_mirror(&src, &dst, false).unwrap());
        assert_eq!(count(&dst), 1);
    }

    #[test]
    fn failed_refresh_preserves_previous_mirror_and_remains_due() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("mirror.db");
        drop(source(&src));
        sync_mirror(&src, &dst, false).unwrap();
        fs::remove_file(last_sync_marker_path(&dst)).unwrap();
        fs::write(&src, b"invalid SQLite data").unwrap();
        assert!(sync_mirror(&src, &dst, false).is_err());
        assert!(mirror_due(&dst));
        assert_eq!(count(&dst), 1);
    }
}
