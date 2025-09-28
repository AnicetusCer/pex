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
const WORKER_COUNT: usize = 8; // parallel download workers
const RESIZE_MAX_W: u32 = 500; // target width for cached JPEGs
const RESIZE_QUALITY: u8 = 75; // JPEG quality
const SHOW_GRID_EARLY: bool = true;      // show grid before 100% done
const MIN_READY_BEFORE_GRID: usize = 24; // how many posters before grid appears
const STATUS_EMIT_EVERY_MS: u64 = 120;   // throttle status label updates (~8/s)
const DIAG_FAKE_STARTUP: bool = false;   // set true to simulate startup without touching the DB
const MAX_DONE_PER_FRAME: usize = 12; // cap how many poster-completions we process per frame


// Two SQL variants (newer plex uses user_thumb_url; older uses thumb_url)
const SQL_POSTERS_USER_THUMB: &str = r#"
SELECT
  m.title,
  m.user_thumb_url
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
  m.thumb_url
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
    Done(Vec<(String, String, String)>), // (title, url, key)
    Error(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Starting,
    FreshnessCheck,
    CopyingDb,
    OpeningDb,
    QueryingDb,
    ScanningRows,
    Deduplicating,
    PrefetchQueue,
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
    Pending,
    Ready,
    Failed,
}

// Day-range selector for fast startup
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DayRange {
    Two,
    Seven,
    Fourteen,
    All,
}

impl DayRange {
    fn label(&self) -> &'static str {
        match self {
            DayRange::Two => "2 days",
            DayRange::Seven => "7 days",
            DayRange::Fourteen => "14 days",
            DayRange::All => "All",
        }
    }
    fn limit(&self) -> i64 {
        match self {
            DayRange::Two => 200,       // tiny quick-start
            DayRange::Seven => 700,     // ~1 week
            DayRange::Fourteen => 1500, // ~2 weeks
            DayRange::All => 5000,      // “everything”
        }
    }
}

struct PosterRow {
    title: String,
    url: String,
    key: String,
    tex: Option<TextureHandle>,
    state: PosterState,
}

impl PosterRow {
    fn new(title: String, url: String) -> Self {
        let key = url_to_cache_key(&url);
        Self {
            title,
            url,
            key,
            tex: None,
            state: PosterState::Pending,
        }
    }
}

enum StartupMsg {
    Info(String),
    Phase(Phase),
    Rows(Vec<(String, String)>),
    Error(String),
}

struct PrefetchDone {
    row_idx: usize,
    key: String,
    result: Result<PathBuf, String>,
}

pub struct PexApp {
    // startup thread → splash
    startup_rx: Option<Receiver<StartupMsg>>,

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
            startup_rx: None,
            rows: Vec::new(),

            current_range: DayRange::Two,

            loading_progress: 0.0,
            loading_message: String::new(),
            last_item_msg: String::new(),

            heartbeat_last: Instant::now(),
            heartbeat_dots: 0,
            status_last_emit: Instant::now(),

            phase: Phase::Starting,
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
    fn set_status<S: Into<String>>(&mut self, s: S) {
        let s = s.into();
        let due = self.status_last_emit.elapsed() >= Duration::from_millis(STATUS_EMIT_EVERY_MS);
        let changed = self.loading_message != s;
        if changed || due {
            self.loading_message = s;
            self.status_last_emit = Instant::now();
        }
    }

    /// Phase 2+3: poster prep warm-up (one-shot on app launch)
fn start_poster_prep(&mut self) {
    if self.prep_started { return; }
    self.prep_started = true;
    self.boot_phase = BootPhase::CheckingNew;
    self.loading_message = "Checking for new posters…".into();
    self.last_item_msg.clear();

    let (tx, rx) = mpsc::channel::<PrepMsg>();
    self.prep_rx = Some(rx);

    // Snapshot config on the UI thread
    let cfg = load_config();
    let db_path_opt = cfg.plex_db_local.clone();
    let src_path_opt = cfg.plex_db_source.clone();

    std::thread::spawn(move || {
        let send = |m: PrepMsg| { let _ = tx.send(m); };

        // (A) Resolve DB path
        let db_path = match db_path_opt {
            Some(p) => p,
            None => { send(PrepMsg::Error("No plex_db_local set in config.json".into())); return; }
        };

        // (B) Optional daily copy from source → local
        if let Some(src_path) = src_path_opt {
            match needs_db_update_daily(&src_path, &db_path) {
                Ok(true) => {
                    send(PrepMsg::Info(format!("Updating local EPG DB…")));
                    let marker = last_sync_marker_path(&db_path);
                    let _ = copy_with_progress(&src_path, &db_path, |_c,_t,_p,_m|{});
                    let _ = touch_last_sync(&marker);
                }
                Ok(false) => {
                    send(PrepMsg::Info("Local EPG DB fresh — skipping update.".into()));
                }
                Err(e) => {
                    send(PrepMsg::Info(format!("Freshness check failed (continuing): {e}")));
                }
            }
        } else {
            send(PrepMsg::Info("Using existing local EPG DB.".into()));
        }

        // (C) Open DB read-only
        let conn = match Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
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

        // (D) Prepare SQL with column fallback
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

        // (E) Harvest all rows (no LIMIT; we’ll filter in the UI)
        send(PrepMsg::Info("Scanning EPG…".into()));
        let mut q = match st.query([i64::MAX]) {
            Ok(r) => r,
            Err(e) => { send(PrepMsg::Error(format!("query failed: {e}"))); return; }
        };

        let mut list: Vec<(String, String, String)> = Vec::new(); // (title,url,key)
        let mut last_emit = Instant::now();
        while let Ok(Some(row)) = q.next() {
            let title: Option<String> = row.get(0).ok().flatten();
            let url: Option<String>   = row.get(1).ok().flatten();
            if let (Some(t), Some(u)) = (title, url) {
                let tt = t.trim();
                if !tt.is_empty() && (u.starts_with("http://") || u.starts_with("https://")) {
                    let key = url_to_cache_key(&u);
                    list.push((tt.to_owned(), u, key));
                    if last_emit.elapsed() >= Duration::from_millis(600) {
                        send(PrepMsg::Info(format!("Found {} posters…", list.len())));
                        last_emit = Instant::now();
                    }
                }
            }
        }

        // (F) Dedupe by title (stable)
        let mut seen = std::collections::HashSet::new();
        list.retain(|(t,_,_)| seen.insert(t.to_ascii_lowercase()));

        // (G) Ensure cache (download or skip if present)
        send(PrepMsg::Info("Ensuring poster cache…".into()));
        let client = match reqwest::blocking::Client::builder()
            .user_agent("pex_new/prep")
            .timeout(Duration::from_secs(20))
            .build()
        {
            Ok(c) => c,
            Err(e) => { send(PrepMsg::Error(format!("http client: {e}"))); return; }
        };

        let mut done: usize = 0;
        let total = list.len();
        let mut last_emit = Instant::now();

        for (_t, url, key) in list.iter() {
            if find_any_by_key(key).is_none() {
                // Try resized → fallback original
                let _ = download_and_store_resized_with_client(&client, url, key, RESIZE_MAX_W, RESIZE_QUALITY)
                    .or_else(|_| download_and_store(url, key));
            }
            done += 1;
            if last_emit.elapsed() >= Duration::from_millis(700) {
                send(PrepMsg::Info(format!("Caching posters… {done}/{total}")));
                last_emit = Instant::now();
            }
            // Let scheduler breathe a bit on huge sets
            if done % 400 == 0 { std::thread::sleep(Duration::from_millis(1)); }
        }

        // (H) Done → hand the manifest to UI
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
                    self.loading_message = s;
                    self.boot_phase = BootPhase::Caching; // once we start emitting, we’re in caching/prep
                    processed += 1;
                    seen_any = true;
                }
                Ok(PrepMsg::Done(list)) => {
                    // Convert manifest to rows (textures will be created later from cache)
                    self.rows = list.into_iter()
                        .map(|(t,u,k)| PosterRow { title: t, url: u, key: k, tex: None, state: PosterState::Pending })
                        .collect();
                    self.boot_phase = BootPhase::Ready;
                    self.loading_message = format!("Poster prep complete. {} items ready.", self.rows.len());

                    // Build textures from cache
                    self.start_prefetch(ctx);

                    keep = None; // channel no longer needed
                }
                Ok(PrepMsg::Error(e)) => {
                    self.loading_message = format!("Poster prep error: {e}");
                    keep = None;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => { keep = None; break; }
            }
        }
        if let Some(rx_back) = keep { self.prep_rx = Some(rx_back); }
    }

    if seen_any { ctx.request_repaint(); }
}

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_started = Instant::now();
    }

    fn ready_count(&self) -> usize {
        self.completed
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
        // Portrait-ish aspect guard: ~2:3 → width/height in 0.55..0.80
        let ar = (w as f32) / (h as f32);
        if !(0.55..=0.80).contains(&ar) {
            return Err(format!("non-poster aspect {w}x{h} ar={ar:.2}"));
        }
        Ok(Self::upload_rgba(ctx, w, h, &bytes, cache_name))
    }

    /// Load up to `limit` rows (title, poster_url) using a simple join.
    /// Safe for small limits on the UI thread; otherwise prefer `start_db_scan`.
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
                if !t.is_empty() && (poster.starts_with("http://") || poster.starts_with("https://"))
                {
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
            .map(|(t, u)| PosterRow::new(t, u))
            .collect();
        Ok(())
    }

    // ----- upfront prefetch -----
    /// Start prefetch: load textures immediately for cache hits; queue only cache misses for workers.
    fn start_prefetch(&mut self, ctx: &eg::Context) {
        if self.prefetch_started {
            return;
        }
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

        for _ in 0..WORKER_COUNT {
            let work_rx = std::sync::Arc::clone(&work_rx);
            let done_tx = done_tx.clone();

            std::thread::spawn(move || {
                let client = match reqwest::blocking::Client::builder()
                    .user_agent("pex_new/prefetch")
                    .timeout(Duration::from_secs(20))
                    .default_headers({
                        use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
                        let mut h = HeaderMap::new();
                        h.insert(
                            ACCEPT,
                            HeaderValue::from_static(
                                "image/avif,image/webp,image/*;q=0.8,*/*;q=0.5",
                            ),
                        );
                        h
                    })
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = done_tx.send(PrefetchDone {
                            row_idx: 0,
                            key: String::new(),
                            result: Err(format!("client build: {e}")),
                        });
                        return;
                    }
                };

                loop {
                    let job = {
                        let rx = work_rx.lock().unwrap();
                        rx.recv()
                    };
                    let (row_idx, key, url, cached_path) = match job {
                        Ok(t) => t,
                        Err(_) => break,
                    };

                    let result: Result<PathBuf, String> = if let Some(path) = cached_path {
                        Ok(path)
                    } else {
                        download_and_store_resized_with_client(
                            &client,
                            &url,
                            &key,
                            RESIZE_MAX_W,
                            RESIZE_QUALITY,
                        )
                        .or_else(|_e| download_and_store(&url, &key))
                    };

                    let _ = done_tx.send(PrefetchDone { row_idx, key, result });
                }
            });
        }

        for (idx, row) in self.rows.iter_mut().enumerate() {
            row.state = PosterState::Pending;
            let cached = find_any_by_key(&row.key);
            let _ = work_tx.send((idx, row.key.clone(), row.url.clone(), cached));
        }

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
                                match PexApp::load_texture_from_path(
                                    ctx,
                                    &path.to_string_lossy(),
                                    &msg.key,
                                ) {
                                    Ok(tex) => {
                                        row.tex = Some(tex);
                                        row.state = PosterState::Ready;
                                        self.completed += 1;
                                        if self.first_ready_at.is_none() {
                                            self.first_ready_at = Some(Instant::now());
                                        }
                                        self.last_item_msg =
                                            format!("Loaded: {}", row.title);
                                    }
                                    Err(e) => {
                                        row.state = PosterState::Failed;
                                        self.failed += 1;
                                        self.last_item_msg = format!(
                                            "Skipped (aspect): {} — {}",
                                            row.title, e
                                        );
                                    }
                                }
                            } else {
                                self.failed += 1;
                            }
                        }
                        Err(e) => {
                            if let Some(row) = self.rows.get_mut(msg.row_idx) {
                                row.state = PosterState::Failed;
                                self.failed += 1;
                                self.last_item_msg =
                                    format!("Download failed: {} — {}", row.title, e);
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
            self.loading_progress = ((self.completed + self.failed) as f32
                / self.total_targets as f32)
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
            // Day window selector (no rescan; just filters view)
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

            // Grid
            let available = ui.available_width() - 8.0;
            let card_w: f32 = 140.0;
            let card_h: f32 = 140.0 * 1.5 + 36.0;
            let cols = (available / card_w.max(1.0)).floor().max(1.0) as usize;

            eg::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                eg::Grid::new("grid").num_columns(cols).spacing([8.0, 8.0]).show(ui, |ui| {
                    let show_limit = self.current_range.limit() as usize;
                    for (i, row) in self.rows.iter().take(show_limit).enumerate() {
                        let (rect, _resp) = ui.allocate_exact_size(eg::vec2(card_w, card_h), eg::Sense::click());
                        let poster_rect = eg::Rect::from_min_max(rect.min, eg::pos2(rect.min.x + card_w, rect.min.y + card_w * 1.5));
                        let text_rect = eg::Rect::from_min_max(eg::pos2(rect.min.x, poster_rect.max.y), rect.max);

                        if let Some(tex) = &row.tex {
                            ui.painter().image(tex.id(), poster_rect, eg::Rect::from_min_max(eg::pos2(0.0, 0.0), eg::pos2(1.0, 1.0)), eg::Color32::WHITE);
                        } else {
                            ui.painter().rect_filled(poster_rect, 6.0, eg::Color32::from_gray(40));
                        }

                        ui.painter().text(
                            text_rect.left_top(),
                            eg::Align2::LEFT_TOP,
                            &row.title,
                            eg::FontId::proportional(14.0),
                            eg::Color32::WHITE,
                        );

                        if (i + 1) % cols == 0 { ui.end_row(); }
                    }
                    ui.end_row();
                });
            });
        });
    }
}

