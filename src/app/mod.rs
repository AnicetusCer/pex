// src/app/mod.rs — async DB scan + upfront poster prefetch + resized cache + single splash

// ---- Standard lib imports ----
use std::{fs, io};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};
use rusqlite::{Connection, OpenFlags};

// ---- Crates ----
use eframe::egui::{self as eg, ColorImage, TextureHandle};

// ---- Local modules ----
pub mod cache;
use crate::app::cache::{
    cache_dir, download_and_store, download_and_store_resized_with_client, find_any_by_key,
    load_rgba_raw_or_image, url_to_cache_key,
};
use crate::config::load_config;

// ---- Tunables ----
const WORKER_COUNT: usize = 16;        // up from 8 — tune freely (8–32 typical)
const RESIZE_MAX_W: u32 = 320;
const RESIZE_QUALITY: u8 = 75;
const SHOW_GRID_EARLY: bool = true;
const MIN_READY_BEFORE_GRID: usize = 24;
const STATUS_EMIT_EVERY_MS: u64 = 120;
const DIAG_FAKE_STARTUP: bool = false;
const MAX_DONE_PER_FRAME: usize = 12;
const MAX_UPLOADS_PER_FRAME: usize = 4;
const PREWARM_UPLOADS: usize = 24;

// Two SQL variants (newer plex uses user_thumb_url; older uses thumb_url)
const SQL_POSTERS_USER_THUMB: &str = r#"
SELECT
  m.title,
  m.user_thumb_url,
  mi.begins_at,
  m.year,
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
  mi.extra_data
FROM metadata_items m
LEFT JOIN media_items mi ON mi.metadata_item_id = m.id
WHERE m.metadata_type = 1
  AND m.thumb_url IS NOT NULL
  AND m.thumb_url <> ''
ORDER BY COALESCE(mi.begins_at, m.added_at) ASC
LIMIT ?1
"#;

// ---- DB helpers ----
fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_row| Ok(()),
    )
    .is_ok()
}

/// Extract channel string from `media_items.extra_data` JSON-ish blob.
/// Prefers `at:channelCallSign`, falls back to `at:channelTitle`.
fn parse_channel_from_extra(extra: &str) -> Option<String> {
    // very light substring extraction to avoid pulling in serde_json
    fn find_val(hay: &str, key: &str) -> Option<String> {
        let needle = format!("\"{}\":\"", key);
        let start = hay.find(&needle)? + needle.len();
        let rest = &hay[start..];
        let end = rest.find('"')?;
        let val = &rest[..end];
        if val.is_empty() { None } else { Some(val.to_string()) }
    }
    find_val(extra, "at:channelCallSign")
        .or_else(|| find_val(extra, "at:channelTitle"))
        .map(|s| {
            // compact "006 ITV2" → "ITV2"
            if let Some((_, right)) = s.split_once(' ') { right.to_string() } else { s }
        })
}

/// How often we allow a fresh copy from source
const MIN_COPY_INTERVAL_HOURS: u64 = 24;

/// `.last_sync` marker file path for a given local DB path
fn last_sync_marker_path(local_db: &str) -> String {
    format!("{}.last_sync", local_db)
}

/// Is the last sync younger than MIN_COPY_INTERVAL_HOURS?
fn fresh_enough(marker_path: &str) -> io::Result<bool> {
    match fs::metadata(marker_path) {
        Ok(meta) => {
            let m = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let age = SystemTime::now().duration_since(m).unwrap_or_default();
            Ok(age.as_secs() < MIN_COPY_INTERVAL_HOURS * 3600)
        }
        Err(_) => Ok(false),
    }
}

/// Update the `.last_sync` marker (touch)
fn touch_last_sync(marker_path: &str) -> io::Result<()> {
    fs::write(marker_path, b"ok")
}

/// Decide if we should copy today; if marker is fresh (<24h) skip immediately.
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

/// Big-buffer copy with progress callback. Writes to `dst.tmp` then renames.
fn copy_with_progress<F>(src: &str, dst: &str, mut on_prog: F) -> io::Result<()>
where
    F: FnMut(u64, u64, f32, f64), // bytes_copied, total, pct, mbps
{
    let mut in_f = fs::File::open(src)?;
    let total = in_f.metadata()?.len();

    let tmp_path = format!("{}.tmp", dst);
    let mut out_f = fs::File::create(&tmp_path)?;

    // 8 MiB chunks (keeps UI responsive on network shares)
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
        if copied % (64 * 1024 * 1024) == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }

        if last_emit.elapsed() >= Duration::from_millis(150) {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let mbps = (copied as f64 / (1024.0 * 1024.0)) / secs;
            let pct = if total > 0 { copied as f32 / total as f32 } else { 1.0 };
            on_prog(copied, total, pct, mbps);
            last_emit = Instant::now();
        }
    }

    // Ensure progress hits 100%
    {
        let secs = started.elapsed().as_secs_f64().max(0.001);
        let mbps = (copied as f64 / (1024.0 * 1024.0)) / secs;
        on_prog(copied, total, 1.0, mbps);
    }

    out_f.flush()?;
    drop(out_f);
    fs::rename(&tmp_path, dst)?;
    Ok(())
}

#[derive(Debug)]
enum PrepMsg {
    Info(String),
    // (title, url, key, begins_at_opt, year_opt, channel_opt)
    Done(Vec<(String, String, String, Option<i64>, Option<i32>, Option<String>)>),
    Error(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prefetching,
    Ready,
}
// ---------- Data model tied 1:1 to a grid card ----------
#[derive(Clone, Copy, PartialEq, Eq)]
enum BootPhase {
    Starting,
    CheckingNew,  // phase 2
    Caching,      // phase 3
    Ready,        // phase 4
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PosterState {
    Pending, // queued or downloading
    Cached,  // file present on disk (ready to upload)
    Ready,   // texture uploaded
    Failed,  // permanent failure
}

// Day-range selector for fast startup
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DayRange {
    Two,
    Seven,
    Fourteen,
    All,
}

struct PosterRow {
    title: String,
    url: String,
    key: String,
    airing: Option<SystemTime>,
    year: Option<i32>,
    channel: Option<String>,      // quick indicator from hostname for now
    path: Option<PathBuf>,
    tex: Option<TextureHandle>,
    state: PosterState,
}

impl PosterRow {
    fn new(title: String, url: String, airing: Option<SystemTime>) -> Self {
        let key = url_to_cache_key(&url);
        Self {
            title,
            url,
            key,
            airing,
            year: None,
            channel: None,
            path: None,
            tex: None,
            state: PosterState::Pending,
        }
    }
}

struct PrefetchDone {
    row_idx: usize,
    result: Result<PathBuf, String>,
}


pub struct PexApp {
    // data
    rows: Vec<PosterRow>,

    // range
    current_range: DayRange,

    // splash state
    loading_progress: f32,
    loading_message: String,
    last_item_msg: String,

    // poster prep warm-up
    boot_phase: BootPhase,
    prep_rx: Option<Receiver<PrepMsg>>,
    prep_started: bool,

    // splash heartbeat (keeps UI visibly alive)
    heartbeat_last: Instant,
    heartbeat_dots: u8,
    status_last_emit: Instant,

    // phase visibility
    phase: Phase,
    phase_started: Instant,
    first_ready_at: Option<Instant>,

    // one-time init guard
    did_init: bool,

    // prefetch plumbing
    prefetch_started: bool,
    total_targets: usize,
    completed: usize,
    failed: usize,

    work_tx: Option<Sender<(usize, String, String, Option<PathBuf>)>>, // (row_idx, key, url, cached_path)
    done_rx: Option<Receiver<PrefetchDone>>,
}

impl Default for PexApp {
    fn default() -> Self {
        Self {
            rows: Vec::new(),

            current_range: DayRange::Two,

            loading_progress: 0.0,
            loading_message: String::new(),
            last_item_msg: String::new(),

            heartbeat_last: Instant::now(),
            heartbeat_dots: 0,
            status_last_emit: Instant::now(),

            phase: Phase::Prefetching, // will be set properly later
            phase_started: Instant::now(),
            first_ready_at: None,

            did_init: false,

            boot_phase: BootPhase::Starting,
            prep_rx: None,
            prep_started: false,

            prefetch_started: false,
            total_targets: 0,
            completed: 0,
            failed: 0,

            work_tx: None,
            done_rx: None,
        }
    }
}

// ---------- methods ----------
impl PexApp {
    // ----- tiny helpers ----

/// Derive a “small” variant cache key from the base key (separate file entry).
fn small_key(base: &str) -> String {
    format!("{base}__s")
}


fn day_bucket(ts: SystemTime) -> i64 {
    let secs = ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    secs / 86_400
}

fn weekday_full_from_bucket(bucket: i64) -> &'static str {
    // 1970-01-01 was Thursday
    let idx = ((bucket + 4).rem_euclid(7)) as usize;
    const NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    NAMES[idx]
}

fn month_short_name(m: u32) -> &'static str {
    const M: [&str; 12] = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    M[(m.saturating_sub(1)).min(11) as usize]
}

/// Ordinal suffix for English (1st, 2nd, 3rd, 4th, …)
fn ordinal_suffix(d: u32) -> &'static str {
    if (11..=13).contains(&(d % 100)) { return "th"; }
    match d % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

/// Convert days since 1970-01-01 (our bucket) into (year, month, day).
/// Algorithm: Howard Hinnant's civil_from_days.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = (z >= 0).then(|| z).unwrap_or(z - 146096) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + (m <= 2) as i64;
    (y as i32, m as u32, d as u32)
}

/// Format divider label like "Friday 3rd Sep" from a day bucket.
fn format_day_label(bucket: i64) -> String {
    let (_y, m, d) = Self::civil_from_days(bucket);
    let wd = Self::weekday_full_from_bucket(bucket);
    format!("{} {}{} {}", wd, d, Self::ordinal_suffix(d), Self::month_short_name(m))
}

/// HH:MM (UTC) from airing SystemTime
fn hhmm_utc(ts: SystemTime) -> String {
    let secs = ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let hm = (secs % 86_400 + 86_400) % 86_400; // handle negatives just in case
    let h = hm / 3600;
    let m = (hm % 3600) / 60;
    format!("{:02}:{:02}", h, m)
}

/// Very light hostname extraction for channel hint (no extra deps).
fn host_from_url(u: &str) -> Option<String> {
    // find "://"
    let start = u.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &u[start..];
    let end = rest.find('/').unwrap_or(rest.len());
    if end == 0 { return None; }
    let host = &rest[..end];
    if host.is_empty() { return None; }
    // compress to something short: first label uppercased, or the host itself
    let ch = host.split('.').next().unwrap_or(host).to_uppercase();
    Some(ch)
}

/// Try to upload texture for a single row if a cached file exists (small variant).
/// Returns true if a texture was uploaded this call.
fn try_lazy_upload_row(&mut self, ctx: &eg::Context, idx: usize) -> bool {
    if let Some(row) = self.rows.get_mut(idx) {
        if row.tex.is_some() || matches!(row.state, PosterState::Failed) {
            return false;
        }
        if row.path.is_none() {
            row.path = find_any_by_key(&row.key); // fallback single lookup only once
        }
        if let Some(path) = row.path.as_ref() {
            match PexApp::load_texture_from_path(ctx, &path.to_string_lossy(), &row.key) {
                Ok(tex) => {
                    row.tex = Some(tex);
                    row.state = PosterState::Ready;
                    return true;
                }
                Err(_) => {
                    row.state = PosterState::Failed;
                }
            }
        }
    }
    false
}

/// Upload a handful of textures immediately for the first visible window (fast perception).
fn prewarm_first_screen(&mut self, ctx: &eg::Context) {
    // Only target near-future rows (for 2d/7d/etc.) and take the first PREWARM_UPLOADS
    let now_bucket = Self::day_bucket(SystemTime::now());
    let max_bucket_opt: Option<i64> = match self.current_range {
        DayRange::Two => Some(now_bucket + 2),
        DayRange::Seven => Some(now_bucket + 7),
        DayRange::Fourteen => Some(now_bucket + 14),
        DayRange::All => None,
    };

    let targets: Vec<usize> = self.rows.iter().enumerate()
        .filter_map(|(idx, row)| {
            let b = row.airing.map(Self::day_bucket)?;
            if b < now_bucket { return None; }
            if let Some(max_b) = max_bucket_opt { if b >= max_b { return None; } }
            Some(idx)
        })
        .take(PREWARM_UPLOADS * 2) // grab a few extra so we have buffers
        .collect();

    // Keep ordering stable (rows are already time-ordered; this is a no-op in most cases)
    // Attempt uploads up to PREWARM_UPLOADS
    let mut uploaded = 0usize;
    for idx in targets {
        if uploaded >= PREWARM_UPLOADS { break; }
        if self.try_lazy_upload_row(ctx, idx) {
            uploaded += 1;
        }
    }
}

    fn set_status<S: Into<String>>(&mut self, s: S) {
        let s = s.into();
        let due = self.status_last_emit.elapsed() >= Duration::from_millis(STATUS_EMIT_EVERY_MS);
        let changed = self.loading_message != s;
        if changed || due {
            self.loading_message = s;
            self.status_last_emit = Instant::now();
        }
    }

#[allow(dead_code)]
fn load_rows_via_plex_join(&mut self, db_path: &str, limit: usize) -> Result<(), String> {
    let conn = Connection::open(db_path).map_err(|e| format!("open db: {e}"))?;
    if !(table_exists(&conn, "metadata_items") && table_exists(&conn, "media_items")) {
        return Err("required tables missing (metadata_items, media_items)".into());
    }

    // Try modern schema first; fall back to older 'thumb_url'
    let mut st = conn
        .prepare(SQL_POSTERS_USER_THUMB)
        .or_else(|e1| {
            let msg = e1.to_string();
            if msg.contains("no such column") && msg.contains("user_thumb_url") {
                conn.prepare(SQL_POSTERS_THUMB).map_err(|e2| {
                    format!("prepare failed:\n{}\n(fallback failed: {})", e1, e2)
                })
            } else {
                Err(e1).map_err(|e| format!("prepare failed: {e}"))
            }
        })?;

    let mut rows = st.query([limit as i64]).map_err(|e| format!("query: {e}"))?;

    // Harvest rows defensively
    let mut out: Vec<(String, String)> = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let title_opt: Option<String> = row.get(0).ok().flatten();
        let poster_opt: Option<String> = row.get(1).ok().flatten();
        if let (Some(title), Some(poster)) = (title_opt, poster_opt) {
            let t = title.trim();
            if !t.is_empty() && (poster.starts_with("http://") || poster.starts_with("https://")) {
                out.push((t.to_owned(), poster));
            }
        }
    }

    if out.is_empty() {
        return Err("query returned 0 posters".into());
    }

    // Deduplicate by title (case-insensitive), keep order
    let mut seen = std::collections::HashSet::new();
    out.retain(|(t, _)| seen.insert(t.to_ascii_lowercase()));

    self.rows = out
        .into_iter()
        .map(|(t, u)| PosterRow::new(t, u, None)) // NOTE: pass None for airing in legacy helper
        .collect();
    Ok(())
}

/// Phase 2+3: poster prep warm-up (one-shot on app launch)
fn start_poster_prep(&mut self) {
    if self.prep_started { return; }
    self.prep_started = true;
    self.boot_phase = BootPhase::CheckingNew;
    self.set_status("Checking for new posters…");
    self.last_item_msg.clear();

    let (tx, rx) = mpsc::channel::<PrepMsg>();
    self.prep_rx = Some(rx);

    let cfg = load_config();
    let db_path_opt = cfg.plex_db_local.clone();
    let src_path_opt = cfg.plex_db_source.clone();

    std::thread::spawn(move || {
        let send = |m: PrepMsg| { let _ = tx.send(m); };

        if DIAG_FAKE_STARTUP {
            send(PrepMsg::Info("DIAG: synthesizing small poster list…".into()));
            let fake: Vec<(String, String, String, Option<i64>, Option<i32>, Option<String>)> = vec![
                ("Blade Runner".into(), "https://example.com/a.jpg".into(), url_to_cache_key("https://example.com/a.jpg"), None, Some(1982), Some("ITV2".into())),
                ("Alien".into(),        "https://example.com/b.jpg".into(), url_to_cache_key("https://example.com/b.jpg"), None, Some(1979), Some("ITV2".into())),
                ("Arrival".into(),      "https://example.com/c.jpg".into(), url_to_cache_key("https://example.com/c.jpg"), None, Some(2016), Some("ITV2".into())),
            ];
            send(PrepMsg::Done(fake));
            return;
        }

        // (A) Resolve DB path
        let db_path = match db_path_opt {
            Some(p) => p,
            None => { send(PrepMsg::Error("No plex_db_local set in config.json".into())); return; }
        };

        // (B) Optional daily copy
        if let Some(src_path) = src_path_opt {
            match needs_db_update_daily(&src_path, &db_path) {
                Ok(true) => {
                    send(PrepMsg::Info("Updating local EPG DB…".into()));
                    let marker = last_sync_marker_path(&db_path);
                    let _ = copy_with_progress(&src_path, &db_path, |_c,_t,_p,_mbps|{});
                    let _ = touch_last_sync(&marker);
                }
                Ok(false) => send(PrepMsg::Info("Local EPG DB fresh — skipping update.".into())),
                Err(e) => send(PrepMsg::Info(format!("Freshness check failed (continuing): {e}"))),
            }
        } else {
            send(PrepMsg::Info("Using existing local EPG DB.".into()));
        }

        // (C) Open DB read-only
        let conn = match Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(e) => { send(PrepMsg::Error(format!("open db failed: {e}"))); return; }
        };
        let _ = conn.busy_timeout(Duration::from_secs(10));
        let _ = conn.pragma_update(None, "temp_store", "MEMORY");

        if !(table_exists(&conn, "metadata_items") && table_exists(&conn, "media_items")) {
            send(PrepMsg::Error("required tables missing (metadata_items, media_items)".into()));
            return;
        }

        // (D) SQL with fallback (NOTE: includes mi.extra_data for channel extraction)
        let mut st = match conn.prepare(SQL_POSTERS_USER_THUMB) {
            Ok(s) => s,
            Err(e1) => {
                if e1.to_string().contains("user_thumb_url") {
                    match conn.prepare(SQL_POSTERS_THUMB) {
                        Ok(s) => s,
                        Err(e2) => { send(PrepMsg::Error(format!("prepare failed: {e1} / fallback: {e2}"))); return; }
                    }
                } else {
                    send(PrepMsg::Error(format!("prepare failed: {e1}"))); return;
                }
            }
        };

        // (E) Harvest rows — NO DOWNLOADING HERE
        send(PrepMsg::Info("Scanning EPG…".into()));
        let mut q = match st.query([i64::MAX]) {
            Ok(r) => r,
            Err(e) => { send(PrepMsg::Error(format!("query failed: {e}"))); return; }
        };

        let mut list: Vec<(String, String, String, Option<i64>, Option<i32>, Option<String>)> = Vec::new();
        let mut last_emit = Instant::now();

        while let Ok(Some(row)) = q.next() {
            let title: Option<String>  = row.get(0).ok().flatten();
            let url:   Option<String>  = row.get(1).ok().flatten();
            let begins:Option<i64>     = row.get(2).ok().flatten();
            let year:  Option<i32>     = row.get(3).ok().flatten();
            let extra: Option<String>  = row.get(4).ok().flatten();

            if let (Some(t), Some(u)) = (title, url) {
                let tt = t.trim();
                if !tt.is_empty() && (u.starts_with("http://") || u.starts_with("https://")) {
                    let key = url_to_cache_key(&u);
                    let ch = extra.as_deref().and_then(parse_channel_from_extra);
                    list.push((tt.to_owned(), u, key, begins, year, ch));
                    if last_emit.elapsed() >= Duration::from_millis(600) {
                        send(PrepMsg::Info(format!("Found {} posters…", list.len())));
                        last_emit = Instant::now();
                    }
                }
            }
        }

        // (F) Dedupe by title (stable)
        let mut seen = std::collections::HashSet::new();
        list.retain(|(t, ..)| seen.insert(t.to_ascii_lowercase()));

        // (G) Hand results to UI (workers will download in parallel)
        send(PrepMsg::Done(list));
    });
}

fn poll_prep(&mut self, ctx: &eg::Context) {
    use std::sync::mpsc::TryRecvError;
    const MAX_MSGS: usize = 16;

    let mut seen_any = false;
    let mut processed = 0;

    if let Some(rx) = self.prep_rx.take() {
        let mut keep = Some(rx);

        while let Some(r) = keep.as_ref() {
            if processed >= MAX_MSGS { break; }
            match r.try_recv() {
                Ok(PrepMsg::Info(s)) => {
                    self.set_status(s);
                    if !matches!(self.boot_phase, BootPhase::Caching | BootPhase::Ready) {
                        self.boot_phase = BootPhase::Caching;
                    }
                    processed += 1;
                    seen_any = true;
                }
                Ok(PrepMsg::Done(list)) => {
                    self.rows = list.into_iter()
                        // (title, url, base_key, begins_opt, year_opt, channel_opt)
                        .map(|(t, u, base_k, ts_opt, year_opt, ch_opt)| {
                            let airing  = ts_opt.map(|ts| SystemTime::UNIX_EPOCH + Duration::from_secs(ts as u64));
                            let channel = ch_opt.or_else(|| Self::host_from_url(&u));
                            let small_k = Self::small_key(&base_k);
                            let path    = find_any_by_key(&small_k); // record cached file path once
                            // compute state BEFORE moving `path` into the struct
                            let state = if path.is_some() { PosterState::Cached } else { PosterState::Pending };
                            PosterRow {
                                title: t,
                                url: u,
                                key: small_k,    // use small variant key everywhere
                                airing,
                                year: year_opt,
                                channel,
                                path,            // move here
                                tex: None,
                                state,           // and use the precomputed state
                            }
                        })
                        .collect();

                    self.boot_phase = BootPhase::Ready;
                    self.set_status(format!("Poster prep complete. {} items ready.", self.rows.len()));

                    // Start workers (will fetch any missing small files), and prewarm first screen
                    self.start_prefetch(ctx);
                    self.prewarm_first_screen(ctx);

                    keep = None;
                    seen_any = true;
                }
                Ok(PrepMsg::Error(e)) => {
                    self.set_status(format!("Poster prep error: {e}"));
                    keep = None;
                    seen_any = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => { keep = None; break; }
            }
        }

        if let Some(rx_back) = keep { self.prep_rx = Some(rx_back); }
    }

    if seen_any { ctx.request_repaint(); }
}

// ---- status/phase helpers ----
fn set_phase(&mut self, phase: Phase) {
    self.phase = phase;
    self.phase_started = Instant::now();
}

fn ready_count(&self) -> usize {
    self.rows.iter().filter(|r| r.tex.is_some()).count()
}

fn in_flight(&self) -> usize {
    self.total_targets.saturating_sub(self.completed + self.failed)
}

fn should_show_grid(&self) -> bool {
    if self.rows.is_empty() {
        return false;
    }
    if !SHOW_GRID_EARLY {
        return self.prefetch_started && self.loading_progress >= 1.0;
    }
    self.ready_count() >= MIN_READY_BEFORE_GRID
        || (self.prefetch_started && self.loading_progress >= 1.0)
}

// ---- texture helpers (UI thread only) ----
fn upload_rgba(
    ctx: &eg::Context,
    w: u32,
    h: u32,
    bytes: &[u8],
    name: &str,
) -> TextureHandle {
    let img = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], bytes);
    ctx.load_texture(name.to_string(), img, eg::TextureOptions::LINEAR)
}

fn load_texture_from_path(
    ctx: &eg::Context,
    path_str: &str,
    cache_name: &str,
) -> Result<TextureHandle, String> {
    let (w, h, bytes) = load_rgba_raw_or_image(path_str)?;
    // Portrait-aspect sanity check ~2:3
    let ar = (w as f32) / (h as f32);
    if !(0.55..=0.80).contains(&ar) {
        return Err(format!("non-poster aspect {w}x{h} ar={ar:.2}"));
    }
    Ok(Self::upload_rgba(ctx, w, h, &bytes, cache_name))
}

    // ----- upfront prefetch -----
/// Start prefetch: queue all rows, but avoid repeated disk lookups by reusing row.path.
/// Workers will download the SMALL variant (key `__s`) if missing.
fn start_prefetch(&mut self, ctx: &eg::Context) {
    if self.prefetch_started { return; }
    self.prefetch_started = true;

    self.completed = 0;
    self.failed = 0;
    self.total_targets = self.rows.len();
    self.loading_progress = if self.total_targets == 0 { 1.0 } else { 0.0 };
    self.last_item_msg.clear();
    self.set_phase(Phase::Prefetching);
    self.set_status(format!("Prefetching {} posters…", self.total_targets));

    let (work_tx, work_rx) = mpsc::channel::<(usize, String, String, Option<PathBuf>)>();
    let (done_tx, done_rx) = mpsc::channel::<PrefetchDone>();
    self.work_tx = Some(work_tx.clone());
    self.done_rx = Some(done_rx);

    let work_rx = std::sync::Arc::new(std::sync::Mutex::new(work_rx));

    // Build ONE shared client (connection pooling + keep-alive + HTTP/2 multiplexing)
    let client = match reqwest::blocking::Client::builder()
        .user_agent("pex_new/prefetch")
        .timeout(Duration::from_secs(20))
        .http2_adaptive_window(true)
        .pool_max_idle_per_host(16)
        .default_headers({
            use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
            let mut h = HeaderMap::new();
            h.insert(ACCEPT, HeaderValue::from_static("image/avif,image/webp,image/*;q=0.8,*/*;q=0.5"));
            h
        })
        .build()
    {
        Ok(c) => std::sync::Arc::new(c),
        Err(e) => {
            // If we can't even create a client, mark all as failed to keep UI consistent
            self.set_status(format!("http client build failed: {e}"));
            self.failed = self.total_targets;
            self.loading_progress = 1.0;
            return;
        }
    };

    for _ in 0..WORKER_COUNT {
        let work_rx = std::sync::Arc::clone(&work_rx);
        let done_tx = done_tx.clone();
        let client = std::sync::Arc::clone(&client);

        std::thread::spawn(move || {
            loop {
                let job = { let rx = work_rx.lock().unwrap(); rx.recv() };
                let (row_idx, key, url, cached_path) = match job {
                    Ok(t) => t,
                    Err(_) => break,
                };

                let result: Result<PathBuf, String> = if let Some(path) = cached_path {
                    Ok(path)
                } else {
                    download_and_store_resized_with_client(&client, &url, &key, RESIZE_MAX_W, RESIZE_QUALITY)
                        .or_else(|_e| download_and_store(&url, &key))
                };

                let _ = done_tx.send(PrefetchDone { row_idx, result });
            }
        });
    }

    // Queue strategy: near-term airings FIRST (2d window), then the rest (stable order).
    let now_bucket = Self::day_bucket(SystemTime::now());
    let soon_cutoff = now_bucket + 2; // prioritize next 2 days

    // collect indices with a priority flag
    let mut indices: Vec<(bool, usize)> = self.rows.iter().enumerate()
        .map(|(i, r)| {
            let prio = r.airing.map(|ts| Self::day_bucket(ts) < soon_cutoff).unwrap_or(false);
            (prio, i)
        })
        .collect();

    // stable-partition: prio=true first, then false, without expensive sorting
    indices.sort_by_key(|(prio, i)| (std::cmp::Reverse(*prio), *i));

    for (_, idx) in indices {
        let row = &mut self.rows[idx];
        row.state = if row.path.is_some() { PosterState::Cached } else { PosterState::Pending };
        let _ = work_tx.send((idx, row.key.clone(), row.url.clone(), row.path.clone()));
    }

    // Perceptual boost
    self.prewarm_first_screen(ctx);
    ctx.request_repaint();
}

    /// Poll prefetch completions and update progress/splash.
fn poll_prefetch_done(&mut self, ctx: &eg::Context) {
    let mut drained = 0usize;

    while drained < MAX_DONE_PER_FRAME {
        let Some(rx) = &self.done_rx else { break; };

        match rx.try_recv() {
            Ok(msg) => {
                drained += 1;
                match msg.result {
                    Ok(path) => {
                        if let Some(row) = self.rows.get_mut(msg.row_idx) {
                            row.path = Some(path);
                            row.state = PosterState::Cached; // will be uploaded lazily during paint
                            self.completed += 1;
                            if self.first_ready_at.is_none() {
                                self.first_ready_at = Some(Instant::now());
                            }
                            self.last_item_msg = format!("Cached: {}", row.title);
                        } else {
                            self.failed += 1;
                        }
                    }
                    Err(e) => {
                        if let Some(row) = self.rows.get_mut(msg.row_idx) {
                            row.state = PosterState::Failed;
                            self.failed += 1;
                            self.last_item_msg = format!("Download failed: {} — {}", row.title, e);
                        } else {
                            self.failed += 1;
                        }
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }

    if self.total_targets > 0 {
        self.loading_progress =
            ((self.completed + self.failed) as f32 / self.total_targets as f32)
                .clamp(0.0, 1.0);
    } else {
        self.loading_progress = 1.0;
    }

    if drained > 0 {
        ctx.request_repaint();
    }
}
}

// ========== App impl ==========
impl eframe::App for PexApp {
fn update(&mut self, ctx: &eg::Context, _frame: &mut eframe::Frame) {
    // Keep frames moving so Windows never flags "Not Responding"
    ctx.request_repaint();

    // First frame
    if !self.did_init {
        self.did_init = true;
        self.loading_message = "Starting…".into();
        self.heartbeat_last = Instant::now();
        self.heartbeat_dots = 0;

        // One-shot warm-up: check/copy DB, harvest rows, ensure cache
        self.start_poster_prep();
    }

    // Drive warm-up progress
    self.poll_prep(ctx);

    // If warm-up not finished, show calm splash and return
    if self.boot_phase != BootPhase::Ready {
        eg::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.heading("Poster preparation…");
                ui.add(eg::Spinner::new().size(16.0));
                ui.separator();
                ui.label(&self.loading_message);
                ui.monospace(format!("Cache: {}", cache_dir().display()));
            });
        });
        return;
    }

    // If prefetch still running, keep polling completions
    if self.prefetch_started && self.loading_progress < 1.0 {
        self.poll_prefetch_done(ctx);
    }
    if self.prefetch_started && self.loading_progress >= 1.0 && !self.rows.is_empty() {
        if !matches!(self.phase, Phase::Ready) {
            self.set_phase(Phase::Ready);
            self.set_status("All posters processed.");
        }
    }

    // Soft heartbeat ticker (safe, non-blocking)
    if (self.rows.is_empty() || (self.prefetch_started && self.loading_progress < 1.0))
        && self.heartbeat_last.elapsed() >= Duration::from_millis(250)
    {
        self.heartbeat_last = Instant::now();
        self.heartbeat_dots = (self.heartbeat_dots + 1) % 4;
    }

    // ---- Main UI ----
    eg::CentralPanel::default().show(ctx, |ui| {
        // Top bar: Day window selector (view-only)
        ui.horizontal(|ui| {
            ui.label("Window:");
            let mut changed = false;

            let mut pick = self.current_range;
            changed |= ui.selectable_value(&mut pick, DayRange::Two, "2d").clicked();
            changed |= ui.selectable_value(&mut pick, DayRange::Seven, "7d").clicked();
            changed |= ui.selectable_value(&mut pick, DayRange::Fourteen, "14d").clicked();
            changed |= ui.selectable_value(&mut pick, DayRange::All, "All").clicked();

            if changed && pick != self.current_range {
                self.current_range = pick;
            }
        });
        ui.separator();

        // Decide whether to show early splash
        let show_splash = !self.should_show_grid();
        if show_splash {
            let done = self.completed + self.failed;
            let inflight = self.in_flight();

            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.heading("Preparing posters…");

                if !self.loading_message.is_empty() { ui.label(&self.loading_message); }
                if !self.last_item_msg.is_empty() { ui.monospace(&self.last_item_msg); }

                let db_phase = if self.prefetch_started {
                    self.loading_progress.max(0.02)
                } else {
                    let t = ctx.input(|i| i.time) as f32;
                    0.02 + 0.18 * (t * 0.8 % 1.0)
                };
                ui.add(eg::ProgressBar::new(db_phase).show_percentage());
                ui.separator();
                ui.add(eg::Spinner::new().size(14.0));
                ui.separator();

                ui.monospace(format!(
                    "Posters: {done}/{total}  (OK {ok}, Fail {fail}, In-flight {inflight})",
                    total = self.total_targets, ok = self.completed, fail = self.failed, inflight = inflight
                ));
                ui.monospace(format!("Cache: {}", cache_dir().display()));
            });
            return;
        }

        // ---- Grouped grid by day-of-week ----
        // Build filtered/sorted list of indices with airing within selected window
        let now_bucket = Self::day_bucket(SystemTime::now());
        let max_bucket_opt: Option<i64> = match self.current_range {
            DayRange::Two => Some(now_bucket + 2),
            DayRange::Seven => Some(now_bucket + 7),
            DayRange::Fourteen => Some(now_bucket + 14),
            DayRange::All => None,
        };

        let mut filtered: Vec<(usize, i64)> = self.rows.iter().enumerate()
            .filter_map(|(idx, row)| {
                let ts = row.airing?;
                let b = Self::day_bucket(ts);
                if b < now_bucket { return None; }
                if let Some(max_b) = max_bucket_opt { if b >= max_b { return None; } }
                Some((idx, b))
            })
            .collect();

        // Sort by (day bucket, title) for stable visual layout
        filtered.sort_by(|a, b| {
            let (ai, ab) = a;
            let (bi, bb) = b;
            ab.cmp(bb).then_with(|| self.rows[*ai].title.cmp(&self.rows[*bi].title))
        });

        // Group by bucket
        let groups = {
            let mut out: Vec<(i64, Vec<usize>)> = Vec::new();
            let mut cur_key: Option<i64> = None;
            for (idx, bucket) in filtered {
                if cur_key != Some(bucket) {
                    out.push((bucket, Vec::new()));
                    cur_key = Some(bucket);
                }
                if let Some((_, v)) = out.last_mut() {
                    v.push(idx);
                }
            }
            out
        };

        // Layout parameters (once)
        let available = ui.available_width() - 8.0;
        let card_w: f32 = 140.0;
        let card_h: f32 = 140.0 * 1.5 + 36.0;
        let cols = (available / card_w.max(1.0)).floor().max(1.0) as usize;

        // Bounded texture uploads per frame
        let mut uploads_left = MAX_UPLOADS_PER_FRAME;

        eg::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for (bucket, idxs) in groups {
                // Divider with day label
                ui.add_space(8.0);
                ui.separator();
                let label = Self::format_day_label(bucket);
                ui.heading(label);
                ui.add_space(4.0);

                // A grid per day
                eg::Grid::new(format!("grid_day_{bucket}"))
                    .num_columns(cols)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        for (i, idx) in idxs.into_iter().enumerate() {
                            let (rect, _resp) = ui.allocate_exact_size(
                                eg::vec2(card_w, card_h),
                                eg::Sense::click(),
                            );

                            // Lazy texture upload for visible cards (cache hits don't wait for workers)
                            if uploads_left > 0 {
                                if self.try_lazy_upload_row(ctx, idx) {
                                    uploads_left -= 1;
                                }
                            }

                            // Now paint card
                            let poster_rect = eg::Rect::from_min_max(
                                rect.min,
                                eg::pos2(rect.min.x + card_w, rect.min.y + card_w * 1.5),
                            );
                            let text_rect = eg::Rect::from_min_max(
                                eg::pos2(rect.min.x, poster_rect.max.y),
                                rect.max,
                            );

                            if let Some(row) = self.rows.get(idx) {
                                if let Some(tex) = &row.tex {
                                    ui.painter().image(
                                        tex.id(),
                                        poster_rect,
                                        eg::Rect::from_min_max(eg::pos2(0.0, 0.0), eg::pos2(1.0, 1.0)),
                                        eg::Color32::WHITE,
                                    );
                                } else {
                                    ui.painter().rect_filled(
                                        poster_rect,
                                        6.0,
                                        eg::Color32::from_gray(40),
                                    );
                                }

                                // Build 3-line label:
                                // Line 1: Title (YYYY)
                                // Line 2: <CH>
                                // Line 3: <HH:MM>
                                let mut lines = String::new();
                                match row.year {
                                    Some(y) => lines.push_str(&format!("{} ({})", row.title, y)),
                                    None => lines.push_str(&row.title),
                                }
                                if let Some(ch) = &row.channel {
                                    lines.push_str(&format!("\n{}", ch));
                                }
                                if let Some(ts) = row.airing {
                                    lines.push_str(&format!("\n{}", Self::hhmm_utc(ts)));
                                }

                                ui.painter().text(
                                    text_rect.left_top(),
                                    eg::Align2::LEFT_TOP,
                                    lines,
                                    eg::FontId::proportional(14.0),
                                    eg::Color32::WHITE,
                                );
                            }

                            if (i + 1) % cols == 0 { ui.end_row(); }
                        }
                        ui.end_row();
                    });
            }
        });
    });
}

}