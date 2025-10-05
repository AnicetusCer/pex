// src/app/prep.rs
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io};

use rusqlite::{Connection, OpenFlags};

use crate::app::cache::url_to_cache_key;
use crate::app::{PrepItem, PrepMsg}; // <- use the re-export from app::types
use crate::config::load_config;
use eframe::egui as eg; // <- gives us eg::Context

// --- local SQL (newer plex uses user_thumb_url; older uses thumb_url) ---
const SQL_POSTERS_USER_THUMB: &str = r#"
SELECT
  m.title,
  m.user_thumb_url,
  mi.begins_at,
  m.year,
  m.tags_genre,
  mi.extra_data
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
  mi.extra_data
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

/// Extract channel from `media_items.extra_data`
fn parse_channel_from_extra(extra: &str) -> Option<String> {
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
    find_val(extra, "at:channelCallSign")
        .or_else(|| find_val(extra, "at:channelTitle"))
        .map(|s| {
            if let Some((_, right)) = s.split_once(' ') {
                right.to_string()
            } else {
                s
            }
        })
}

const MIN_COPY_INTERVAL_HOURS: u64 = 24;

fn last_sync_marker_path(local_db: &str) -> String {
    format!("{}.last_sync", local_db)
}

fn fresh_enough(marker_path: &str) -> io::Result<bool> {
    fs::metadata(marker_path).map_or(Ok(false), |meta| {
        let m = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now().duration_since(m).unwrap_or_default();
        Ok(age.as_secs() < MIN_COPY_INTERVAL_HOURS * 3600)
    })
}

fn touch_last_sync(marker_path: &str) -> io::Result<()> {
    fs::write(marker_path, b"ok")
}

fn needs_db_update_daily(src: &str, dst: &str) -> io::Result<bool> {
    if fresh_enough(&last_sync_marker_path(dst))? {
        return Ok(false);
    }

    let src_meta = fs::metadata(src)
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("src meta: {e}")))?;
    let dst_meta = match fs::metadata(dst) {
        Ok(m) => m,
        Err(_) => return Ok(true), // no local db yet
    };

    if src_meta.len() != dst_meta.len() {
        return Ok(true);
    }
    let src_m = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let dst_m = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(src_m > dst_m)
}

fn copy_with_progress<F>(src: &str, dst: &str, mut on_prog: F) -> io::Result<()>
where
    F: FnMut(u64, u64, f32, f64),
{
    let mut in_f = fs::File::open(src)?;
    let total = in_f.metadata()?.len();

    let tmp_path = format!("{}.tmp", dst);
    let mut out_f = fs::File::create(&tmp_path)?;

    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut copied: u64 = 0;
    let started = Instant::now();
    let mut last_emit = Instant::now();

    loop {
        let n = in_f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out_f.write_all(&buf[..n])?;
        copied += n as u64;

        std::thread::yield_now();
        if copied.is_multiple_of(64 * 1024 * 1024) {
            std::thread::sleep(Duration::from_millis(1));
        }

        if last_emit.elapsed() >= Duration::from_millis(150) {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let mbps = (copied as f64 / (1024.0 * 1024.0)) / secs;
            let pct = if total > 0 {
                copied as f32 / total as f32
            } else {
                1.0
            };
            on_prog(copied, total, pct, mbps);
            last_emit = Instant::now();
        }
    }

    let secs = started.elapsed().as_secs_f64().max(0.001);
    let mbps = (copied as f64 / (1024.0 * 1024.0)) / secs;
    on_prog(copied, total, 1.0, mbps);

    out_f.flush()?;
    drop(out_f);
    fs::rename(&tmp_path, dst)?;
    Ok(())
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
                (
                    "Blade Runner".into(),
                    "https://example.com/a.jpg".into(),
                    url_to_cache_key("https://example.com/a.jpg"),
                    None,
                    Some(1982),
                    Some("Sci-Fi|Thriller".into()),
                    Some("ITV2".into()),
                ),
                (
                    "Alien".into(),
                    "https://example.com/b.jpg".into(),
                    url_to_cache_key("https://example.com/b.jpg"),
                    None,
                    Some(1979),
                    Some("Sci-Fi|Horror".into()),
                    Some("ITV2".into()),
                ),
                (
                    "Arrival".into(),
                    "https://example.com/c.jpg".into(),
                    url_to_cache_key("https://example.com/c.jpg"),
                    None,
                    Some(2016),
                    Some("Sci-Fi|Drama".into()),
                    Some("ITV2".into()),
                ),
            ];
            send(PrepMsg::Done(fake));
            return;
        }

        // Resolve DB paths from config
        let cfg = load_config();
        let db_path = match cfg.plex_db_local.clone() {
            Some(p) => p,
            None => {
                send(PrepMsg::Error("No plex_db_local set in config.json".into()));
                return;
            }
        };

        // Optional daily copy from source to local
        if let Some(src_path) = cfg.plex_db_source.as_deref() {
            match needs_db_update_daily(src_path, &db_path) {
                Ok(true) => {
                    send(PrepMsg::Info("Updating local EPG DB…".into()));
                    let marker = last_sync_marker_path(&db_path);
                    let _ = copy_with_progress(src_path, &db_path, |_c, _t, _p, _mbps| {});
                    let _ = touch_last_sync(&marker);
                }
                Ok(false) => send(PrepMsg::Info(
                    "Local EPG DB fresh — skipping update.".into(),
                )),
                Err(e) => send(PrepMsg::Info(format!(
                    "Freshness check failed (continuing): {e}"
                ))),
            }
        } else {
            send(PrepMsg::Info("Using existing local EPG DB.".into()));
        }

        // Open DB read-only
        let conn = match Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(e) => {
                send(PrepMsg::Error(format!("open db failed: {e}")));
                return;
            }
        };
        let _ = conn.busy_timeout(Duration::from_secs(10));
        let _ = conn.pragma_update(None, "temp_store", "MEMORY");

        if !(table_exists(&conn, "metadata_items") && table_exists(&conn, "media_items")) {
            send(PrepMsg::Error(
                "required tables missing (metadata_items, media_items)".into(),
            ));
            return;
        }

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
        send(PrepMsg::Info("Scanning EPG…".into()));
        let mut q = match st.query([i64::MAX]) {
            Ok(r) => r,
            Err(e) => {
                send(PrepMsg::Error(format!("query failed: {e}")));
                return;
            }
        };

        let mut list: Vec<PrepItem> = Vec::new();
        let mut last_emit = Instant::now();

        while let Ok(Some(row)) = q.next() {
            let title: Option<String> = row.get(0).ok().flatten();
            let url: Option<String> = row.get(1).ok().flatten();
            let begins: Option<i64> = row.get(2).ok().flatten();
            let year: Option<i32> = row.get(3).ok().flatten();
            let tags: Option<String> = row.get(4).ok().flatten();
            let extra: Option<String> = row.get(5).ok().flatten();

            if let (Some(t), Some(u)) = (title, url) {
                let tt = t.trim();
                if !tt.is_empty() && (u.starts_with("http://") || u.starts_with("https://")) {
                    let key = url_to_cache_key(&u);
                    let ch = extra.as_deref().and_then(parse_channel_from_extra);
                    list.push((tt.to_owned(), u, key, begins, year, tags, ch));
                    if last_emit.elapsed() >= Duration::from_millis(600) {
                        send(PrepMsg::Info(format!("Found {} posters…", list.len())));
                        last_emit = Instant::now();
                    }
                }
            }
        }

        // Dedupe by title (stable)
        let mut seen = std::collections::HashSet::new();
        list.retain(|(t, ..)| seen.insert(t.to_ascii_lowercase()));

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
        self.set_status("Checking for new posters…");
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
                        self.rows = list
                            .into_iter()
                            .map(|(t, u, base_k, ts_opt, year_opt, tags_opt, ch_opt)| {
                                let airing = ts_opt.map(|ts| {
                                    std::time::SystemTime::UNIX_EPOCH
                                        + std::time::Duration::from_secs(ts as u64)
                                });
                                let channel_raw =
                                    ch_opt.or_else(|| crate::app::utils::host_from_url(&u));
                                let channel =
                                    channel_raw.map(|c| crate::app::utils::humanize_channel(&c));

                                let small_k = Self::small_key(&base_k);
                                let path = crate::app::cache::find_any_by_key(&small_k);
                                let state = if path.is_some() {
                                    crate::app::PosterState::Cached
                                } else {
                                    crate::app::PosterState::Pending
                                };
                                let genres = tags_opt
                                    .as_deref()
                                    .map(crate::app::utils::parse_genres)
                                    .unwrap_or_default();

                                crate::app::PosterRow {
                                    title: t,
                                    url: u,
                                    key: small_k,
                                    airing,
                                    year: year_opt,
                                    channel,
                                    genres,
                                    path,
                                    tex: None,
                                    state,
                                    owned: false, // filled in by apply_owned_flags()
                                }
                            })
                            .collect();

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

                        // Owned flags (if ready)
                        self.apply_owned_flags();
                        let poster_done_status =
                            format!("Poster prep complete. {} items ready.", self.rows.len());
                        if self.owned_keys.is_some() {
                            self.boot_phase = crate::app::BootPhase::Ready;
                            self.set_status(poster_done_status);
                        } else {
                            self.boot_phase = crate::app::BootPhase::Caching;
                            self.set_status("Poster prep complete. Scanning owned library...");
                        }

                        self.start_prefetch(ctx);
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
