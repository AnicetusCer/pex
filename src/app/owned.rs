// src/app/owned.rs
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use eframe::egui as eg;
use tracing::warn;

use crate::app::types::OwnedMsg;
use crate::config::load_config;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct OwnedManifest {
    dirs: HashMap<String, DirSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DirSnapshot {
    mtime: Option<u64>,
    files: Vec<FileSnapshot>,
    subdirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileSnapshot {
    key: String,
    hd: bool,
    modified: Option<u64>,
    #[serde(default)]
    title_hint: Option<String>,
    #[serde(default)]
    path: String,
}

impl OwnedManifest {
    fn load() -> Self {
        let path = Self::path();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(err) if err.kind() == ErrorKind::NotFound => Self::default(),
            Err(err) => {
                warn!("Failed to read owned manifest {}: {err}", path.display());
                Self::default()
            }
        }
    }

    fn save(&self) -> io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let data =
            serde_json::to_vec_pretty(self).map_err(|err| io::Error::new(ErrorKind::Other, err))?;
        fs::write(&tmp, data)?;
        fs::rename(tmp, path)
    }

    fn save_if_changed(&self, previous: &Self) -> io::Result<bool> {
        if self == previous {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }
    fn path() -> PathBuf {
        crate::app::cache::cache_dir().join("owned_manifest.json")
    }

    fn insert_snapshot(&mut self, dir: String, snapshot: DirSnapshot) {
        self.dirs.insert(dir, snapshot);
    }

    fn get(&self, dir: &str) -> Option<&DirSnapshot> {
        self.dirs.get(dir)
    }

    fn rebuild_hd_flags(
        &mut self,
    ) -> Result<
        (
            HashSet<String>,
            HashSet<String>,
            HashMap<String, Option<u64>>,
            bool,
        ),
        String,
    > {
        use std::path::Path;

        let mut owned: HashSet<String> = HashSet::new();
        let mut hd_keys: HashSet<String> = HashSet::new();
        let mut owned_dates: HashMap<String, Option<u64>> = HashMap::new();
        let mut changed = false;

        for snapshot in self.dirs.values_mut() {
            for file in snapshot.files.iter_mut() {
                if file.path.trim().is_empty() {
                    return Err(
                        "Owned manifest is missing file paths; run 'Refresh owned scan' once to upgrade."
                            .into(),
                    );
                }

                let path = Path::new(&file.path);
                if let Some(result) = crate::app::utils::is_path_hd(path) {
                    if result != file.hd {
                        file.hd = result;
                        changed = true;
                    }
                }

                accumulate_owned_entry(file, &mut owned, &mut hd_keys, &mut owned_dates);
            }
        }

        Ok((owned, hd_keys, owned_dates, changed))
    }
}

fn path_modified_seconds(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|dur| dur.as_secs())
}

fn reuse_directory(
    dir: &str,
    manifest: &OwnedManifest,
    new_manifest: &mut OwnedManifest,
    owned: &mut HashSet<String>,
    hd_keys: &mut HashSet<String>,
    owned_dates: &mut HashMap<String, Option<u64>>,
) {
    if let Some(snapshot) = manifest.get(dir) {
        for file in &snapshot.files {
            owned.insert(file.key.clone());
            if file.hd {
                hd_keys.insert(file.key.clone());
            }
            owned_dates.insert(file.key.clone(), file.modified);

            if let Some(title) = &file.title_hint {
                let alt_key = crate::app::PexApp::make_owned_key(title, None);
                if alt_key != file.key {
                    owned.insert(alt_key.clone());
                    if file.hd {
                        hd_keys.insert(alt_key.clone());
                    }
                    owned_dates.insert(alt_key, file.modified);
                }
            }
        }
        new_manifest.insert_snapshot(dir.to_owned(), snapshot.clone());
        for sub in &snapshot.subdirs {
            reuse_directory(sub, manifest, new_manifest, owned, hd_keys, owned_dates);
        }
    }
}

fn scan_directory(
    dir: &Path,
    manifest: &OwnedManifest,
    new_manifest: &mut OwnedManifest,
    owned: &mut HashSet<String>,
    hd_keys: &mut HashSet<String>,
    owned_dates: &mut HashMap<String, Option<u64>>,
    tx: &Sender<OwnedMsg>,
) {
    let dir_str = dir.to_string_lossy().to_string();
    let mtime = path_modified_seconds(dir);

    if let Some(snapshot) = manifest.get(&dir_str) {
        if snapshot.mtime == mtime {
            let _ = tx.send(OwnedMsg::Info(format!(
                "Stage 3/4 - Owned scan: reusing snapshot for {}",
                dir.display()
            )));
            reuse_directory(&dir_str, manifest, new_manifest, owned, hd_keys, owned_dates);
            return;
        }
    }

    let _ = tx.send(OwnedMsg::Info(format!(
        "Stage 3/4 - Owned scan: walking {}",
        dir.display()
    )));

    let mut snapshot = DirSnapshot {
        mtime,
        files: Vec::new(),
        subdirs: Vec::new(),
    };

    let read_dir = match fs::read_dir(dir) {
        Ok(iter) => iter,
        Err(err) => {
            warn!("Owned scan: unable to read {}: {err}", dir.display());
            return;
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                warn!("Owned scan: failed entry in {}: {err}", dir.display());
                continue;
            }
        };

        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(err) => {
                warn!(
                    "Owned scan: failed to get type for {}: {err}",
                    path.display()
                );
                continue;
            }
        };

        if file_type.is_dir() {
            let subdir_str = path.to_string_lossy().to_string();
            snapshot.subdirs.push(subdir_str.clone());
            scan_directory(&path, manifest, new_manifest, owned, hd_keys, owned_dates, tx);
            continue;
        }

        if !file_type.is_file() || !is_video_ext(&path) {
            continue;
        }

        let file_mtime = path_modified_seconds(&path);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let year = extract_year_from_filename(stem);
        let title = clean_owned_title(stem, year);
        let key = crate::app::PexApp::make_owned_key(&title, year);

        let alt_key = crate::app::PexApp::make_owned_key(&title, None);

        owned.insert(key.clone());
        if alt_key != key {
            owned.insert(alt_key.clone());
        }

        let hd = crate::app::utils::is_path_hd(&path).unwrap_or(false);
        if hd {
            hd_keys.insert(key.clone());
            if alt_key != key {
                hd_keys.insert(alt_key.clone());
            }
        }

        owned_dates.insert(key.clone(), file_mtime);
        if alt_key != key {
            owned_dates.insert(alt_key.clone(), file_mtime);
        }

        snapshot.files.push(FileSnapshot {
            key,
            hd,
            modified: file_mtime,
            title_hint: Some(title.clone()),
            path: path.to_string_lossy().into_owned(),
        });
    }

    new_manifest.insert_snapshot(dir_str, snapshot);
}

// --------- small helpers (private to this module) ---------
fn is_video_ext(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "mkv" | "mp4" | "avi" | "mov" | "mpg" | "mpeg" | "m4v" | "wmv"
    )
}

fn extract_year_from_filename(stem: &str) -> Option<i32> {
    let mut candidate: Option<i32> = None;
    let mut buffer = String::new();
    let mut in_bracket = false;

    for ch in stem.chars() {
        match ch {
            '(' | '[' | '{' => {
                buffer.clear();
                in_bracket = true;
            }
            ')' | ']' | '}' => {
                if in_bracket && buffer.len() == 4 && buffer.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(val) = buffer.parse::<i32>() {
                        candidate = Some(val);
                    }
                }
                in_bracket = false;
                buffer.clear();
            }
            _ => {
                if in_bracket {
                    buffer.push(ch);
                }
            }
        }
    }

    if candidate.is_some() {
        candidate
    } else {
        crate::app::utils::find_year_in_str(stem)
    }
}

fn clean_owned_title(stem: &str, year: Option<i32>) -> String {
    let mut title = stem.trim().to_string();

    if let Some(year) = year {
        let year_str = year.to_string();
        if let Some(pos) = title.find(&year_str) {
            let mut start = pos;
            while start > 0 {
                let c = title.as_bytes()[start - 1] as char;
                if c == '(' || c == '[' || c == '{' || c.is_whitespace() {
                    start -= 1;
                } else {
                    break;
                }
            }
            let mut end = pos + year_str.len();
            while end < title.len() {
                let c = title.as_bytes()[end] as char;
                if c == ')' || c == ']' || c == '}' || c.is_whitespace() {
                    end += 1;
                } else {
                    break;
                }
            }
            if start < end && end <= title.len() {
                title.replace_range(start..end, "");
            }
        }
    }

    let mut collapsed = String::with_capacity(title.len());
    let mut prev_space = false;
    for ch in title.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(ch);
            prev_space = false;
        }
    }
    title = collapsed.trim().to_string();

    for marker in [" - ", " – ", " — ", " -- ", "- "] {
        if let Some(idx) = title.rfind(marker) {
            let remainder = title[idx + marker.len()..].trim();
            if remainder.len() <= 24 {
                title.truncate(idx);
                break;
            }
        }
    }

    while title
        .chars()
        .last()
        .map(|c| c.is_whitespace() || "-_.,".contains(c))
        .unwrap_or(false)
    {
        title.pop();
    }

    if title.is_empty() {
        stem.trim().to_string()
    } else {
        title
    }
}

fn accumulate_owned_entry(
    file: &FileSnapshot,
    owned: &mut HashSet<String>,
    hd_keys: &mut HashSet<String>,
    owned_dates: &mut HashMap<String, Option<u64>>,
) {
    owned.insert(file.key.clone());
    if file.hd {
        hd_keys.insert(file.key.clone());
    }
    owned_dates.insert(file.key.clone(), file.modified);

    if let Some(title) = &file.title_hint {
        let alt_key = crate::app::PexApp::make_owned_key(title, None);
        if alt_key != file.key {
            owned.insert(alt_key.clone());
            if file.hd {
                hd_keys.insert(alt_key.clone());
            }
            owned_dates.insert(alt_key, file.modified);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_owned_title, extract_year_from_filename};
    use crate::app::PexApp;

    #[test]
    fn strips_year_and_suffix_hyphen() {
        let stem = "Harry Potter and the Goblet of Fire (2005) - 4";
        let year = extract_year_from_filename(stem);
        assert_eq!(year, Some(2005));
        let cleaned = clean_owned_title(stem, year);
        assert_eq!(cleaned, "Harry Potter and the Goblet of Fire");

        let key_file = PexApp::make_owned_key(&cleaned, Some(2005));
        let key_row = PexApp::make_owned_key(
            "Harry Potter and the Goblet of Fire",
            Some(2005),
        );
        assert_eq!(key_file, key_row);
    }

    #[test]
    fn trims_brackets_and_extra_comment() {
        let stem = "Some Film (2000) - TVHD";
        let year = extract_year_from_filename(stem);
        assert_eq!(year, Some(2000));
        let cleaned = clean_owned_title(stem, year);
        assert_eq!(cleaned, "Some Film");
    }

    #[test]
    fn falls_back_when_no_year() {
        let stem = "Example Movie - Director's Cut";
        let year = extract_year_from_filename(stem);
        assert_eq!(year, None);
        let cleaned = clean_owned_title(stem, year);
        assert_eq!(cleaned, "Example Movie");
    }

    #[test]
    fn prefers_trailing_parenthetical_year() {
        let stem = "2012 (2009)";
        let year = extract_year_from_filename(stem);
        assert_eq!(year, Some(2009));
        let cleaned = clean_owned_title(stem, year);
        assert_eq!(cleaned, "2012");

        let key_file = PexApp::make_owned_key(&cleaned, year);
        let key_row = PexApp::make_owned_key("2012", Some(2009));
        assert_eq!(key_file, key_row);
    }
}

fn persist_owned_keys_sidecar(
    cache_dir: &Path,
    owned_keys: &HashSet<String>,
) -> std::io::Result<()> {
    use std::io::Write;
    let path = cache_dir.join("owned_all.txt");
    let mut f = std::fs::File::create(&path)?;
    for k in owned_keys {
        writeln!(f, "{k}")?;
    }
    Ok(())
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
    /// Kick off a non-blocking owned-file scan across library_roots.
    pub(crate) fn start_owned_scan(&mut self) {
        if self.owned_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<OwnedMsg>();
        self.owned_rx = Some(rx);

        // Resolve roots from config and launch the scanner thread.
        let cfg = load_config();
        let roots: Vec<PathBuf> = cfg.library_roots.into_iter().map(PathBuf::from).collect();
        self.owned_scan_in_progress = true;
        self.record_owned_message(format!(
            "Stage 3/4 - Scanning owned library ({} root{}). Powers Owned badges and HD upgrade hints; large libraries may take a while.",
            roots.len(),
            if roots.len() == 1 { "" } else { "s" }
        ));
        self.set_status("Stage 3/4 - Scanning owned library (marks Owned titles and HD upgrades).");
        Self::spawn_owned_scan(tx, roots);
    }

    pub(crate) fn start_owned_hd_refresh(&mut self) -> Result<(), String> {
        if self.owned_scan_in_progress {
            return Err("Another owned-library operation is already running; please wait.".into());
        }

        let manifest = OwnedManifest::load();
        if manifest.dirs.is_empty() {
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
                            let _ = tx.send(Error(format!(
                                "Failed to save owned manifest: {save_err}"
                            )));
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
            let key = row.owned_key.as_str();
            row.owned = keys.contains(key);
            row.owned_modified = modified
                .and_then(|m| m.get(key))
                .and_then(|v| *v);
        }
    }

    pub(crate) fn spawn_owned_scan(tx: Sender<OwnedMsg>, library_roots: Vec<PathBuf>) {
        use OwnedMsg::{Done, Info};

        std::thread::spawn(move || {
            if library_roots.is_empty() {
                let _ = tx.send(Info(
                    "No library_roots in config.json; owned scan skipped.".into(),
                ));
                let _ = tx.send(Done {
                    keys: HashSet::new(),
                    modified: HashMap::new(),
                });
                return;
            }

            let manifest = OwnedManifest::load();
            let mut new_manifest = OwnedManifest::default();
        let mut owned: HashSet<String> = HashSet::new();
        let mut hd_keys: HashSet<String> = HashSet::new();
        let mut owned_dates: HashMap<String, Option<u64>> = HashMap::new();

            for root in &library_roots {
                if !root.exists() {
                    let _ = tx.send(Info(format!("Owned scan: missing root {}", root.display())));
                    continue;
                }

                scan_directory(
                    root,
                    &manifest,
                    &mut new_manifest,
                    &mut owned,
                    &mut hd_keys,
                    &mut owned_dates,
                    &tx,
                );
            }

            let manifest_changed = match new_manifest.save_if_changed(&manifest) {
                Ok(changed) => changed,
                Err(err) => {
                    warn!("Failed to persist owned manifest: {err}");
                    false
                }
            };

            if manifest_changed {
                let cache_dir = crate::app::cache::cache_dir();
                if let Err(err) = persist_owned_keys_sidecar(&cache_dir, &owned) {
                    warn!("Failed to persist owned sidecar: {err}");
                }
                if let Err(err) = persist_owned_hd_sidecar(&cache_dir, &hd_keys) {
                    warn!("Failed to persist owned HD sidecar: {err}");
                }
            }

            let _ = tx.send(Done {
                keys: owned,
                modified: owned_dates,
            });
        });
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
                    self.record_owned_message(format!(
                        "Owned scan complete ({count} titles)."
                    ));
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

