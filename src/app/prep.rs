// src/app/prep.rs
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

// --- local SQL (newer plex uses user_thumb_url; older uses thumb_url) ---
const SQL_POSTERS_USER_THUMB: &str = r#"
SELECT
  m.title,
  m.user_thumb_url,
  mi.begins_at,
  m.year,
  m.tags_genre,
  mi.extra_data,
  m.guid,
  m.summary,
  m.audience_rating,
  m.rating
FROM metadata_items m
LEFT JOIN media_items mi ON mi.metadata_item_id = m.id
WHERE m.metadata_type = 1
  AND m.user_thumb_url IS NOT NULL
  AND m.user_thumb_url <> ''
ORDER BY COALESCE(mi.begins_at, m.added_at) ASC
LIMIT ?1
"#;

const SQL_POSTERS_THUMB: &str = r#"
SELECT
  m.title,
  m.thumb_url,
  mi.begins_at,
  m.year,
  m.tags_genre,
  mi.extra_data,
  m.guid,
  m.summary,
  m.audience_rating,
  m.rating
FROM metadata_items m
LEFT JOIN media_items mi ON mi.metadata_item_id = m.id
WHERE m.metadata_type = 1
  AND m.thumb_url IS NOT NULL
  AND m.thumb_url <> ''
ORDER BY COALESCE(mi.begins_at, m.added_at) ASC
LIMIT ?1
"#;

// ---- helpers only used in this module ----
fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_row| Ok(()),
    )
    .is_ok()
}

#[derive(Default, Clone, Debug)]
struct ChannelMeta {
    call_sign: Option<String>,
    title: Option<String>,
    thumb: Option<String>,
}

/// Extract channel metadata from `media_items.extra_data`
fn parse_channel_meta(extra: &str) -> ChannelMeta {
    fn find_val(hay: &str, key: &str) -> Option<String> {
        let needle = format!("\"{}\":\"", key);
        let start = hay.find(&needle)? + needle.len();
        let rest = &hay[start..];
        let end = rest.find('"')?;
        let val = &rest[..end];
        if val.is_empty() {
            None
        } else {
            Some(val.to_string())
        }
    }
    let call_sign = find_val(extra, "at:channelCallSign");
    let title = find_val(extra, "at:channelTitle");
    let thumb = find_val(extra, "at:channelThumb");

    ChannelMeta {
        call_sign,
        title,
        thumb,
    }
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

#[derive(PartialEq, Eq)]
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

fn copy_contents(src: &Path, dst: &Path) -> io::Result<()> {
    // Keep the staging files writable when the NAS files are read-only.
    io::copy(&mut fs::File::open(src)?, &mut fs::File::create(dst)?)?;
    Ok(())
}

fn copy_sqlite_snapshot(src: &Path, dst: &Path) -> io::Result<()> {
    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    if dst.exists() && fs::canonicalize(src)? == fs::canonicalize(dst)? {
        return Err(io::Error::other(
            "source and destination refer to the same database",
        ));
    }
    let scratch = tempfile::tempdir_in(parent)?;
    let staged = tempfile::NamedTempFile::new_in(scratch.path())?.into_temp_path();
    let staged_path: &Path = staged.as_ref();
    let before = source_state(src)?;
    copy_contents(src, staged_path)?;
    if before.1.is_some() {
        copy_contents(
            &sqlite_sidecar_path(src, "wal"),
            &sqlite_sidecar_path(staged_path, "wal"),
        )?;
    }
    if before != source_state(src)? {
        return Err(io::Error::other(
            "source changed during copy; keeping previous snapshot",
        ));
    }
    // Rebuild SHM locally, validate the guide/library schema and let SQLite replay
    // committed WAL frames. Never copy a live SHM file or touch SQLite on the NAS.
    let db = Connection::open(staged_path).map_err(io::Error::other)?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata_items), EXISTS(SELECT 1 FROM media_items)",
        [],
        |_| Ok(()),
    )
    .map_err(io::Error::other)?;
    db.pragma_update(None, "journal_mode", "DELETE")
        .map_err(io::Error::other)?;
    if dst.exists()
        || ["wal", "shm", "journal"]
            .iter()
            .any(|suffix| sqlite_sidecar_path(dst, suffix).exists())
    {
        // Restore transactionally into legacy mirrors, including existing local WALs.
        // An error rolls back without exposing mismatched generations of DB and WAL.
        db.backup(rusqlite::DatabaseName::Main, dst, None)
            .map_err(io::Error::other)?;
    } else {
        drop(db);
        staged.persist_noclobber(dst).map_err(|err| err.error)?;
    }
    Ok(())
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

pub(crate) fn sync_library_db_from_source(force: bool) -> Result<bool, String> {
    let Some(src) = load_config().plex_library_db_source else {
        return Ok(false);
    };
    sync_mirror(&src, &local_library_db_path(), force)
        .map_err(|e| format!("Copying Plex library from {} failed: {e}", src.display()))
}

// Set to true if you want to synthesize a tiny fake list for debugging.
const DIAG_FAKE_STARTUP: bool = false;

/// Spawn the background thread that prepares the poster list (no downloads here).
pub(crate) fn spawn_poster_prep(tx: Sender<PrepMsg>) {
    std::thread::spawn(move || {
        let send = |m: PrepMsg| {
            let _ = tx.send(m);
        };

        if DIAG_FAKE_STARTUP {
            send(PrepMsg::Info(
                "DIAG: synthesizing small poster list…".into(),
            ));
            let fake: Vec<PrepItem> = vec![
                PrepItem {
                    title: "Blade Runner".into(),
                    thumb_url: "https://example.com/a.jpg".into(),
                    key: url_to_cache_key("https://example.com/a.jpg"),
                    begins_at: None,
                    year: Some(1982),
                    tags_genre: Some("Sci-Fi|Thriller".into()),
                    channel_call_sign: Some("ITV2".into()),
                    channel_title: Some("006 ITV2".into()),
                    channel_thumb: Some("https://example.com/channel_itv2.png".into()),
                    guid: Some("com.plexapp.agents.imdb://tt0083658".into()),
                    summary: Some("In the future, blade runners hunt replicants.".into()),
                    audience_rating: Some(8.5),
                    critic_rating: Some(8.9),
                },
                PrepItem {
                    title: "Alien".into(),
                    thumb_url: "https://example.com/b.jpg".into(),
                    key: url_to_cache_key("https://example.com/b.jpg"),
                    begins_at: None,
                    year: Some(1979),
                    tags_genre: Some("Sci-Fi|Horror".into()),
                    channel_call_sign: Some("ITV2".into()),
                    channel_title: Some("006 ITV2".into()),
                    channel_thumb: Some("https://example.com/channel_itv2.png".into()),
                    guid: Some("com.plexapp.agents.imdb://tt0078748".into()),
                    summary: Some("The crew of the Nostromo encounters a deadly alien.".into()),
                    audience_rating: Some(8.4),
                    critic_rating: Some(9.0),
                },
                PrepItem {
                    title: "Arrival".into(),
                    thumb_url: "https://example.com/c.jpg".into(),
                    key: url_to_cache_key("https://example.com/c.jpg"),
                    begins_at: None,
                    year: Some(2016),
                    tags_genre: Some("Sci-Fi|Drama".into()),
                    channel_call_sign: Some("ITV2".into()),
                    channel_title: Some("006 ITV2".into()),
                    channel_thumb: Some("https://example.com/channel_itv2.png".into()),
                    guid: Some("com.plexapp.agents.imdb://tt2543164".into()),
                    summary: Some("A linguist communicates with extraterrestrial visitors.".into()),
                    audience_rating: Some(8.0),
                    critic_rating: Some(8.4),
                },
            ];
            send(PrepMsg::Done(fake));
            return;
        }

        // Resolve DB paths from config
        let cfg = load_config();
        let db_path = local_db_path();
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

        // Quick counts to see if our WHERE will match anything
        let cnt_total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_items WHERE metadata_type=1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let cnt_with_thumb: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_items \
                 WHERE metadata_type=1 AND (COALESCE(user_thumb_url,'')<>'' OR COALESCE(thumb_url,'')<>'')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        info!("prep: movies total={cnt_total}, with_thumb={cnt_with_thumb}");
        send(PrepMsg::Info(format!(
            "Movies in DB: total {cnt_total}, with thumbs {cnt_with_thumb}"
        )));

        // SQL with fallback (includes mi.extra_data for channel extraction)
        let mut st = match conn.prepare(SQL_POSTERS_USER_THUMB) {
            Ok(s) => s,
            Err(e1) => {
                if e1.to_string().contains("user_thumb_url") {
                    match conn.prepare(SQL_POSTERS_THUMB) {
                        Ok(s) => s,
                        Err(e2) => {
                            send(PrepMsg::Error(format!(
                                "prepare failed: {e1} / fallback: {e2}"
                            )));
                            return;
                        }
                    }
                } else {
                    send(PrepMsg::Error(format!("prepare failed: {e1}")));
                    return;
                }
            }
        };

        // Harvest list — NO network here
        send(PrepMsg::Info(
            "Stage 2/4 - Parsing Plex guide data (collecting posters and metadata for the grid)."
                .into(),
        ));
        let mut q = match st.query([1_000_000_i64]) {
            Ok(r) => r,
            Err(e) => {
                send(PrepMsg::Error(format!("query failed: {e}")));
                return;
            }
        };

        let mut list: Vec<PrepItem> = Vec::new();
        let mut last_emit = Instant::now();

        loop {
            let row = match q.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(err) => {
                    send(PrepMsg::Error(format!("Reading guide rows failed: {err}")));
                    return;
                }
            };
            let title: Option<String> = row.get(0).ok().flatten();
            let url: Option<String> = row.get(1).ok().flatten();
            let begins: Option<i64> = row.get(2).ok().flatten();
            let year: Option<i32> = row.get(3).ok().flatten();
            let tags: Option<String> = row.get(4).ok().flatten();
            let extra: Option<String> = row.get(5).ok().flatten();
            let guid: Option<String> = row.get(6).ok().flatten();
            let summary: Option<String> = row.get(7).ok().flatten();
            let audience_rating: Option<f32> = row
                .get::<_, Option<f64>>(8)
                .ok()
                .flatten()
                .map(|v| v as f32);
            let critic_rating: Option<f32> = row
                .get::<_, Option<f64>>(9)
                .ok()
                .flatten()
                .map(|v| v as f32);

            if let (Some(t), Some(u)) = (title, url) {
                let tt = t.trim();
                if !tt.is_empty() && (u.starts_with("http://") || u.starts_with("https://")) {
                    let key = url_to_cache_key(&u);
                    let channel_meta = extra.as_deref().map(parse_channel_meta).unwrap_or_default();

                    list.push(crate::app::types::PrepItem {
                        title: tt.to_owned(),
                        thumb_url: u,
                        key,
                        begins_at: begins,
                        year,
                        tags_genre: tags,
                        channel_call_sign: channel_meta.call_sign,
                        channel_title: channel_meta.title,
                        channel_thumb: channel_meta.thumb,
                        guid,
                        summary,
                        audience_rating,
                        critic_rating,
                    });
                    if last_emit.elapsed() >= Duration::from_millis(600) {
                        send(PrepMsg::Info(format!("Stage 2/4 - Parsing Plex guide data ({} posters discovered so far; powers the main grid).", list.len())));
                        last_emit = Instant::now();
                    }
                }
            }
        }

        // Dedupe by title (stable)
        let mut seen = std::collections::HashSet::new();
        list.retain(|item| seen.insert(item.title.to_ascii_lowercase()));

        info!("prep: final poster rows after dedupe = {}", list.len());
        if list.is_empty() {
            warn!("prep: no posters found — likely DB path/columns mismatch");
            send(PrepMsg::Info(format!(
                "No posters found — check DB path/type in {}",
                crate::config::CONFIG_FILENAME
            )));
        }

        send(PrepMsg::Done(list));
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
        self.last_item_msg.clear();

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
                                let airing = item.begins_at.map(|ts| {
                                    std::time::SystemTime::UNIX_EPOCH
                                        + std::time::Duration::from_secs(ts as u64)
                                });

                                let channel_raw = item
                                    .channel_call_sign
                                    .clone()
                                    .or_else(|| crate::app::utils::host_from_url(&item.thumb_url));

                                let channel_title_original =
                                    item.channel_title.clone().filter(|s| !s.trim().is_empty());

                                let normalized_title = channel_title_original
                                    .as_ref()
                                    .map(|s| crate::app::utils::humanize_channel(s));

                                let channel_display =
                                    normalized_title.as_ref().cloned().or_else(|| {
                                        channel_raw
                                            .as_ref()
                                            .map(|c| crate::app::utils::humanize_channel(c))
                                    });

                                let small_k = Self::small_key(&item.key);
                                let path = crate::app::cache::find_any_by_key(&small_k);
                                let state = if path.is_some() {
                                    crate::app::PosterState::Cached
                                } else {
                                    crate::app::PosterState::Pending
                                };
                                let genres = item
                                    .tags_genre
                                    .as_deref()
                                    .map(crate::app::utils::parse_genres)
                                    .unwrap_or_default();
                                let tags_joined = (!genres.is_empty()).then(|| genres.join("|"));
                                let broadcast_hd = crate::app::utils::infer_broadcast_hd(
                                    tags_joined.as_deref(),
                                    channel_display.as_deref(),
                                );
                                let summary = item.summary.and_then(|s| {
                                    let trimmed = s.trim();
                                    if trimmed.is_empty() {
                                        None
                                    } else {
                                        Some(trimmed.to_string())
                                    }
                                });

                                crate::app::PosterRow {
                                    title: item.title,
                                    url: item.thumb_url,
                                    key: small_k,
                                    airing,
                                    year: item.year,
                                    channel: channel_display,
                                    channel_raw,
                                    channel_title: channel_title_original,
                                    channel_thumb: item.channel_thumb,
                                    genres,
                                    guid: item.guid,
                                    summary,
                                    audience_rating: item.audience_rating,
                                    critic_rating: item.critic_rating,
                                    path,
                                    tex: None,
                                    state,
                                    owned: false, // filled in by apply_owned_flags()
                                    owned_modified: None,
                                    owned_hd: false,
                                    broadcast_hd,
                                    scheduled: false,
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
                            Self::spawn_channel_icon_prefetch(icon_urls);
                        }

                        // Warm-start: upload last hotset first (bounded)
                        if let Some(hs) = self.last_hotset.take() {
                            for row in &mut self.rows {
                                if let Some(p) = hs.get(&row.key) {
                                    if p.exists() {
                                        row.path = Some(p.clone());
                                        row.state = crate::app::PosterState::Cached;
                                    }
                                }
                            }
                            let mut uploaded = 0usize;
                            for i in 0..self.rows.len() {
                                if uploaded >= crate::app::PREWARM_UPLOADS {
                                    break;
                                }
                                let should_upload = self
                                    .rows
                                    .get(i)
                                    .is_some_and(|row| hs.contains_key(&row.key));
                                if should_upload && self.try_lazy_upload_row(ctx, i) {
                                    uploaded += 1;
                                }
                            }
                        }

                        // Scheduled recordings (from Plex library DB)
                        self.apply_scheduled_flags();

                        // Owned flags (if ready)
                        self.apply_owned_flags();
                        self.set_startup_progress_floor(crate::app::STARTUP_STAGE2_DONE);
                        let poster_done_status =
                            format!("Poster prep complete. {} items ready.", self.rows.len());
                        if self.owned_keys.is_some() || !self.owned_scan_in_progress {
                            self.boot_phase = crate::app::BootPhase::Ready;
                            self.set_status(poster_done_status);
                        } else {
                            self.boot_phase = crate::app::BootPhase::Caching;
                            self.set_status("Poster prep complete. Scanning owned library...");
                        }

                        self.start_prefetch(
                            ctx,
                            std::env::var_os("PEX_DISABLE_PREFETCH").is_some(),
                        );
                        self.prewarm_first_screen(ctx);

                        keep = None;
                        seen_any = true;
                    }
                    Ok(crate::app::PrepMsg::Error(e)) => {
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
mod mirror_tests {
    use super::*;

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
