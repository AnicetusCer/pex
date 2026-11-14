// src/app/mod.rs — async DB scan + upfront poster prefetch + resized cache + single splash

// ---- Standard lib imports ----
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

// ---- Crates ----
use eframe::egui as eg;
use serde::{de::DeserializeOwned, Deserialize};
use tracing::warn;
use urlencoding::encode;

// ---- Local modules ----
pub mod cache;
use crate::app::filters::{
    parse_owned_cutoff, OWNED_BEFORE_CUTOFF_DEFAULT_STR, OWNED_BEFORE_CUTOFF_DEFAULT_TS,
};
use crate::app::scheduled::ScheduledIndex;
use crate::config::load_config;

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
#[cfg(test)]
mod guide_ui_tests;
pub mod owned;
mod path_checks;
pub mod prefetch;
pub mod prefs;
mod rating_key;
#[cfg(test)]
mod rating_key_tests;
mod setup;
#[cfg(test)]
mod test_fixtures;
#[path = "ui/uimod.rs"] // this is we don't have duplicate file names in within the workspace.
pub mod ui;

// ---- Tunables ----
const WORKER_COUNT: usize = 16; // up from 8 — tune freely (8–32 typical)
const RESIZE_MAX_W: u32 = 320;
const RESIZE_QUALITY: u8 = 75;
const STATUS_EMIT_EVERY_MS: u64 = 120;
const MAX_DONE_PER_FRAME: usize = 12;
const MAX_UPLOADS_PER_FRAME: usize = 4;
const PREWARM_UPLOADS: usize = 24;
pub(crate) const STARTUP_STAGE1_DONE: f32 = 0.12;
pub(crate) const STARTUP_STAGE2_STARTED: f32 = 0.20;
pub(crate) const STARTUP_STAGE2_DONE: f32 = 0.55;
pub(crate) const STARTUP_STAGE3_DONE: f32 = 0.75;
pub(crate) const STARTUP_STAGE4_START: f32 = 0.75;
pub(crate) const OWNED_SCAN_COMPLETE_STATUS: &str =
    "Stage 3/4 - Owned scan complete (Owned and HD badges ready). Finishing artwork cache...";

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OwnedMatchStats {
    pub by_guid: usize,
    pub by_key: usize,
    pub misses: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScheduledMatchStats {
    pub by_guid: usize,
    pub by_title_slot: usize,
    pub misses: usize,
}

pub struct PexApp {
    // data
    rows: Vec<PosterRow>,

    // range
    current_range: DayRange,

    // splash state
    loading_progress: f32,
    startup_progress_floor: f32,
    loading_message: String,

    // poster prep warm-up
    boot_phase: BootPhase,
    prep_rx: Option<Receiver<PrepMsg>>,
    prep_started: bool,

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

    done_rx: Option<Receiver<PrefetchDone>>,

    // --- control flags (UI only; not wired yet) ---
    hide_owned: bool,
    dim_owned: bool,

    // darken strength for dimming (0.10–0.90)
    dim_strength_ui: f32,

    // background owned scan
    owned_rx: Option<Receiver<OwnedMsg>>,
    owned_keys: Option<HashSet<String>>,
    owned_library: Option<types::OwnedLibrary>,
    owned_source: Option<owned::LibrarySource>,
    owned_hd_keys: Option<HashSet<String>>,
    owned_match_stats: OwnedMatchStats,
    owned_scan_in_progress: bool,
    owned_scan_available: bool,
    owned_scan_messages: VecDeque<String>,
    rating_tx: Option<Sender<RatingMsg>>,
    rating_rx: Option<Receiver<RatingMsg>>,
    rating_states: HashMap<String, RatingState>,
    rating_key_editor: Option<rating_key::RatingKeyEditor>,
    has_ratings_key: bool,
    rating_key_feedback: Option<&'static str>,

    scheduled_index: Option<ScheduledIndex>,
    scheduled_match_stats: ScheduledMatchStats,

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
    advanced_paths: path_checks::PathChecks,
    advanced_feedback: Option<String>,
    connection: setup::ConnectionSetup,
    owned_notice: Option<String>,
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
        let cfg = load_config();
        let owned_source = owned::LibrarySource::configured(&cfg);
        let (owned_keys, owned_hd_keys) =
            owned::title_cache::preload(&cache::cache_dir(), owned_source.as_ref()).unzip();
        Self {
            rows: Vec::new(),

            current_range: DayRange::Two,

            loading_progress: 0.0,
            startup_progress_floor: 0.0,
            loading_message: String::new(),

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

            done_rx: None,

            hide_owned: false,
            dim_owned: false,
            dim_strength_ui: 0.8, // stronger dimming by default

            owned_rx: None,
            owned_keys,
            owned_hd_keys,
            owned_library: None,
            owned_source,
            owned_match_stats: OwnedMatchStats::default(),
            owned_scan_in_progress: false,
            owned_scan_available: false,
            owned_scan_messages: VecDeque::new(),
            rating_tx: None,
            rating_rx: None,
            rating_states: HashMap::new(),
            rating_key_editor: None,
            has_ratings_key: cfg
                .tmdb_api_key
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            rating_key_feedback: None,
            scheduled_index: None,
            scheduled_match_stats: ScheduledMatchStats::default(),

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
            advanced_paths: Default::default(),
            advanced_feedback: None,
            connection: setup::ConnectionSetup::from_config(cfg),
            owned_notice: None,
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

        let mut visible = groups
            .iter()
            .flat_map(|(_, indices)| indices.iter().copied());
        let same_film = self.rows.get(current).and_then(|previous| {
            visible
                .clone()
                .find(|&i| self.rows[i].film_key == previous.film_key)
        });
        self.selected_idx = same_film.or_else(|| visible.next());
        self.scroll_to_idx = self.selected_idx;
    }
    // ----- tiny helpers ----

    /// Derive a “small” variant cache key from the base key (separate file entry).
    fn small_key(base: &str) -> String {
        format!("{base}__s")
    }
}

impl PexApp {
    fn apply_scheduled_flags(&mut self) {
        if let Some(index) = &self.scheduled_index {
            self.scheduled_match_stats = index.apply(&mut self.rows);
        } else {
            self.scheduled_match_stats = ScheduledMatchStats {
                misses: self.rows.len(),
                ..Default::default()
            };
            for row in &mut self.rows {
                row.scheduled = Default::default();
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

    const fn set_startup_progress_floor(&mut self, progress: f32) {
        self.startup_progress_floor = self.startup_progress_floor.max(progress.clamp(0.0, 1.0));
    }

    fn startup_progress(&self) -> f32 {
        let mut progress = self.startup_progress_floor;
        if self.prefetch_started {
            let prefetch_ratio = if self.total_targets == 0 {
                1.0
            } else {
                (self.completed + self.failed) as f32 / self.total_targets as f32
            }
            .clamp(0.0, 1.0);
            progress = progress
                .max((1.0 - STARTUP_STAGE4_START).mul_add(prefetch_ratio, STARTUP_STAGE4_START));
        }
        progress.clamp(0.0, 1.0)
    }

    const fn in_flight(&self) -> usize {
        self.total_targets
            .saturating_sub(self.completed + self.failed)
    }

    fn should_show_grid(&self) -> bool {
        !self.rows.is_empty()
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
        if matches!(
            self.rating_states.get(&row.film_key),
            Some(RatingState::Pending)
        ) {
            return;
        }
        let lookup = RatingLookup::from(row);
        let cfg = load_config();
        let Some(api_key) = cfg
            .tmdb_api_key
            .filter(|k| !k.trim().is_empty())
            .map(|k| k.trim().to_string())
        else {
            self.open_rating_key_editor(Some(lookup));
            return;
        };
        self.start_rating_lookup(lookup, api_key);
    }

    fn start_rating_lookup(&mut self, lookup: RatingLookup, api_key: String) {
        let RatingLookup {
            key,
            title,
            year,
            imdb_id,
        } = lookup;
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
        self.done_rx = None;
        self.rows.clear();
        self.total_targets = 0;
        self.completed = 0;
        self.failed = 0;
        self.loading_progress = 0.0;
        self.startup_progress_floor = 0.0;
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
                Self::spawn_channel_icon_prefetch(vec![url.to_string()], ctx.clone());
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

    fn spawn_channel_icon_prefetch(urls: Vec<String>, ctx: eg::Context) {
        std::thread::spawn(move || {
            for url in urls {
                if crate::app::cache::ensure_channel_icon(&url).is_ok() {
                    ctx.request_repaint();
                }
            }
        });
    }

    fn refresh_owned_scan(&mut self) {
        self.start_owned_scan(true);
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

struct RatingLookup {
    key: String,
    title: String,
    year: Option<i32>,
    imdb_id: Option<String>,
}

impl From<&PosterRow> for RatingLookup {
    fn from(row: &PosterRow) -> Self {
        Self {
            key: row.film_key.clone(),
            title: row.title.clone(),
            year: row.year,
            imdb_id: row.guid.as_deref().and_then(imdb_id_from_guid),
        }
    }
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
        Err(_) => return RatingState::Error("Could not start the TMDb network client.".into()),
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
        "https://api.themoviedb.org/3/find/{imdb_id}?api_key={}&language=en-US&external_source=imdb_id",
        encode(api_key)
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
        "https://api.themoviedb.org/3/search/movie?api_key={}&language=en-US&include_adult=false&query={}",
        encode(api_key),
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
    let resp = client.get(url).send().map_err(|err| {
        let message = if err.is_timeout() {
            "TMDb timed out. Try again."
        } else {
            "Could not reach TMDb. Check your connection and try again."
        };
        RatingState::Error(message.into())
    })?;
    if matches!(resp.status().as_u16(), 401 | 403) {
        return Err(RatingState::KeyRejected);
    }
    if !resp.status().is_success() {
        return Err(RatingState::Error(format!(
            "TMDb returned HTTP {}.",
            resp.status().as_u16()
        )));
    }
    resp.text()
        .map_err(|_| RatingState::Error("Could not read the TMDb response. Try again.".into()))
}

fn parse_tmdb_body<T: DeserializeOwned>(body: &str) -> Result<T, RatingState> {
    // Responses and serde type errors can quote arbitrary input, including request data.
    let invalid_response = |_| RatingState::Error("TMDb returned an unreadable response.".into());
    let value: serde_json::Value = serde_json::from_str(body).map_err(invalid_response)?;
    if value.get("status_code").is_some() {
        return Err(RatingState::Error(
            "TMDb could not complete the rating request.".into(),
        ));
    }
    serde_json::from_value(value).map_err(invalid_response)
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
        self.update_ui(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist_on_exit();
    }
}

impl PexApp {
    pub(crate) fn update_ui(&mut self, ctx: &eg::Context) {
        self.update_guide_ui(ctx);
        self.ui_render_rating_key(ctx);
        // Poll workers while they have results to deliver. Idle guides only need
        // a periodic clock tick; egui wakes immediately for input.
        let busy = self.connection.checking()
            || self.prep_rx.is_some()
            || self.owned_scan_in_progress
            || self.in_flight() > 0
            || self.prefs_dirty
            || self
                .rating_states
                .values()
                .any(|s| matches!(s, RatingState::Pending));
        ctx.request_repaint_after(if busy {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(30)
        });
    }

    fn update_guide_ui(&mut self, ctx: &eg::Context) {
        if !self.setup_checked {
            self.run_setup_checks();
        }

        // First frame
        if !self.did_init {
            if !self.setup_checked || !self.setup_errors.is_empty() {
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
            self.set_startup_progress_floor(STARTUP_STAGE1_DONE);

            // Kick off poster prep first (Stage 2), then owned scan (Stage 3)
            self.start_poster_prep();
            self.start_owned_scan(false);
        }

        // Drive warm-up progress
        self.poll_prep(ctx);
        self.poll_owned_scan(ctx);

        // Keep prefetch draining while it's running
        if self.prefetch_started && self.loading_progress < 1.0 {
            self.poll_prefetch_done(ctx);
        }

        self.poll_rating_updates();
        if self.connection.open {
            self.render_setup_gate(ctx);
            return;
        }

        // A completed owned scan can precede guide data. Artwork is optional.
        if self.prep_rx.is_some() && !self.should_show_grid() {
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

        // --- NEW: Right-side detail panel (shown when selected) ---
        self.ui_render_detail_panel(ctx);

        // ---- Main UI ----
        eg::CentralPanel::default().show(ctx, |ui| {
            // Top bar (range/search/sort/workers/owned)
            self.ui_render_topbar(ui);
            if let Some(notice) = self.owned_notice.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(notice);
                    if self.owned_scan_available
                        && ui
                            .add_enabled(
                                !self.owned_scan_in_progress,
                                eg::Button::new("Retry owned scan"),
                            )
                            .clicked()
                    {
                        self.refresh_owned_scan();
                    }
                });
            }
            for warning in &self.setup_warnings {
                ui.label(warning);
            }

            // Channel & genre filter popups (separate windows)
            self.ui_render_channel_filter_popup(ctx);
            self.ui_render_genre_filter_popup(ctx);
            self.ui_render_advanced_popup(ctx);

            if self.prefetch_started && self.loading_progress >= 1.0 && self.rows.is_empty() {
                ui.label("No films in the programme guide.");
                return;
            }

            // Grouped grid
            self.ui_render_grouped_grid(ui, ctx);
        });

        self.maybe_save_prefs();
    }

    fn persist_on_exit(&mut self) {
        let _ = self.save_hotset_manifest(180); // remember ~a couple of screens
        if let Err(err) = self.save_prefs() {
            warn!("Failed to persist UI preferences on exit: {err}");
        }
    }
}

#[cfg(test)]
mod tmdb_http_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    const FIXTURE_KEY: &str = "example&key=abcdef";

    fn error_message(error: RatingState) -> String {
        match error {
            RatingState::Error(message) => message,
            other => panic!("Expected a rating error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_http_errors_without_showing_the_request_url_or_server_text() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let local = server.local_addr().unwrap();
        let reply = std::thread::spawn(move || {
            for status in [401, 403, 500] {
                let (mut stream, _) = server.accept().unwrap();
                let mut request = BufReader::new(&stream);
                let mut line = String::new();
                request.read_line(&mut line).unwrap();
                assert!(line.contains("api_key=example%26key%3Dabcdef"));
                loop {
                    line.clear();
                    if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                write!(stream, "HTTP/1.1 {status} {FIXTURE_KEY}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{FIXTURE_KEY}", FIXTURE_KEY.len()).unwrap();
            }
        });
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let url = format!("http://{local}/find/tt123?api_key={}", encode(FIXTURE_KEY));
        for _ in 0..2 {
            assert!(matches!(
                tmdb_get(&client, &url),
                Err(RatingState::KeyRejected)
            ));
        }
        let message = error_message(tmdb_get(&client, &url).unwrap_err());
        assert!(message.contains("HTTP 500"));
        assert!(!message.contains("abcdef") && !message.contains("127.0.0.1"));
        reply.join().unwrap();
        let message = error_message(tmdb_get(&client, &url).unwrap_err());
        assert!(!message.contains("abcdef") && !message.contains("127.0.0.1"));
    }

    #[test]
    fn response_parsing_never_displays_untrusted_values() {
        for body in [
            format!(r#"{{"status_code":7,"status_message":"{FIXTURE_KEY}"}}"#),
            format!(r#"{{"results":[{{"vote_average":"{FIXTURE_KEY}"}}]}}"#),
            format!("malformed: {FIXTURE_KEY}"),
        ] {
            let error = parse_tmdb_body::<TmdbSearchResponse>(&body).err().unwrap();
            assert!(!error_message(error).contains("abcdef"));
        }
        let result: TmdbFindResponse =
            parse_tmdb_body(r#"{"movie_results":[{"vote_average":7.2,"vote_count":4}]}"#).unwrap();
        assert!(
            matches!(extract_tmdb_rating(result.movie_results, None), Some(RatingState::Success(text)) if text == "TMDb 7.2/10 (4 votes)")
        );
    }
}
