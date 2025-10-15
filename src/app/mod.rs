// src/app/mod.rs — async DB scan + upfront poster prefetch + resized cache + single splash

// ---- Standard lib imports ----
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

// ---- Crates ----
use eframe::egui as eg;
use serde::Deserialize;
use tracing::warn;
use urlencoding::encode;

// ---- Local modules ----
pub mod cache;
use crate::app::cache::find_any_by_key;
use crate::config::{load_config, local_db_path};

type WorkItem = (usize, String, String, Option<PathBuf>);

pub mod prep;
pub mod types;
pub mod utils;
pub use types::{
    BootPhase, DayRange, OwnedMsg, Phase, PosterRow, PosterState, PrefetchDone, PrepItem, PrepMsg,
    RatingMsg, RatingState, SortKey,
};
pub mod detail;
pub mod filters;
pub mod gfx;
pub mod owned;
pub mod prefetch;
pub mod prefs;
#[path = "ui/uimod.rs"] // this is we don't have duplicate file names in within the workspace.
pub mod ui;

// ---- Tunables ----
const WORKER_COUNT: usize = 16; // up from 8 — tune freely (8–32 typical)
const RESIZE_MAX_W: u32 = 320;
const RESIZE_QUALITY: u8 = 75;
const SHOW_GRID_EARLY: bool = true;
const MIN_READY_BEFORE_GRID: usize = 24;
const STATUS_EMIT_EVERY_MS: u64 = 120;
const MAX_DONE_PER_FRAME: usize = 12;
const MAX_UPLOADS_PER_FRAME: usize = 4;
const PREWARM_UPLOADS: usize = 24;
pub(crate) const OWNED_SCAN_COMPLETE_STATUS: &str =
    "Stage 3/4 - Owned scan complete (Owned and HD badges ready). Finishing artwork cache...";

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

    // one-time init guard
    did_init: bool,

    // prefetch plumbing
    prefetch_started: bool,
    total_targets: usize,
    completed: usize,
    failed: usize,

    work_tx: Option<Sender<WorkItem>>,
    done_rx: Option<Receiver<PrefetchDone>>,

    // --- control flags (UI only; not wired yet) ---
    hide_owned: bool,
    dim_owned: bool,

    // darken strength for dimming (0.10–0.90)
    dim_strength_ui: f32,

    // background owned scan
    owned_rx: Option<Receiver<OwnedMsg>>,
    owned_keys: Option<HashSet<String>>,
    owned_hd_keys: Option<HashSet<String>>,
    owned_modified: Option<HashMap<String, Option<u64>>>,
    owned_scan_in_progress: bool,
    owned_scan_messages: VecDeque<String>,
    rating_tx: Option<Sender<RatingMsg>>,
    rating_rx: Option<Receiver<RatingMsg>>,
    rating_states: HashMap<String, RatingState>,

    // search/filter/sort controls
    search_query: String,
    filter_hd_only: bool,

    // channel filter
    show_channel_filter_popup: bool,
    selected_channels: std::collections::BTreeSet<String>,
    selected_genres: std::collections::BTreeSet<String>,
    show_genre_filter_popup: bool,
    show_advanced_popup: bool,
    advanced_feedback: Option<String>,
    setup_checked: bool,
    setup_errors: Vec<String>,
    setup_warnings: Vec<String>,
    stage4_complete_message: Option<String>,
    channel_icon_textures: HashMap<String, eg::TextureHandle>,
    channel_icon_pending: HashSet<String>,

    // sorting
    sort_key: SortKey,
    sort_desc: bool,

    // poster size (UI only for now)
    poster_width_ui: f32, // e.g., card width in px

    // concurrency (UI placeholder; not applied to workers yet)
    worker_count_ui: usize,

    // --- prefs autosave ---
    prefs_dirty: bool,
    prefs_last_write: Instant,

    last_hotset: Option<std::collections::HashMap<String, PathBuf>>,

    selected_idx: Option<usize>,
    // UI state
    detail_panel_width: f32,
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

            hide_owned: false,
            dim_owned: false,
            dim_strength_ui: 0.8, // stronger dimming by default

            owned_rx: None,
            owned_keys: Self::load_owned_keys_sidecar(),
            owned_hd_keys: Self::load_owned_hd_sidecar(),
            owned_modified: None,
            owned_scan_in_progress: false,
            owned_scan_messages: VecDeque::new(),
            rating_tx: None,
            rating_rx: None,
            rating_states: HashMap::new(),

            search_query: String::new(),
            filter_hd_only: false,

            show_channel_filter_popup: false,
            selected_channels: std::collections::BTreeSet::new(),
            selected_genres: std::collections::BTreeSet::new(),
            show_genre_filter_popup: false,
            show_advanced_popup: false,
            advanced_feedback: None,
            setup_checked: false,
            setup_errors: Vec::new(),
            setup_warnings: Vec::new(),
            stage4_complete_message: None,
            channel_icon_textures: HashMap::new(),
            channel_icon_pending: HashSet::new(),

            sort_key: SortKey::Time,
            sort_desc: false,

            poster_width_ui: 143.0,        // tuned default card width
            worker_count_ui: WORKER_COUNT, // show the current worker count

            prefs_dirty: false,
            prefs_last_write: Instant::now(),

            last_hotset: prefs::load_hotset_manifest().ok(),

            selected_idx: None,

            detail_panel_width: 320.0,
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

    pub(crate) fn make_owned_key(title: &str, year: Option<i32>) -> String {
        let normalized = utils::normalize_title(title);
        let year = year.or_else(|| utils::find_year_in_str(title));
        if let Some(year) = year {
            format!("{normalized}:{year}")
        } else {
            let digest = md5::compute(normalized.as_bytes());
            let short = format!("{:x}", digest);
            let short = &short[..short.len().min(8)];
            format!("{normalized}:0:{short}")
        }
    }

    /// Determine whether the airing metadata implies an HD broadcast.
    pub(crate) fn row_broadcast_hd(row: &PosterRow) -> bool {
        row.broadcast_hd
    }

    /// Determine whether the owned library already has an HD copy of this title.
    pub(crate) fn row_owned_is_hd(&self, row: &PosterRow) -> bool {
        self.owned_hd_keys
            .as_ref()
            .is_some_and(|set| set.contains(&row.owned_key))
    }

    fn load_owned_keys_sidecar() -> Option<HashSet<String>> {
        Self::load_sidecar_file("owned_all.txt")
    }

    fn load_owned_hd_sidecar() -> Option<HashSet<String>> {
        Self::load_sidecar_file("owned_hd.txt")
    }

    fn load_sidecar_file(file_name: &str) -> Option<HashSet<String>> {
        use std::{collections::HashSet, fs};
        let path = crate::app::cache::cache_dir().join(file_name);
        let text = fs::read_to_string(path).ok()?;
        let mut set = HashSet::new();
        for line in text.lines().map(str::trim) {
            if !line.is_empty() {
                set.insert(line.to_owned());
            }
        }
        Some(set)
    }
}

impl PexApp {
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
                match crate::app::gfx::load_texture_from_path(
                    ctx,
                    &path.to_string_lossy(),
                    &row.key,
                ) {
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
        let now_bucket = utils::day_bucket(SystemTime::now());
        let max_bucket_opt = self.current_range.max_bucket(now_bucket);

        let targets: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                let b = row.airing.map(utils::day_bucket)?;
                if b < now_bucket {
                    return None;
                }
                if let Some(max_b) = max_bucket_opt {
                    if b >= max_b {
                        return None;
                    }
                }
                Some(idx)
            })
            .take(PREWARM_UPLOADS * 2) // grab a few extra so we have buffers
            .collect();

        // Keep ordering stable (rows are already time-ordered; this is a no-op in most cases)
        // Attempt uploads up to PREWARM_UPLOADS
        let mut uploaded = 0usize;
        for idx in targets {
            if uploaded >= PREWARM_UPLOADS {
                break;
            }
            if self.try_lazy_upload_row(ctx, idx) {
                uploaded += 1;
            }
        }
    }

    fn run_setup_checks(&mut self) {
        self.setup_checked = true;
        self.setup_errors.clear();
        self.setup_warnings.clear();
        self.set_status("Stage 1/4 – Checking config & cache (validates Plex paths and tools).");

        let cfg_path = Path::new("config.json");
        if !cfg_path.exists() {
            self.setup_warnings.push(
                "config.json not found next to the executable; using built-in defaults.".into(),
            );
        }

        let cfg = load_config();
        let local_db = local_db_path();
        if !local_db.exists() {
            if cfg
                .plex_db_source
                .as_deref()
                .map_or(false, |s| !s.trim().is_empty())
            {
                self.setup_warnings.push(format!(
                    "Local Plex EPG database not found at {}; it will be copied from plex_db_source on startup.",
                    local_db.display()
                ));
            } else {
                self.setup_errors.push(format!(
                    "Local Plex EPG database not found at {}. Provide plex_db_source in config.json or copy the DB into the db/ folder.",
                    local_db.display()
                ));
            }
        }

        if cfg.library_roots.is_empty() {
            self.setup_warnings.push(
                "config.json: library_roots is empty; owned titles will not be marked.".into(),
            );
        } else {
            for root in &cfg.library_roots {
                if root.contains("REPLACE_ME") {
                    self.setup_errors.push(
                        "config.json: library_roots contains a REPLACE_ME placeholder; replace it with your library path."
                            .into(),
                    );
                    continue;
                }
                if !Path::new(root).exists() {
                    self.setup_warnings
                        .push(format!("config.json: library root not found: {root}"));
                }
            }
        }

        if !crate::app::utils::ffprobe_available() {
            self.setup_warnings.push(
                "ffprobe not found on PATH; HD detection falls back to filename heuristics.".into(),
            );
        }

        let omdb_missing = cfg
            .omdb_api_key
            .as_ref()
            .map(|k| k.trim().is_empty())
            .unwrap_or(true);
        if omdb_missing {
            self.setup_warnings
                .push("omdb_api_key not set; using the public demo key (rate limited).".into());
        } else if cfg
            .omdb_api_key
            .as_ref()
            .is_some_and(|k| k.contains("REPLACE_ME"))
        {
            self.setup_warnings
                .push("omdb_api_key still uses the REPLACE_ME placeholder; add your own key for reliable ratings."
                    .into());
        }

        if !self.setup_errors.is_empty() {
            if let Some(first) = self.setup_errors.first() {
                self.set_status(format!("Setup required: {first}"));
            }
        } else if self.advanced_feedback.is_none() && !self.setup_warnings.is_empty() {
            self.advanced_feedback = Some(self.setup_warnings.join("\n"));
        }
    }

    fn render_setup_gate(&mut self, ctx: &eg::Context) {
        eg::CentralPanel::default()
            .frame(
                eg::Frame::default()
                    .inner_margin(eg::Margin::symmetric(16.0, 20.0))
                    .fill(ctx.style().visuals.panel_fill),
            )
            .show(ctx, |ui| {
                ui.heading("Pex setup required");
                ui.add_space(8.0);
                if !self.setup_errors.is_empty() {
                    ui.label(
                        eg::RichText::new("Fix these before Pex can start:")
                            .strong()
                            .color(eg::Color32::LIGHT_RED),
                    );
                    ui.add_space(6.0);
                    for err in &self.setup_errors {
                        ui.label(
                            eg::RichText::new(format!("- {err}")).color(eg::Color32::LIGHT_RED),
                        );
                    }
                }

                if !self.setup_warnings.is_empty() {
                    ui.add_space(10.0);
                    ui.label(
                        eg::RichText::new(
                            "Warnings (the app can run, but some features may be limited):",
                        )
                        .strong(),
                    );
                    ui.add_space(4.0);
                    for warn in &self.setup_warnings {
                        ui.label(format!("- {warn}"));
                    }
                }

                ui.add_space(16.0);
                if ui.button("Retry checks").clicked() {
                    self.setup_checked = false;
                }
                ui.label("Edit config.json next to the executable, then press Retry.");
            });
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

    // ---- status/phase helpers ----
    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_started = Instant::now();
    }

    fn ready_count(&self) -> usize {
        self.rows.iter().filter(|r| r.tex.is_some()).count()
    }

    const fn in_flight(&self) -> usize {
        self.total_targets
            .saturating_sub(self.completed + self.failed)
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

    fn record_owned_message<S: Into<String>>(&mut self, msg: S) {
        const MAX_MESSAGES: usize = 8;
        self.owned_scan_messages.push_front(msg.into());
        while self.owned_scan_messages.len() > MAX_MESSAGES {
            self.owned_scan_messages.pop_back();
        }
    }

    fn rating_state_for_key(&self, key: &str) -> RatingState {
        self.rating_states
            .get(key)
            .cloned()
            .unwrap_or(RatingState::Idle)
    }

    fn ensure_rating_channel(&mut self) -> Sender<RatingMsg> {
        if self.rating_tx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<RatingMsg>();
            self.rating_tx = Some(tx.clone());
            self.rating_rx = Some(rx);
        }
        self.rating_tx.as_ref().unwrap().clone()
    }

    fn request_rating_for(&mut self, idx: usize) {
        let Some(row) = self.rows.get(idx) else {
            return;
        };
        let key = row.key.clone();
        if matches!(self.rating_states.get(&key), Some(RatingState::Pending)) {
            return;
        }

        let cfg = load_config();
        let api_key = cfg
            .omdb_api_key
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| "4a3b711b".to_string());
        if api_key.trim().is_empty() {
            self.rating_states.insert(key, RatingState::MissingApiKey);
            return;
        }

        let imdb_id = row.guid.as_deref().and_then(imdb_id_from_guid);
        let title = row.title.clone();
        let year = row.year;
        let sender = self.ensure_rating_channel();

        self.rating_states.insert(key.clone(), RatingState::Pending);

        std::thread::spawn(move || {
            let state = fetch_rating_from_omdb(api_key, imdb_id, title, year);
            let _ = sender.send(RatingMsg { key, state });
        });
    }

    fn poll_rating_updates(&mut self) {
        use std::sync::mpsc::TryRecvError;

        loop {
            let Some(rx) = self.rating_rx.as_ref() else {
                break;
            };
            match rx.try_recv() {
                Ok(msg) => {
                    self.rating_states.insert(msg.key, msg.state);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rating_rx = None;
                    self.rating_tx = None;
                    break;
                }
            }
        }
    }

    fn restart_poster_pipeline(&mut self, ctx: &eg::Context) {
        self.prep_started = false;
        self.prep_rx = None;
        self.prefetch_started = false;
        self.work_tx = None;
        self.done_rx = None;
        self.rows.clear();
        self.total_targets = 0;
        self.completed = 0;
        self.failed = 0;
        self.loading_progress = 0.0;
        self.last_item_msg.clear();
        self.stage4_complete_message = None;
        self.phase = Phase::Prefetching;
        self.phase_started = Instant::now();
        self.boot_phase = BootPhase::Starting;
        self.last_hotset = crate::app::prefs::load_hotset_manifest().ok();
        self.selected_idx = None;
        self.rating_states.clear();
        self.channel_icon_textures.clear();
        self.channel_icon_pending.clear();
        self.owned_modified = None;
        self.set_status("Restarting poster prep…");
        self.start_poster_prep();
        ctx.request_repaint();
    }

    fn channel_icon_texture(&mut self, ctx: &eg::Context, url: &str) -> Option<eg::TextureHandle> {
        if url.trim().is_empty() {
            return None;
        }
        if let Some(tex) = self.channel_icon_textures.get(url) {
            return Some(tex.clone());
        }

        let path = crate::app::cache::channel_icon_path(url);
        if !path.exists() {
            if self.channel_icon_pending.insert(url.to_string()) {
                Self::spawn_channel_icon_prefetch(vec![url.to_string()]);
            }
            return None;
        }

        let path_str = path.to_string_lossy();
        let (w, h, rgba) = crate::app::cache::load_rgba_raw_or_image(&path_str).ok()?;
        if w == 0 || h == 0 || rgba.is_empty() {
            return None;
        }

        let size = [w as usize, h as usize];
        let image = eg::ColorImage::from_rgba_unmultiplied(size, &rgba);
        let key = format!("channel_icon_{}", crate::app::cache::url_to_cache_key(url));
        let tex = ctx.load_texture(key, image, eg::TextureOptions::LINEAR);
        let handle = tex.clone();
        self.channel_icon_textures.insert(url.to_string(), tex);
        self.channel_icon_pending.remove(url);
        Some(handle)
    }

    fn spawn_channel_icon_prefetch(urls: Vec<String>) {
        std::thread::spawn(move || {
            for url in urls {
                let _ = crate::app::cache::ensure_channel_icon(&url);
            }
        });
    }

    fn clear_poster_cache_files(&self) -> Result<usize, String> {
        let dir = crate::app::cache::cache_dir();
        if !dir.exists() {
            return Ok(0);
        }
        let mut removed = 0usize;
        let entries =
            fs::read_dir(&dir).map_err(|err| format!("Failed to read {}: {err}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|err| format!("Failed to read entry in {}: {err}", dir.display()))?;
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|err| format!("Failed to stat {}: {err}", path.display()))?
                .is_file()
            {
                continue;
            }
            let remove = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| {
                    let ext = ext.to_ascii_lowercase();
                    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "rgba")
                })
                .unwrap_or(false);
            if !remove {
                continue;
            }
            fs::remove_file(&path)
                .map_err(|err| format!("Failed to remove {}: {err}", path.display()))?;
            removed += 1;
        }
        let _ = fs::remove_file(dir.join("hotset.txt"));
        Ok(removed)
    }

    fn clear_owned_cache_files(&self) -> Result<usize, String> {
        let dir = crate::app::cache::cache_dir();
        let mut removed = 0usize;
        for name in ["owned_all.txt", "owned_hd.txt", "owned_manifest.json"] {
            let path = dir.join(name);
            match fs::remove_file(&path) {
                Ok(_) => removed += 1,
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(format!("Failed to remove {}: {err}", path.display()));
                }
            }
        }
        let _ = fs::remove_file(dir.join("owned_manifest.json.tmp"));
        Ok(removed)
    }

    fn clear_ffprobe_cache_file(&self) -> Result<bool, String> {
        let path = crate::app::cache::cache_dir().join("ffprobe_cache.json");
        match fs::remove_file(&path) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
            Err(err) => Err(format!("Failed to remove {}: {err}", path.display())),
        }
    }

    fn clear_owned_cache(&mut self) -> Result<usize, String> {
        let removed = self.clear_owned_cache_files()?;
        self.refresh_owned_scan();
        Ok(removed)
    }

    fn clear_ffprobe_cache(&mut self) -> Result<bool, String> {
        let removed = self.clear_ffprobe_cache_file()?;
        crate::app::utils::reset_ffprobe_runtime_state();
        self.stage4_complete_message = None;
        self.start_owned_hd_refresh()?;
        Ok(removed)
    }

    fn refresh_ffprobe_cache(&mut self) -> Result<usize, String> {
        let removed = crate::app::utils::refresh_ffprobe_cache().map_err(|e| e.to_string())?;
        self.start_owned_hd_refresh()?;
        Ok(removed)
    }

    fn refresh_poster_cache_light(&mut self) -> Result<usize, String> {
        crate::app::cache::refresh_poster_cache_light().map_err(|e| e.to_string())
    }

    fn refresh_owned_scan(&mut self) {
        self.owned_rx = None;
        self.owned_keys = None;
        self.owned_hd_keys = None;
        self.owned_modified = None;
        for row in &mut self.rows {
            row.owned = false;
            row.owned_modified = None;
        }
        self.mark_dirty();
        self.owned_scan_in_progress = false;
        self.record_owned_message("Refreshing owned scan…");
        self.start_owned_scan();
    }
}

fn imdb_id_from_guid(guid: &str) -> Option<String> {
    let lower = guid.to_ascii_lowercase();
    let pos = lower.find("tt")?;
    let mut id = String::from("tt");
    for ch in guid[pos + 2..].chars() {
        if ch.is_ascii_digit() {
            id.push(ch);
        } else {
            break;
        }
    }
    if id.len() > 2 {
        Some(id)
    } else {
        None
    }
}

#[derive(Deserialize)]
struct OmdbResponse {
    #[serde(rename = "Response")]
    response: String,
    #[serde(rename = "imdbRating")]
    imdb_rating: Option<String>,
    #[serde(rename = "Error")]
    error: Option<String>,
}

fn fetch_rating_from_omdb(
    api_key: String,
    imdb_id: Option<String>,
    title: String,
    year: Option<i32>,
) -> RatingState {
    if imdb_id.is_none() && title.trim().is_empty() {
        return RatingState::NotFound;
    }

    let client = match reqwest::blocking::Client::builder()
        .user_agent("pex/rating-fetch")
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => return RatingState::Error(format!("client: {err}")),
    };

    let url = if let Some(id) = imdb_id {
        format!("https://www.omdbapi.com/?i={}&apikey={}", id, api_key)
    } else {
        let mut url = format!(
            "https://www.omdbapi.com/?t={}&apikey={}",
            encode(title.trim()),
            api_key
        );
        if let Some(y) = year {
            url.push_str(&format!("&y={}", y));
        }
        url
    };

    let resp = match client.get(&url).send() {
        Ok(r) => r,
        Err(err) => return RatingState::Error(format!("network: {err}")),
    };
    if !resp.status().is_success() {
        return RatingState::Error(format!("HTTP {}", resp.status()));
    }
    let text = match resp.text() {
        Ok(t) => t,
        Err(err) => return RatingState::Error(format!("read: {err}")),
    };

    let parsed: OmdbResponse = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(err) => return RatingState::Error(format!("parse: {err}")),
    };

    if parsed.response.eq_ignore_ascii_case("true") {
        if let Some(r) = parsed.imdb_rating {
            if r.trim().is_empty() || r.trim().eq_ignore_ascii_case("N/A") {
                RatingState::NotFound
            } else if let Ok(val) = r.trim().parse::<f32>() {
                RatingState::Success(format!("IMDb {:.1}/10", val))
            } else {
                RatingState::Success(format!("IMDb {}", r.trim()))
            }
        } else {
            RatingState::NotFound
        }
    } else if let Some(err) = parsed.error {
        if err.to_ascii_lowercase().contains("not found") {
            RatingState::NotFound
        } else {
            RatingState::Error(err)
        }
    } else {
        RatingState::NotFound
    }
}

impl eframe::App for PexApp {
    fn update(&mut self, ctx: &eg::Context, _frame: &mut eframe::Frame) {
        // Keep frames moving so Windows never flags "Not Responding"
        ctx.request_repaint();

        // First frame
        if !self.did_init {
            if !self.setup_checked {
                self.run_setup_checks();
            }

            if !self.setup_errors.is_empty() {
                self.render_setup_gate(ctx);
                return;
            }

            self.load_prefs();
            self.prefs_dirty = false;
            self.did_init = true;
            self.loading_message = if self.setup_warnings.is_empty() {
                "Stage 1/4 - Setup complete. Loading saved preferences.".into()
            } else {
                "Stage 1/4 - Setup complete with warnings (see Advanced menu).".into()
            };
            self.heartbeat_last = Instant::now();
            self.heartbeat_dots = 0;

            // Kick off poster prep first (Stage 2), then owned scan (Stage 3)
            self.start_poster_prep();
            self.start_owned_scan();
        }

        // Drive warm-up progress
        self.poll_prep(ctx);
        self.poll_owned_scan(ctx);

        // Keep prefetch draining while it's running
        if self.prefetch_started && self.loading_progress < 1.0 {
            self.poll_prefetch_done(ctx);
        }

        self.poll_rating_updates();

        // If warm-up not finished, show calm splash and return
        if self.boot_phase != types::BootPhase::Ready {
            eg::CentralPanel::default()
                .frame(eg::Frame::default().inner_margin(eg::Margin::symmetric(4.0, 6.0)))
                .show(ctx, |ui| {
                    self.ui_render_splash(ui);
                });
            return;
        }

        // If prefetch finished, swap phase
        if self.prefetch_started
            && self.loading_progress >= 1.0
            && !self.rows.is_empty()
            && !matches!(self.phase, types::Phase::Ready)
        {
            self.set_phase(types::Phase::Ready);
            let message = self.stage4_complete_message.clone().unwrap_or_else(|| {
                "Stage 4/4 - Artwork cache ready (all posters processed).".into()
            });
            self.stage4_complete_message = Some(message.clone());
            if !self.owned_scan_in_progress {
                self.set_status(message);
            }
        }

        // Soft heartbeat ticker for subtle activity (optional)
        if (self.rows.is_empty() || (self.prefetch_started && self.loading_progress < 1.0))
            && self.heartbeat_last.elapsed() >= Duration::from_millis(250)
        {
            self.heartbeat_last = Instant::now();
            self.heartbeat_dots = (self.heartbeat_dots + 1) % 4;
        }

        // --- NEW: Right-side detail panel (shown when selected) ---
        self.ui_render_detail_panel(ctx);

        // ---- Main UI ----
        eg::CentralPanel::default().show(ctx, |ui| {
            // Top bar (range/search/sort/workers/owned)
            self.ui_render_topbar(ui);

            // Channel & genre filter popups (separate windows)
            self.ui_render_channel_filter_popup(ctx);
            self.ui_render_genre_filter_popup(ctx);
            self.ui_render_advanced_popup(ctx);

            // Decide whether to show the early splash (before enough textures ready)
            let show_splash = !self.should_show_grid();
            if show_splash {
                // Progress variant of splash
                let done = self.completed + self.failed;
                let inflight = self.in_flight();

                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.heading("Preparing posters…");

                    if !self.loading_message.is_empty() {
                        ui.label(&self.loading_message);
                    }
                    if !self.last_item_msg.is_empty() {
                        ui.monospace(&self.last_item_msg);
                    }

                    let db_phase = if self.prefetch_started {
                        self.loading_progress.max(0.02)
                    } else {
                        let t = ctx.input(|i| i.time) as f32;
                        0.18f32.mul_add((t * 0.8) % 1.0, 0.02)
                    };

                    ui.add(eg::ProgressBar::new(db_phase).show_percentage());
                    ui.separator();
                    ui.add(eg::Spinner::new().size(14.0));
                    ui.separator();

                    ui.monospace(format!(
                        "Posters: {done}/{total}  (OK {ok}, Fail {fail}, In-flight {inflight})",
                        total = self.total_targets,
                        ok = self.completed,
                        fail = self.failed,
                        inflight = inflight
                    ));
                    ui.monospace(format!(
                        "Cache: {}",
                        crate::app::cache::cache_dir().display()
                    ));
                });
                return;
            }

            // Grouped grid
            self.ui_render_grouped_grid(ui, ctx);
        });

        self.maybe_save_prefs();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.save_hotset_manifest(180); // remember ~a couple of screens
        if let Err(err) = self.save_prefs() {
            warn!("Failed to persist UI preferences on exit: {err}");
        }
    }
}
