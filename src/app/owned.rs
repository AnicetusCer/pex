// src/app/owned.rs
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use eframe::egui as eg;

use crate::app::types::OwnedMsg;
use crate::config::load_config;

// --------- small helpers (private to this module) ---------
fn is_video_ext(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "mkv" | "mp4" | "avi" | "mov" | "mpg" | "mpeg" | "m4v" | "wmv")
}

fn persist_owned_hd_sidecar(cache_dir: &Path, hd_keys: &HashSet<String>) -> std::io::Result<()> {
    use std::io::Write;
    let path = cache_dir.join("owned_hd.txt");
    let mut f = std::fs::File::create(&path)?;
    for k in hd_keys {
        writeln!(f, "{k}")?;
    }
    Ok(())
}

impl crate::app::PexApp {
    /// Kick off a non-blocking owned-file scan across `library_roots`.
    pub(crate) fn start_owned_scan(&mut self) {
        if self.owned_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<OwnedMsg>();
        self.owned_rx = Some(rx);

        // Resolve roots from config and launch the scanner thread.
        let cfg = load_config();
        let roots: Vec<PathBuf> = cfg.library_roots.into_iter().map(PathBuf::from).collect();
        Self::spawn_owned_scan(tx, roots);
    }

    /// Apply the owned flags using the computed key set (no-ops if not ready).
    pub(crate) fn apply_owned_flags(&mut self) {
        let Some(keys) = &self.owned_keys else { return; };
        for row in &mut self.rows {
            let key = Self::make_owned_key(&row.title, row.year);
            row.owned = keys.contains(&key);
        }
    }

    pub(crate) fn spawn_owned_scan(tx: Sender<OwnedMsg>, library_roots: Vec<PathBuf>) {
        use OwnedMsg::{Done, Info};

        std::thread::spawn(move || {
            if library_roots.is_empty() {
                let _ = tx.send(Info("No library_roots in config.json; owned scan skipped.".into()));
                let _ = tx.send(Done(HashSet::new()));
                return;
            }

            let mut owned: HashSet<String> = HashSet::new();
            let mut hd_keys: HashSet<String> = HashSet::new(); // positive HD detections

            for root in &library_roots {
                let _ = tx.send(Info(format!("Scanning {}", root.display())));
                if !root.exists() {
                    let _ = tx.send(Info(format!("Owned scan: missing root {}", root.display())));
                    continue;
                }

                for e in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
                    if !e.file_type().is_file() { continue; }
                    let p = e.path();
                    if !is_video_ext(p) { continue; }

                    // Build canonical owned key
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                    let year = crate::app::utils::find_year_in_str(stem);
                    let title = year.map(|y| stem.replace(&y.to_string(), " ")).unwrap_or_else(|| stem.to_string());
                    let key = Self::make_owned_key(&title, year);

                    owned.insert(key.clone());

                    // filename heuristic; if it says "HD", mark positive
                    if let Some(hd) = crate::app::utils::is_path_hd(p) {
                        if hd { hd_keys.insert(key); }
                    }
                }

                // Be gentle on massive trees
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            // Persist HD subset (one-per-line) for the UI to load
            let _ = persist_owned_hd_sidecar(&crate::app::cache::cache_dir(), &hd_keys);

            let _ = tx.send(Done(owned));
        });
    }

    /// Drain owned-scan messages without blocking the UI thread.
    pub(crate) fn poll_owned_scan(&mut self, _ctx: &eg::Context) {
        use crate::app::types::OwnedMsg::{Done, Error, Info};

        loop {
            // Limit the immutable borrow of rx to just this call.
            let msg = {
                let rx = match self.owned_rx.as_ref() {
                    Some(r) => r,
                    None => return,
                };
                match rx.try_recv() {
                    Ok(m) => m,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            };

            match msg {
                Info(s) => self.set_status(format!("Owned scan: {s}")),
                Error(e) => self.set_status(format!("Owned scan error: {e}")),
                Done(keys) => {
                    self.owned_keys = Some(keys);
                    self.apply_owned_flags();
                    self.owned_hd_keys = Self::load_owned_hd_sidecar();
                    self.mark_dirty();
                    self.set_status("Owned scan complete.");
                }
            }
        }
    }
}
