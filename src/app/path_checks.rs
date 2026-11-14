use crate::{app::cache, config};
use eframe::egui as eg;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

struct CheckedPath {
    label: &'static str,
    path: Option<PathBuf>,
    available: bool,
}

impl CheckedPath {
    fn check(label: &'static str, path: Option<PathBuf>) -> Self {
        let available = path.as_ref().is_some_and(|p| p.exists());
        Self {
            label,
            path,
            available,
        }
    }
}

struct Report {
    paths: Vec<CheckedPath>,
}

impl Report {
    fn collect() -> Self {
        let cfg = config::load_config();
        let (library_label, library_path) = match cfg.library_snapshot_dir {
            Some(dir) => ("Snapshot folder", Some(dir)),
            None => ("Library source", cfg.plex_library_db_source),
        };
        Self {
            paths: vec![
                CheckedPath::check("Config file", config::discover_config_path()),
                CheckedPath::check("EPG source", cfg.plex_epg_db_source),
                CheckedPath::check("EPG mirror", Some(config::local_db_path())),
                CheckedPath::check(library_label, library_path),
                CheckedPath::check("Library mirror", Some(config::local_library_db_path())),
                CheckedPath::check("Cache root", Some(cache::cache_dir())),
            ],
        }
    }
}

#[derive(Default)]
pub(super) struct PathChecks {
    report: Option<Report>,
    pending: Option<Receiver<Report>>,
    failed: bool,
}

impl PathChecks {
    pub(super) fn start(&mut self, ctx: &eg::Context) {
        // Reopening Advanced while a disconnected share is timing out reuses the same check.
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.failed = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Report::collect());
            ctx.request_repaint();
        });
    }

    pub(super) fn show(&mut self, ui: &mut eg::Ui) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(report) => {
                    self.report = Some(report);
                    self.pending = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.failed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(report) = &self.report {
            for p in &report.paths {
                if let Some(path) = &p.path {
                    let suffix = if p.available { "" } else { " (unavailable)" };
                    let color = if p.available {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().warn_fg_color
                    };
                    ui.colored_label(color, format!("{}: {}{suffix}", p.label, path.display()));
                } else {
                    ui.weak(format!("{}: not configured", p.label));
                }
            }
        }
        if self.failed {
            ui.label("The path check did not finish. Refresh to try again.");
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    eg::Button::new("Refresh path status"),
                )
                .clicked()
            {
                self.start(ui.ctx());
            }
            if self.pending.is_some() {
                ui.spinner();
                ui.label("Checking paths…");
            }
        });
    }
}
