// src/app/mod.rs — async DB scan + upfront poster prefetch + resized cache + single splash

// ---- Standard lib imports ----
use std::path::{PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};
use std::collections::HashSet;

// ---- Crates ----
use eframe::egui as eg;

// ---- Local modules ----
pub mod cache;
use crate::app::cache::find_any_by_key;

type WorkItem = (usize, String, String, Option<PathBuf>);

pub mod utils;
pub mod prep;
pub mod types;
pub use types::{
    OwnedMsg, PrepItem, PrepMsg, PrefetchDone,
    Phase, BootPhase, PosterState, DayRange, SortKey, PosterRow
};
pub mod prefetch;
pub mod owned;
pub mod ui;
pub mod prefs;
pub mod gfx;
pub mod filters;
pub mod detail;

// ---- Tunables ----
const WORKER_COUNT: usize = 16;        // up from 8 — tune freely (8–32 typical)
const RESIZE_MAX_W: u32 = 320;
const RESIZE_QUALITY: u8 = 75;
const SHOW_GRID_EARLY: bool = true;
const MIN_READY_BEFORE_GRID: usize = 24;
const STATUS_EMIT_EVERY_MS: u64 = 120;
const MAX_DONE_PER_FRAME: usize = 12;
const MAX_UPLOADS_PER_FRAME: usize = 4;
const PREWARM_UPLOADS: usize = 24;

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

    // search/filter/sort controls
    search_query: String,

    // channel filter
    show_channel_filter_popup: bool,
    selected_channels: std::collections::BTreeSet<String>,

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
            dim_strength_ui: 0.6, // sensible default, darker not lighter

            owned_rx: None,
            owned_keys: None,
            owned_hd_keys: None,

            search_query: String::new(),

            show_channel_filter_popup: false,
            selected_channels: std::collections::BTreeSet::new(),

            sort_key: SortKey::Time,
            sort_desc: false,

            poster_width_ui: 140.0, // matches current card_w
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

fn make_owned_key(title: &str, year: Option<i32>) -> String {
    format!("{}:{}", utils::normalize_title(title), year.unwrap_or_default())
}

fn load_owned_hd_sidecar() -> Option<HashSet<String>> {
    use std::{collections::HashSet, fs};
    let path = crate::app::cache::cache_dir().join("owned_hd.txt");
    let text = fs::read_to_string(path).ok()?;
    let mut set = HashSet::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        set.insert(line.to_owned());
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
            match crate::app::gfx::load_texture_from_path(ctx, &path.to_string_lossy(), &row.key) {
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
    let max_bucket_opt: Option<i64> = match self.current_range {
        DayRange::Two => Some(now_bucket + 2),
        DayRange::Four => Some(now_bucket + 4),
        DayRange::Five => Some(now_bucket + 5),
        DayRange::Seven => Some(now_bucket + 7),
        DayRange::Fourteen => Some(now_bucket + 14),
    };

    let targets: Vec<usize> = self.rows.iter().enumerate()
        .filter_map(|(idx, row)| {
            let b = row.airing.map(utils::day_bucket)?;
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

// ---- status/phase helpers ----
fn set_phase(&mut self, phase: Phase) {
    self.phase = phase;
    self.phase_started = Instant::now();
}

fn ready_count(&self) -> usize {
    self.rows.iter().filter(|r| r.tex.is_some()).count()
}

const fn in_flight(&self) -> usize {
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

}

impl eframe::App for PexApp {
    fn update(&mut self, ctx: &eg::Context, _frame: &mut eframe::Frame) {
        // Keep frames moving so Windows never flags "Not Responding"
        ctx.request_repaint();

        // First frame
        if !self.did_init {
            self.load_prefs();
            self.prefs_dirty = false;
            self.did_init = true;
            self.loading_message = "Starting…".into();
            self.heartbeat_last = Instant::now();
            self.heartbeat_dots = 0;

            // Kick off owned scan (non-blocking) + DB warm-up
            self.start_owned_scan();
            self.start_poster_prep();
        }

        // Drive warm-up progress
        self.poll_prep(ctx);
        self.poll_owned_scan(ctx);

        // Keep prefetch draining while it's running
        if self.prefetch_started && self.loading_progress < 1.0 {
            self.poll_prefetch_done(ctx);
        }

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
        if self.prefetch_started && self.loading_progress >= 1.0 && !self.rows.is_empty()
            && !matches!(self.phase, types::Phase::Ready)
        {
            self.set_phase(types::Phase::Ready);
            self.set_status("All posters processed.");
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

            // Channel filter popup (separate window)
            self.ui_render_channel_filter_popup(ctx);

            // Decide whether to show the early splash (before enough textures ready)
            let show_splash = !self.should_show_grid();
            if show_splash {
                // Progress variant of splash
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
                        0.18f32.mul_add((t * 0.8) % 1.0, 0.02)
                    };

                    ui.add(eg::ProgressBar::new(db_phase).show_percentage());
                    ui.separator();
                    ui.add(eg::Spinner::new().size(14.0));
                    ui.separator();

                    ui.monospace(format!(
                        "Posters: {done}/{total}  (OK {ok}, Fail {fail}, In-flight {inflight})",
                        total = self.total_targets, ok = self.completed, fail = self.failed, inflight = inflight
                    ));
                    ui.monospace(format!("Cache: {}", crate::app::cache::cache_dir().display()));
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
        self.save_prefs();
    }
}
