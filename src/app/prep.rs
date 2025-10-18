// src/app/prep.rs
use std::io::{Read, Write};
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
        call_sign: call_sign.clone(),
        title,
        thumb,
    }
}

const MIN_COPY_INTERVAL_HOURS: u64 = 24;

fn last_sync_marker_path(local_db: &Path) -> PathBuf {
    PathBuf::from(format!("{}.last_sync", local_db.display()))
}

fn fresh_enough(marker_path: &Path) -> io::Result<bool> {
    fs::metadata(marker_path).map_or(Ok(false), |meta| {
        let m = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now().duration_since(m).unwrap_or_default();
        Ok(age.as_secs() < MIN_COPY_INTERVAL_HOURS * 3600)
    })
}

fn touch_last_sync(marker_path: &Path) -> io::Result<()> {
    fs::write(marker_path, b"ok")
}

fn needs_db_update_daily(src: &Path, dst: &Path) -> io::Result<bool> {
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

fn copy_with_progress<F>(src: &Path, dst: &Path, mut on_prog: F) -> io::Result<()>
where
    F: FnMut(u64, u64, f32, f64),
{
    let mut in_f = fs::File::open(src)?;
    let total = in_f.metadata()?.len();

    let tmp_path = PathBuf::from(format!("{}.tmp", dst.display()));
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
        if let Some(src_path) = cfg.plex_epg_db_source.as_deref() {
            let src = Path::new(src_path);
            match needs_db_update_daily(src, &db_path) {
                Ok(true) => {
                    send(PrepMsg::Info("Stage 2/4 – Copying Plex DB from source (enables offline start-ups). First run may take a while.".into()));
                    let marker = last_sync_marker_path(&db_path);
                    let _ = copy_with_progress(src, &db_path, |_c, _t, _p, _mbps| {});
                    let _ = touch_last_sync(&marker);
                }
                Ok(false) => send(PrepMsg::Info(
                    "Stage 2/4 – Local Plex DB already fresh; skipping copy.".into(),
                )),
                Err(e) => send(PrepMsg::Info(format!(
                    "Stage 2/4 – Freshness check failed (continuing anyway): {e}"
                ))),
            }
        } else {
            send(PrepMsg::Info(
                "Stage 2/4 – Using existing local EPG DB (no source copy configured).".into(),
            ));
        }

        // Optional daily copy for the Plex library database
        let library_db_path = local_library_db_path();
        if let Some(src_path) = cfg.plex_library_db_source.as_deref() {
            let src = Path::new(src_path);
            match needs_db_update_daily(src, &library_db_path) {
                Ok(true) => {
                    send(PrepMsg::Info(
                        "Stage 2/4 – Copying Plex library DB from plex_library_db_source.".into(),
                    ));
                    info!(
                        "prep: copying Plex library DB from {} to {}",
                        src.display(),
                        library_db_path.display()
                    );
                    let marker = last_sync_marker_path(&library_db_path);
                    match copy_with_progress(src, &library_db_path, |_c, _t, _p, _mbps| {}) {
                        Ok(_) => {
                            let _ = touch_last_sync(&marker);
                            send(PrepMsg::Info(
                                "Stage 2/4 – Plex library DB copy complete.".into(),
                            ));
                        }
                        Err(err) => {
                            warn!(
                                "Copying Plex library DB failed (continuing with existing copy if any): {err}"
                            );
                            send(PrepMsg::Info(format!(
                                "Stage 2/4 – Copying Plex library DB failed: {err}"
                            )));
                        }
                    }
                }
                Ok(false) => send(PrepMsg::Info(
                    "Stage 2/4 – Plex library DB already fresh; skipping copy.".into(),
                )),
                Err(e) => send(PrepMsg::Info(format!(
                    "Stage 2/4 – Plex library DB freshness check failed (continuing anyway): {e}"
                ))),
            }
        } else {
            send(PrepMsg::Info(
                "Stage 2/4 – plex_library_db_source not set; skipping Plex library DB copy.".into(),
            ));
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

        while let Ok(Some(row)) = q.next() {
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
            send(PrepMsg::Info(
                "No posters found — check DB path/type in config.json".into(),
            ));
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

                                let channel_display = normalized_title.clone().or_else(|| {
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
                                let owned_key =
                                    crate::app::PexApp::make_owned_key(&item.title, item.year);
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
                                    owned_key,
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
                        self.refresh_scheduled_index();

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
