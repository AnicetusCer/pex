pub(crate) mod owned_scan_fs;
pub(crate) mod owned_scan_plex;

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui as eg;
use tracing::warn;

use crate::app::types::OwnedMsg;
use crate::config::{load_config, OwnedSourceKind};

use self::owned_scan_fs::{
    persist_owned_hd_sidecar, persist_owned_keys_sidecar, OwnedManifest, OwnedScanFs,
};
use self::owned_scan_plex::OwnedScanPlex;

impl crate::app::PexApp {
    /// Kick off a non-blocking owned-file scan across library_roots.
    pub(crate) fn start_owned_scan(&mut self) {
        if self.owned_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<OwnedMsg>();
        self.owned_rx = Some(rx);

        // Resolve config and launch the appropriate scanner thread.
        let cfg = load_config();
        let roots: Vec<PathBuf> = cfg.library_roots.into_iter().map(PathBuf::from).collect();
        self.owned_scan_in_progress = true;

        match cfg.owned_source {
            OwnedSourceKind::Filesystem => {
                self.record_owned_message(format!(
                    "Stage 3/4 - Scanning owned library ({} root{}). Powers Owned badges and HD upgrade hints; large libraries may take a while.",
                    roots.len(),
                    if roots.len() == 1 { "" } else { "s" }
                ));
                self.set_status(
                    "Stage 3/4 - Scanning owned library (marks Owned titles and HD upgrades).",
                );
                OwnedScanFs::spawn_scan(tx, roots);
            }
            OwnedSourceKind::PlexLibrary => {
                self.record_owned_message(
                    "Stage 3/4 - Loading owned titles from the Plex library database.",
                );
                self.set_status("Stage 3/4 - Loading owned titles from Plex (marks Owned titles and HD upgrades).");
                OwnedScanPlex::spawn_scan(tx, roots);
            }
        }
    }

    pub(crate) fn start_owned_hd_refresh(&mut self) -> Result<(), String> {
        if self.owned_scan_in_progress {
            return Err("Another owned-library operation is already running; please wait.".into());
        }

        let cfg = load_config();
        if cfg.owned_source == OwnedSourceKind::PlexLibrary {
            return Err(
                "HD flags are sourced directly from the Plex library database; rerun the owned scan instead."
                    .into(),
            );
        }

        let manifest = OwnedManifest::load();
        if manifest.is_empty() {
            return Err(
                "Owned manifest is empty. Run 'Refresh owned scan' once before refreshing HD flags."
                    .into(),
            );
        }

        let (tx, rx) = std::sync::mpsc::channel::<OwnedMsg>();
        self.owned_rx = Some(rx);
        self.owned_scan_in_progress = true;
        self.record_owned_message("Stage 3/4 - Refreshing HD flags using cached manifest.");
        self.set_status("Stage 3/4 - Refreshing HD flags (re-running ffprobe on owned files).");

        std::thread::spawn(move || {
            use OwnedMsg::{Done, Error, Info};
            let mut manifest = manifest;
            let _ = tx.send(Info(
                "Stage 3/4 - Refreshing HD flags using cached manifest.".into(),
            ));

            match manifest.rebuild_hd_flags() {
                Err(err) => {
                    let _ = tx.send(Error(err));
                }
                Ok((owned, hd_keys, owned_dates, changed)) => {
                    if changed {
                        if let Err(save_err) = manifest.save() {
                            let _ = tx
                                .send(Error(format!("Failed to save owned manifest: {save_err}")));
                            return;
                        }
                    }

                    let cache_dir = crate::app::cache::cache_dir();
                    if let Err(err) = persist_owned_keys_sidecar(&cache_dir, &owned) {
                        warn!("Failed to persist owned sidecar: {err}");
                    }
                    if let Err(err) = persist_owned_hd_sidecar(&cache_dir, &hd_keys) {
                        warn!("Failed to persist owned HD sidecar: {err}");
                    }

                    let _ = tx.send(Done {
                        keys: owned,
                        modified: owned_dates,
                    });
                }
            }
        });

        Ok(())
    }

    /// Apply the owned flags using the computed key set (no-ops if not ready).
    pub(crate) fn apply_owned_flags(&mut self) {
        let Some(keys) = &self.owned_keys else {
            return;
        };
        let modified = self.owned_modified.as_ref();
        for row in &mut self.rows {
            let base_key = row.owned_key.clone();
            let mut matched_key: Option<String> = None;

            for candidate in Self::owned_key_variants(&row.title, row.year) {
                if keys.contains(&candidate) {
                    matched_key = Some(candidate);
                    break;
                }
            }

            if matched_key.is_none() && keys.contains(&base_key) {
                matched_key = Some(base_key.clone());
            }

            if let Some(found) = matched_key {
                row.owned = true;
                row.owned_key = found.clone();
                row.owned_modified = modified.and_then(|m| m.get(&found)).and_then(|v| *v);
            } else {
                row.owned = false;
                row.owned_key = base_key;
                row.owned_modified = None;
            }
        }
    }

    pub(crate) fn owned_key_variants(title: &str, year: Option<i32>) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut variants: Vec<String> = Vec::new();

        let titles = Self::owned_title_variants(title);
        let mut year_candidates: Vec<Option<i32>> = Vec::new();
        if let Some(y) = year {
            for offset in [0, -1, 1] {
                let candidate = y.saturating_add(offset);
                if candidate > 0 {
                    year_candidates.push(Some(candidate));
                }
            }
        }
        year_candidates.push(None);

        for variant_title in titles {
            for candidate_year in &year_candidates {
                let key = Self::make_owned_key(&variant_title, *candidate_year);
                if seen.insert(key.clone()) {
                    variants.push(key);
                }
            }
        }

        if variants.is_empty() {
            let key = Self::make_owned_key(title, year);
            if seen.insert(key.clone()) {
                variants.push(key);
            }
        }

        variants
    }

    fn owned_title_variants(title: &str) -> Vec<String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut titles = Vec::new();
        titles.push(trimmed.to_string());

        if let Some(idx) = trimmed.find(':') {
            let head = trimmed[..idx].trim();
            if !head.is_empty() && head != trimmed {
                titles.push(head.to_string());
            }
        }

        // Handle possessive prefixes like "Lemony Snicket's A Series of ..."
        // so we also match library entries that drop the leading proper name.
        for needle in ["'s ", "’s "] {
            if let Some(pos) = trimmed.find(needle) {
                let before = &trimmed[..pos];
                if !before.contains(' ') {
                    continue;
                }
                let candidate = trimmed[pos + needle.len()..].trim_start();
                if !candidate.is_empty() && candidate != trimmed {
                    titles.push(candidate.to_string());
                }
            }
        }

        // Drop leading English articles so "The Return of Sabata" matches "Return of Sabata".
        let lower = trimmed.to_ascii_lowercase();
        for article in ["the ", "a ", "an "] {
            if lower.starts_with(article) && trimmed.len() > article.len() {
                let candidate = trimmed[article.len()..].trim_start();
                if !candidate.is_empty() && candidate != trimmed {
                    titles.push(candidate.to_string());
                }
            }
        }

        titles
    }

    /// Drain owned-scan messages without blocking the UI thread.
    pub(crate) fn poll_owned_scan(&mut self, _ctx: &eg::Context) {
        use crate::app::types::OwnedMsg::{Done, Error, Info};

        loop {
            let msg = {
                let rx = match self.owned_rx.as_ref() {
                    Some(r) => r,
                    None => return,
                };
                match rx.try_recv() {
                    Ok(m) => m,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.owned_scan_in_progress = false;
                        break;
                    }
                }
            };

            match msg {
                Info(s) => {
                    self.record_owned_message(s.clone());
                    self.owned_scan_in_progress = true;
                    self.set_status(s);
                }
                Error(e) => {
                    let msg = format!("Owned scan error: {e}");
                    self.record_owned_message(msg.clone());
                    self.owned_scan_in_progress = false;
                    self.set_status(msg);
                }
                Done { keys, modified } => {
                    let count = keys.len();
                    self.owned_keys = Some(keys);
                    self.owned_hd_keys = Self::load_owned_hd_sidecar();
                    self.owned_modified = Some(modified);
                    self.apply_owned_flags();
                    self.mark_dirty();
                    self.owned_scan_in_progress = false;
                    self.record_owned_message(format!("Owned scan complete ({count} titles)."));
                    if let Some(msg) = self.stage4_complete_message.clone() {
                        self.set_status(msg);
                    } else {
                        self.set_status(crate::app::OWNED_SCAN_COMPLETE_STATUS);
                    }
                    if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                        self.boot_phase = crate::app::BootPhase::Ready;
                    }
                }
            }
        }
    }
}
