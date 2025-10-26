// src/app/mod.rs — async DB scan + upfront poster prefetch + resized cache + single splash

// ---- Standard lib imports ----
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

// ---- Crates ----
use eframe::egui as eg;
use serde::{de::DeserializeOwned, Deserialize};
use tracing::warn;
use urlencoding::encode;

// ---- Local modules ----
pub mod cache;
use crate::app::cache::find_any_by_key;
use crate::app::filters::{
    parse_owned_cutoff, OWNED_BEFORE_CUTOFF_DEFAULT_STR, OWNED_BEFORE_CUTOFF_DEFAULT_TS,
};
use crate::app::scheduled::ScheduledIndex;
use crate::config::{load_config, local_db_path};

type WorkItem = (usize, String, String, Option<PathBuf>);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

pub mod prep;
pub mod scheduled;
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
const OWNED_AUTO_RETRY_MAX: u8 = 2;
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
    owned_retry_attempts: u8,
    owned_retry_next: Option<Instant>,
    rating_tx: Option<Sender<RatingMsg>>,
    rating_rx: Option<Receiver<RatingMsg>>,
    rating_states: HashMap<String, RatingState>,

    scheduled_index: Option<ScheduledIndex>,

    // search/filter/sort controls
    search_query: String,
    filter_hd_only: bool,
    filter_owned_before_cutoff: bool,
    owned_before_cutoff_ts: u64,
    owned_before_cutoff_input: String,
    owned_before_cutoff_valid: bool,

    // channel filter
    show_channel_filter_popup: bool,
    selected_channels: BTreeSet<String>,
    selected_genres: BTreeSet<String>,
    selected_decades: BTreeSet<i32>,
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
    grid_rows: Vec<Vec<usize>>,
    scroll_to_idx: Option<usize>,
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
            owned_retry_attempts: 0,
            owned_retry_next: None,
            rating_tx: None,
            rating_rx: None,
            rating_states: HashMap::new(),
            scheduled_index: None,

            search_query: String::new(),
            filter_hd_only: false,
            filter_owned_before_cutoff: false,
            owned_before_cutoff_ts: OWNED_BEFORE_CUTOFF_DEFAULT_TS,
            owned_before_cutoff_input: OWNED_BEFORE_CUTOFF_DEFAULT_STR.to_string(),
            owned_before_cutoff_valid: true,

            show_channel_filter_popup: false,
            selected_channels: BTreeSet::new(),
            selected_genres: BTreeSet::new(),
            selected_decades: BTreeSet::new(),
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
            grid_rows: Vec::new(),
            scroll_to_idx: None,

            detail_panel_width: 320.0,
        }
    }
}

// ---------- methods ----------
impl PexApp {
    pub(crate) fn set_owned_cutoff_from_str(&mut self, input: &str) -> bool {
        if let Some(ts) = parse_owned_cutoff(input) {
            self.owned_before_cutoff_ts = ts;
            self.owned_before_cutoff_input = input.trim().to_string();
            self.owned_before_cutoff_valid = true;
            true
        } else {
            self.owned_before_cutoff_input = input.to_string();
            self.owned_before_cutoff_valid = false;
            false
        }
    }

    pub(crate) fn reset_owned_cutoff_to_default(&mut self) {
        self.owned_before_cutoff_ts = OWNED_BEFORE_CUTOFF_DEFAULT_TS;
        self.owned_before_cutoff_input = OWNED_BEFORE_CUTOFF_DEFAULT_STR.to_string();
        self.owned_before_cutoff_valid = true;
    }

    fn handle_keyboard_navigation(&mut self, ctx: &eg::Context) {
        if self.grid_rows.is_empty() {
            return;
        }

        if ctx.memory(|mem| mem.focused().is_some()) {
            return;
        }

        let mut direction: Option<NavDirection> = None;
        ctx.input(|input| {
            if input.key_pressed(eg::Key::ArrowUp) {
                direction = Some(NavDirection::Up);
            } else if input.key_pressed(eg::Key::ArrowDown) {
                direction = Some(NavDirection::Down);
            } else if input.key_pressed(eg::Key::ArrowLeft) {
                direction = Some(NavDirection::Left);
            } else if input.key_pressed(eg::Key::ArrowRight) {
                direction = Some(NavDirection::Right);
            }
        });

        let Some(dir) = direction else {
            return;
        };

        if let Some(sel) = self.selected_idx {
            if !self.is_idx_in_grid(sel) {
                self.selected_idx = None;
            }
        }

        if self.selected_idx.is_none() {
            if let Some(first) = self.grid_rows.first().and_then(|row| row.first()).copied() {
                self.selected_idx = Some(first);
                self.scroll_to_idx = Some(first);
                ctx.request_repaint();
            }
            return;
        }

        let current = self.selected_idx.unwrap();
        if let Some(next) = self.compute_nav_target(current, dir) {
            if next != current {
                self.selected_idx = Some(next);
                self.scroll_to_idx = Some(next);
                ctx.request_repaint();
            }
        }
    }

    fn compute_nav_target(&self, current: usize, dir: NavDirection) -> Option<usize> {
        let (row_i, col_i) = self.find_grid_position(current)?;
        let current_row = self.grid_rows.get(row_i)?;
        match dir {
            NavDirection::Left => {
                if col_i > 0 {
                    Some(current_row[col_i - 1])
                } else if row_i > 0 {
                    self.grid_rows
                        .get(row_i - 1)
                        .and_then(|row| row.last().copied())
                        .or(Some(current))
                } else {
                    Some(current)
                }
            }
            NavDirection::Right => {
                if col_i + 1 < current_row.len() {
                    Some(current_row[col_i + 1])
                } else if row_i + 1 < self.grid_rows.len() {
                    self.grid_rows
                        .get(row_i + 1)
                        .and_then(|row| row.first().copied())
                        .or(Some(current))
                } else {
                    Some(current)
                }
            }
            NavDirection::Up => {
                if row_i == 0 {
                    Some(current)
                } else {
                    let prev_row = &self.grid_rows[row_i - 1];
                    if prev_row.is_empty() {
                        Some(current)
                    } else {
                        let target_col = col_i.min(prev_row.len() - 1);
                        Some(prev_row[target_col])
                    }
                }
            }
            NavDirection::Down => {
                if row_i + 1 >= self.grid_rows.len() {
                    Some(current)
                } else {
                    let next_row = &self.grid_rows[row_i + 1];
                    if next_row.is_empty() {
                        Some(current)
                    } else {
                        let target_col = col_i.min(next_row.len() - 1);
                        Some(next_row[target_col])
                    }
                }
            }
        }
    }

    fn find_grid_position(&self, idx: usize) -> Option<(usize, usize)> {
        for (row_i, row) in self.grid_rows.iter().enumerate() {
            if let Some(col_i) = row.iter().position(|&value| value == idx) {
                return Some((row_i, col_i));
            }
        }
        None
    }

    fn is_idx_in_grid(&self, idx: usize) -> bool {
        self.grid_rows.iter().any(|row| row.contains(&idx))
    }

    fn sync_selection_with_groups(&mut self, groups: &[(i64, Vec<usize>)]) {
        let Some(current) = self.selected_idx else {
            return;
        };

        let still_valid = groups.iter().any(|(_, idxs)| idxs.contains(&current));
        if still_valid {
            return;
        }

        let first_idx = groups.iter().find_map(|(_, idxs)| idxs.first()).copied();
        if let Some(first) = first_idx {
            self.selected_idx = Some(first);
            if self.scroll_to_idx.is_none() {
                self.scroll_to_idx = Some(first);
            }
        } else {
            self.selected_idx = None;
        }
    }
    // ----- tiny helpers ----

    /// Derive a “small” variant cache key from the base key (separate file entry).
    fn small_key(base: &str) -> String {
        format!("{base}__s")
    }

    pub(crate) fn make_owned_key(title: &str, year: Option<i32>) -> String {
        let normalized = utils::normalize_title(title);
        let year = year.or_else(|| utils::find_year_in_str(title));
        year.map_or_else(
            || {
                let digest = md5::compute(normalized.as_bytes());
                let short = format!("{:x}", digest);
                let short = &short[..short.len().min(8)];
                format!("{normalized}:0:{short}")
            },
            |year| format!("{normalized}:{year}"),
        )
    }

    /// Determine whether the airing metadata implies an HD broadcast.
    pub(crate) const fn row_broadcast_hd(row: &PosterRow) -> bool {
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
    fn refresh_scheduled_index(&mut self) {
        match crate::app::scheduled::load_scheduled_index() {
            Ok(index) => {
                if index.is_empty() {
                    self.scheduled_index = None;
                } else {
                    self.scheduled_index = Some(index);
                }
            }
            Err(err) => {
                warn!("Failed to load scheduled recordings: {err}");
                self.scheduled_index = None;
            }
        }
        self.apply_scheduled_flags();
    }

    fn apply_scheduled_flags(&mut self) {
        for row in &mut self.rows {
            row.scheduled = false;
        }
        let Some(index) = self.scheduled_index.as_ref() else {
            return;
        };
        for row in &mut self.rows {
            if index.is_scheduled(row.guid.as_deref(), &row.title, row.year, row.airing) {
                row.scheduled = true;
            }
        }
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
            if cfg.plex_epg_db_source.is_some() {
                self.setup_warnings.push(format!(
                    "Local Plex EPG database not found at {}; it will be copied from plex_epg_db_source on startup.",
                    local_db.display()
                ));
            } else {
                self.setup_errors.push(format!(
                    "Local Plex EPG database not found at {}. Provide plex_epg_db_source in config.json or copy the DB into the db/ folder.",
                    local_db.display()
                ));
            }
        }

        let tmdb_missing = cfg
            .tmdb_api_key
            .as_ref()
            .is_none_or(|k| k.trim().is_empty());
        if tmdb_missing {
            self.setup_warnings
                .push("tmdb_api_key not set; ratings button will be disabled.".into());
        } else if cfg
            .tmdb_api_key
            .as_ref()
            .is_some_and(|k| k.contains("REPLACE_ME") || k.contains("YOUR"))
        {
            self.setup_warnings.push(
                "tmdb_api_key still uses the placeholder value; replace it with your TMDB V3 API key."
                    .into(),
            );
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
            self.rating_tx = Some(tx);
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
        let Some(api_key) = cfg
            .tmdb_api_key
            .filter(|k| !k.trim().is_empty())
            .map(|k| k.trim().to_string())
        else {
            self.rating_states.insert(key, RatingState::MissingApiKey);
            return;
        };

        let imdb_id = row.guid.as_deref().and_then(imdb_id_from_guid);
        let title = row.title.clone();
        let year = row.year;
        let sender = self.ensure_rating_channel();

        self.rating_states.insert(key.clone(), RatingState::Pending);

        std::thread::spawn(move || {
            let state = fetch_rating_from_tmdb(api_key, imdb_id, title, year);
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
        self.grid_rows.clear();
        self.scroll_to_idx = None;
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
        for name in ["owned_all.txt", "owned_hd.txt"] {
            let path = dir.join(name);
            match fs::remove_file(&path) {
                Ok(_) => removed += 1,
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(format!("Failed to remove {}: {err}", path.display()));
                }
            }
        }
        Ok(removed)
    }

    fn clear_owned_cache(&mut self) -> Result<usize, String> {
        let removed = self.clear_owned_cache_files()?;
        self.refresh_owned_scan();
        Ok(removed)
    }

    fn refresh_owned_scan(&mut self) {
        self.refresh_owned_scan_internal(true, true);
    }

    fn refresh_owned_scan_internal(&mut self, force_copy: bool, reset_auto_retry: bool) {
        if reset_auto_retry {
            self.owned_retry_attempts = 0;
            self.owned_retry_next = None;
        }
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

        match crate::app::prep::sync_library_db_from_source(force_copy) {
            Ok(true) => {
                self.record_owned_message("Copied Plex library DB from plex_library_db_source.")
            }
            Ok(false) => {}
            Err(err) => {
                self.record_owned_message(format!("Plex library DB refresh skipped: {err}"))
            }
        }

        self.refresh_scheduled_index();
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
struct TmdbFindResponse {
    #[serde(default)]
    movie_results: Vec<TmdbMovie>,
}

#[derive(Deserialize)]
struct TmdbSearchResponse {
    #[serde(default)]
    results: Vec<TmdbMovie>,
}

#[derive(Deserialize)]
struct TmdbMovie {
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    vote_count: u32,
    #[serde(default)]
    release_date: Option<String>,
}

fn fetch_rating_from_tmdb(
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

    if let Some(id) = imdb_id {
        match tmdb_find_by_imdb(&client, &api_key, &id, year) {
            Ok(Some(state)) => return state,
            Ok(None) => {}
            Err(err) => return err,
        }
    }

    let title = title.trim();
    if title.is_empty() {
        return RatingState::NotFound;
    }

    match tmdb_search_by_title(&client, &api_key, title, year) {
        Ok(Some(state)) => state,
        Ok(None) => RatingState::NotFound,
        Err(err) => err,
    }
}

fn tmdb_find_by_imdb(
    client: &reqwest::blocking::Client,
    api_key: &str,
    imdb_id: &str,
    year: Option<i32>,
) -> Result<Option<RatingState>, RatingState> {
    let url = format!(
        "https://api.themoviedb.org/3/find/{imdb_id}?api_key={api_key}&language=en-US&external_source=imdb_id"
    );
    let body = tmdb_get(client, &url)?;
    let parsed: TmdbFindResponse = parse_tmdb_body(&body)?;
    Ok(extract_tmdb_rating(parsed.movie_results, year))
}

fn tmdb_search_by_title(
    client: &reqwest::blocking::Client,
    api_key: &str,
    title: &str,
    year: Option<i32>,
) -> Result<Option<RatingState>, RatingState> {
    let mut url = format!(
        "https://api.themoviedb.org/3/search/movie?api_key={api_key}&language=en-US&include_adult=false&query={}",
        encode(title)
    );
    if let Some(y) = year {
        url.push_str(&format!("&year={y}"));
    }
    let body = tmdb_get(client, &url)?;
    let parsed: TmdbSearchResponse = parse_tmdb_body(&body)?;
    Ok(extract_tmdb_rating(parsed.results, year))
}

fn tmdb_get(client: &reqwest::blocking::Client, url: &str) -> Result<String, RatingState> {
    let resp = client
        .get(url)
        .send()
        .map_err(|err| RatingState::Error(format!("network: {err}")))?;
    if !resp.status().is_success() {
        return Err(RatingState::Error(format!("HTTP {}", resp.status())));
    }
    resp.text()
        .map_err(|err| RatingState::Error(format!("read: {err}")))
}

fn parse_tmdb_body<T: DeserializeOwned>(body: &str) -> Result<T, RatingState> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| RatingState::Error(format!("parse: {err}")))?;
    if let Some(status) = value.get("status_code") {
        let code = status.as_i64().unwrap_or_default();
        let message = value
            .get("status_message")
            .and_then(|m| m.as_str())
            .unwrap_or("TMDb request failed");
        return Err(RatingState::Error(format!("TMDb error {code}: {message}")));
    }
    serde_json::from_value(value).map_err(|err| RatingState::Error(format!("parse: {err}")))
}

fn extract_tmdb_rating(movies: Vec<TmdbMovie>, target_year: Option<i32>) -> Option<RatingState> {
    let mut fallback: Option<(f32, u32)> = None;

    for movie in movies {
        if movie.vote_average <= 0.0 || movie.vote_count == 0 {
            continue;
        }

        if let Some(target) = target_year {
            if tmdb_release_year(&movie.release_date) == Some(target) {
                return Some(format_tmdb_rating(movie.vote_average, movie.vote_count));
            }
        }

        if fallback.is_none() {
            fallback = Some((movie.vote_average, movie.vote_count));
        }
    }

    fallback.map(|(avg, count)| format_tmdb_rating(avg, count))
}

fn tmdb_release_year(date: &Option<String>) -> Option<i32> {
    let value = date.as_ref()?;
    let year = value.split('-').next()?;
    year.parse().ok()
}

fn format_tmdb_rating(avg: f32, count: u32) -> RatingState {
    let votes = match count {
        0 => "0 votes".to_string(),
        1 => "1 vote".to_string(),
        _ => format!("{count} votes"),
    };
    RatingState::Success(format!("TMDb {:.1}/10 ({})", avg, votes))
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

        if !self.owned_scan_in_progress {
            if let Some(next) = self.owned_retry_next {
                if Instant::now() >= next && self.owned_retry_attempts > 0 {
                    let attempt = self.owned_retry_attempts;
                    self.set_status(format!(
                        "Retrying owned scan ({attempt}/{})…",
                        OWNED_AUTO_RETRY_MAX
                    ));
                    self.refresh_owned_scan_internal(true, false);
                    self.owned_retry_next = None;
                }
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
