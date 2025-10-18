use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use tracing::warn;

use crate::app::cache;
use crate::app::types::OwnedMsg;
use crate::app::utils;
use crate::app::PexApp;

const OWNED_MANIFEST_VERSION: u32 = 2;

fn default_manifest_version() -> u32 {
    0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OwnedManifest {
    #[serde(default = "default_manifest_version")]
    version: u32,
    dirs: HashMap<String, DirSnapshot>,
}

impl Default for OwnedManifest {
    fn default() -> Self {
        Self {
            version: OWNED_MANIFEST_VERSION,
            dirs: HashMap::new(),
        }
    }
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
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    year_hint: Option<i32>,
}

impl OwnedManifest {
    pub(crate) fn load() -> Self {
        let path = Self::path();
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(mut manifest) => {
                    if manifest.needs_upgrade() {
                        warn!(
                            "Owned manifest {} is outdated; forcing full rescan.",
                            path.display()
                        );
                        Self::default()
                    } else {
                        manifest.version = OWNED_MANIFEST_VERSION;
                        manifest
                    }
                }
                Err(err) => {
                    warn!(
                        "Failed to parse owned manifest {}: {err}. Forcing rebuild.",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) if err.kind() == ErrorKind::NotFound => Self::default(),
            Err(err) => {
                warn!("Failed to read owned manifest {}: {err}", path.display());
                Self::default()
            }
        }
    }

    pub(crate) fn save(&self) -> io::Result<()> {
        let mut manifest = self.clone();
        manifest.version = OWNED_MANIFEST_VERSION;
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&manifest)
            .map_err(|err| io::Error::new(ErrorKind::Other, err))?;
        fs::write(&tmp, data)?;
        fs::rename(tmp, path)
    }

    pub(crate) fn save_if_changed(&self, previous: &Self) -> io::Result<bool> {
        if self == previous {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }

    pub(crate) fn rebuild_hd_flags(
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
                let (modified, size) = utils::file_modified_and_size(path);
                if file.modified != modified {
                    file.modified = modified;
                    changed = true;
                }
                if file.size != size {
                    file.size = size;
                    changed = true;
                }

                if let Some(result) = utils::is_path_hd(path) {
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

    fn path() -> PathBuf {
        cache::cache_dir().join("owned_manifest.json")
    }

    fn insert_snapshot(&mut self, dir: String, snapshot: DirSnapshot) {
        self.dirs.insert(dir, snapshot);
    }

    fn get(&self, dir: &str) -> Option<&DirSnapshot> {
        self.dirs.get(dir)
    }

    fn needs_upgrade(&self) -> bool {
        if self.version < OWNED_MANIFEST_VERSION {
            return true;
        }
        for snapshot in self.dirs.values() {
            for file in &snapshot.files {
                if file.path.trim().is_empty() || file.size.is_none() {
                    return true;
                }
            }
        }
        false
    }
}

pub(crate) struct OwnedScanFs;

impl OwnedScanFs {
    pub(crate) fn spawn_scan(tx: Sender<OwnedMsg>, library_roots: Vec<PathBuf>) {
        thread::spawn(move || {
            use OwnedMsg::{Done, Info};

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
                let cache_dir = cache::cache_dir();
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
            accumulate_owned_entry(file, owned, hd_keys, owned_dates);
        }
        new_manifest.insert_snapshot(dir.to_owned(), snapshot.clone());
        for sub in &snapshot.subdirs {
            reuse_directory(sub, manifest, new_manifest, owned, hd_keys, owned_dates);
        }
    }
}

#[derive(Clone, Copy)]
enum EntryKind {
    Directory,
    VideoFile,
}

struct DirEntryInfo {
    path: PathBuf,
    modified: Option<u64>,
    size: Option<u64>,
    kind: EntryKind,
}

fn snapshot_matches(snapshot: &DirSnapshot, entries: &[DirEntryInfo]) -> bool {
    use std::collections::{HashMap, HashSet};

    let mut actual_dirs: HashSet<String> = HashSet::new();
    let mut actual_files: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();

    for entry in entries {
        match entry.kind {
            EntryKind::Directory => {
                actual_dirs.insert(entry.path.to_string_lossy().to_string());
            }
            EntryKind::VideoFile => {
                actual_files.insert(
                    entry.path.to_string_lossy().to_string(),
                    (entry.modified, entry.size),
                );
            }
        }
    }

    if snapshot.subdirs.len() != actual_dirs.len() {
        return false;
    }
    for subdir in &snapshot.subdirs {
        if !actual_dirs.remove(subdir) {
            return false;
        }
    }
    if !actual_dirs.is_empty() {
        return false;
    }

    if snapshot.files.len() != actual_files.len() {
        return false;
    }
    for file in &snapshot.files {
        match actual_files.remove(&file.path) {
            None => return false,
            Some((modified, size)) => {
                if file.modified != modified {
                    return false;
                }
                match (file.size, size) {
                    (Some(expected), Some(actual)) if expected == actual => {}
                    (Some(_), Some(_)) => return false,
                    (Some(_), None) | (None, Some(_)) => return false,
                    (None, None) => {}
                }
            }
        }
    }

    actual_files.is_empty()
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

    let read_dir = match fs::read_dir(dir) {
        Ok(iter) => iter,
        Err(err) => {
            warn!("Owned scan: unable to read {}: {err}", dir.display());
            return;
        }
    };

    let mut entries: Vec<DirEntryInfo> = Vec::new();
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
            entries.push(DirEntryInfo {
                path,
                modified: None,
                size: None,
                kind: EntryKind::Directory,
            });
            continue;
        }

        if !file_type.is_file() || !is_video_ext(&path) {
            continue;
        }

        let (modified, size) = utils::file_modified_and_size(&path);
        entries.push(DirEntryInfo {
            path,
            modified,
            size,
            kind: EntryKind::VideoFile,
        });
    }

    if let Some(snapshot) = manifest.get(&dir_str) {
        if snapshot.mtime == mtime && snapshot_matches(snapshot, &entries) {
            let _ = tx.send(OwnedMsg::Info(format!(
                "Stage 3/4 - Owned scan: reusing snapshot for {}",
                dir.display()
            )));
            reuse_directory(
                &dir_str,
                manifest,
                new_manifest,
                owned,
                hd_keys,
                owned_dates,
            );
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

    for entry in entries {
        match entry {
            DirEntryInfo {
                kind: EntryKind::Directory,
                path,
                ..
            } => {
                let subdir_str = path.to_string_lossy().to_string();
                snapshot.subdirs.push(subdir_str.clone());
                scan_directory(
                    &path,
                    manifest,
                    new_manifest,
                    owned,
                    hd_keys,
                    owned_dates,
                    tx,
                );
            }
            DirEntryInfo {
                kind: EntryKind::VideoFile,
                path,
                modified,
                size,
            } => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                let year = extract_year_from_filename(stem);
                let title = clean_owned_title(stem, year);
                let key = PexApp::make_owned_key(&title, year);
                let hd = utils::is_path_hd(&path).unwrap_or(false);
                let file_entry = FileSnapshot {
                    key,
                    hd,
                    modified,
                    title_hint: Some(title.clone()),
                    path: path.to_string_lossy().into_owned(),
                    size,
                    year_hint: year,
                };
                accumulate_owned_entry(&file_entry, owned, hd_keys, owned_dates);
                snapshot.files.push(file_entry);
            }
        }
    }

    new_manifest.insert_snapshot(dir_str, snapshot);
}

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
        utils::find_year_in_str(stem)
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
    let mut insert_key = |key: String| {
        owned.insert(key.clone());
        if file.hd {
            hd_keys.insert(key.clone());
        }
        owned_dates.insert(key, file.modified);
    };

    if let Some(title) = &file.title_hint {
        let year_hint = file.year_hint.or_else(|| owned_year_from_key(&file.key));
        for key in PexApp::owned_key_variants(title, year_hint) {
            insert_key(key);
        }
    } else {
        insert_key(file.key.clone());
    }
}

pub(crate) fn persist_owned_keys_sidecar(
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

fn owned_year_from_key(key: &str) -> Option<i32> {
    let mut parts = key.split(':');
    let _title = parts.next()?;
    let year_part = parts.next()?;
    match year_part.parse::<i32>() {
        Ok(y) if y > 0 => Some(y),
        _ => None,
    }
}

pub(crate) fn persist_owned_hd_sidecar(
    cache_dir: &Path,
    hd_keys: &HashSet<String>,
) -> std::io::Result<()> {
    use std::io::Write;
    let path = cache_dir.join("owned_hd.txt");
    let mut f = std::fs::File::create(&path)?;
    for k in hd_keys {
        writeln!(f, "{k}")?;
    }
    Ok(())
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
        let key_row = PexApp::make_owned_key("Harry Potter and the Goblet of Fire", Some(2005));
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
