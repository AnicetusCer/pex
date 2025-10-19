// src/app/util.rs
use chrono::{Local, TimeZone};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};
pub(crate) fn normalize_title(s: &str) -> String {
    let mut normalized = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\'' | '’' | '‘' | '`' => {
                // Drop apostrophes entirely so “Schindler's” matches “Schindlers”.
            }
            '&' => {
                normalized.push(' ');
                normalized.push_str("and");
                normalized.push(' ');
            }
            ch if ch.is_alphanumeric() => {
                for lower in ch.to_lowercase() {
                    normalized.push(lower);
                }
            }
            _ => {
                normalized.push(' ');
            }
        }
    }

    normalized
        .split_whitespace()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn find_year_in_str(s: &str) -> Option<i32> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            if let Ok(val) = s[i..i + 4].parse::<i32>() {
                if (1900..=2099).contains(&val) {
                    return Some(val);
                }
            }
        }
    }
    None
}

pub(crate) fn day_bucket(ts: SystemTime) -> i64 {
    let secs = ts
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    secs / 86_400
}

pub(crate) const fn weekday_full_from_bucket(bucket: i64) -> &'static str {
    let idx = ((bucket + 4).rem_euclid(7)) as usize; // 1970-01-01 was Thursday
    const NAMES: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    NAMES[idx]
}

pub(crate) const fn civil_from_days(z0: i64) -> (i32, u32, u32) {
    let z = z0 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + (m <= 2) as i64;
    (y as i32, m as u32, d as u32)
}

pub(crate) fn month_short_name(m: u32) -> &'static str {
    const M: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    M[(m.saturating_sub(1)).min(11) as usize]
}

pub(crate) fn ordinal_suffix(d: u32) -> &'static str {
    if (11..=13).contains(&(d % 100)) {
        return "th";
    }
    match d % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

pub(crate) fn format_day_label(bucket: i64) -> String {
    let (_y, m, d) = civil_from_days(bucket);
    let wd = weekday_full_from_bucket(bucket);
    format!("{} {}{} {}", wd, d, ordinal_suffix(d), month_short_name(m))
}

pub(crate) fn hhmm_utc(ts: SystemTime) -> String {
    let secs = ts
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let hm = (secs % 86_400 + 86_400) % 86_400;
    let h = hm / 3600;
    let m = (hm % 3600) / 60;
    format!("{:02}:{:02}", h, m)
}

pub(crate) fn format_owned_timestamp(ts: u64) -> Option<String> {
    Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// Very light hostname extraction for channel hint (no extra deps).
pub(crate) fn host_from_url(u: &str) -> Option<String> {
    let start = u.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &u[start..];
    let end = rest.find('/').unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let host = &rest[..end];
    if host.is_empty() {
        return None;
    }
    Some(host.split('.').next().unwrap_or(host).to_uppercase())
}

pub(crate) fn parse_genres(tags: &str) -> Vec<String> {
    let mut v: Vec<String> = tags
        .split('|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Make a channel label friendlier:
/// - drop leading virtual channel numbers like "006 "
/// - replace '_' and '-' with spaces; collapse spaces
/// - if it looks like a hostname (e.g., "itv.com"), use the primary label ("ITV")
/// - uppercase simple lowercase words (e.g. "itv2" -> "ITV2")
pub fn humanize_channel(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Remove leading digits/spaces like "006 ITV2"
    let mut cut = 0usize;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() || ch.is_whitespace() {
            cut = i + ch.len_utf8();
        } else {
            break;
        }
    }
    if cut > 0 && cut < s.len() {
        s = s[cut..].to_string();
    }

    // Replace separators, collapse spaces
    s = s.replace(['_', '-'], " ");
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    s = s.trim().to_string();

    // If it looks like a hostname, pick a friendly label (e.g., "itv.com" -> "ITV")
    if s.contains('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        let mut parts: Vec<&str> = s.split('.').collect();
        parts.retain(|p| !p.is_empty() && *p != "www");
        if let Some(name) = parts.first() {
            let up = name.to_ascii_uppercase();
            if up.len() >= 2 {
                return up;
            }
        }
    }

    // Uppercase simple lowercase labels
    if s.chars().any(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c.is_whitespace())
    {
        s = s.to_ascii_uppercase();
    }

    // Ensure a space before trailing HD/UHD if missing: "ITVHD" -> "ITV HD", "SKYSPORTSUHD" -> "SKYSPORTS UHD"
    let upper = s.to_ascii_uppercase();
    if upper.ends_with("UHD") && !upper.ends_with(" UHD") && s.len() >= 3 {
        let base = &s[..s.len().saturating_sub(3)];
        s = format!("{} UHD", base.trim_end());
    } else if upper.ends_with("HD") && !upper.ends_with(" HD") && s.len() >= 2 {
        let base = &s[..s.len().saturating_sub(2)];
        s = format!("{} HD", base.trim_end());
    }

    if s.is_empty() {
        "—".into()
    } else {
        s
    }
}

/// Very cheap HD inference from tags/channel.
/// We treat >=720p or “HD/UHD/4K/HDR” as HD. No serde; all substring checks.
pub fn infer_broadcast_hd(tags_genre: Option<&str>, channel: Option<&str>) -> bool {
    // Tags are strongest: UHD/4K/HDR/1080/720 -> HD
    if let Some(tags) = tags_genre {
        let t = tags.to_ascii_lowercase();
        for needle in [
            "2160", "uhd", "4k", "hdr", "1080", "720", " hd ", "(hd)", "[hd]",
        ] {
            if t.contains(needle) {
                return true;
            }
        }
    }

    // Channel-name checks (support "ITV HD", "ITVHD", "ITVHDG", "BBCONEHD", etc.)
    if let Some(ch) = channel {
        let spaced = ch.trim().to_ascii_lowercase();
        if spaced.ends_with(" hd") || spaced.contains(" hd ") {
            return true;
        }
        let compact: String = spaced
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let cc = compact.to_ascii_uppercase();

        // UHD/4K anywhere → HD
        if cc.contains("UHD") || cc.contains("4K") {
            return true;
        }
        // "HD" token near the end (optionally followed by up to 3 region letters: HD, HDA, HDG, HDUK, etc.)
        if let Some(pos) = cc.rfind("HD") {
            if cc.len().saturating_sub(pos + 2) <= 3 {
                return true;
            }
        }
    }

    false
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct ProbeEntry {
    mtime: Option<u64>,
    #[serde(default)]
    size: Option<u64>,
    width: u32,
    height: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct ProbeCache {
    entries: HashMap<String, ProbeEntry>,
}

static FFPROBE_CACHE: Lazy<Mutex<ProbeCache>> = Lazy::new(|| Mutex::new(ProbeCache::load()));
static FFPROBE_CMD: Lazy<RwLock<Option<OsString>>> = Lazy::new(|| RwLock::new(None));
static FFPROBE_AVAILABLE: Lazy<Mutex<Option<bool>>> = Lazy::new(|| Mutex::new(None));

impl ProbeCache {
    fn cache_path() -> std::path::PathBuf {
        crate::app::cache::cache_dir().join("ffprobe_cache.json")
    }

    fn load() -> Self {
        let path = Self::cache_path();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(err) => {
                if err.kind() != ErrorKind::NotFound {
                    warn!("Failed to read ffprobe cache {}: {err}", path.display());
                }
                Self::default()
            }
        }
    }

    fn save(&self) -> io::Result<()> {
        let path = Self::cache_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&tmp, data)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    fn key_for(path: &Path) -> Option<String> {
        Some(path.to_str()?.to_owned())
    }

    fn lookup(&self, key: &str, mtime: Option<u64>, size: Option<u64>) -> Option<ProbeEntry> {
        self.entries.get(key).and_then(|entry| {
            let mtime_matches = entry.mtime == mtime;
            let size_matches = entry.size == size;
            if mtime_matches && size_matches {
                Some(*entry)
            } else {
                None
            }
        })
    }

    fn update(&mut self, key: String, entry: ProbeEntry) {
        self.entries.insert(key, entry);
    }

    fn refresh_stale_entries(&mut self) -> io::Result<usize> {
        use std::path::Path;
        let mut stale_keys: Vec<String> = Vec::new();
        for (key, entry) in self.entries.iter() {
            let path = Path::new(key);
            let (current_mtime, current_size) = file_modified_and_size(path);
            if current_mtime != entry.mtime || current_size != entry.size {
                stale_keys.push(key.clone());
            }
        }

        for key in &stale_keys {
            self.entries.remove(key);
        }

        if !stale_keys.is_empty() {
            self.save()?;
        }

        Ok(stale_keys.len())
    }
}

pub fn file_modified_and_size(path: &Path) -> (Option<u64>, Option<u64>) {
    fs::metadata(path).map_or((None, None), |meta| {
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|dur| dur.as_secs());
        let size = Some(meta.len());
        (modified, size)
    })
}

fn ffprobe_cmd() -> OsString {
    if let Ok(guard) = FFPROBE_CMD.read() {
        if let Some(cmd) = guard.as_ref() {
            return cmd.clone();
        }
    }
    let cmd = {
        let cfg = crate::config::load_config();
        cfg.ffprobe_cmd
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from("ffprobe"))
    };
    if let Ok(mut guard) = FFPROBE_CMD.write() {
        *guard = Some(cmd.clone());
    }
    cmd
}

pub(crate) fn reset_ffprobe_runtime_state() {
    if let Ok(mut guard) = FFPROBE_CMD.write() {
        *guard = None;
    }
    if let Ok(mut guard) = FFPROBE_AVAILABLE.lock() {
        *guard = None;
    }
    if let Ok(mut cache) = FFPROBE_CACHE.lock() {
        *cache = ProbeCache::default();
    }
}

pub(crate) fn refresh_ffprobe_cache() -> io::Result<usize> {
    let mut cache = FFPROBE_CACHE.lock().expect("ffprobe cache mutex poisoned");
    cache.refresh_stale_entries()
}

pub(crate) fn ffprobe_available() -> bool {
    if let Ok(mut guard) = FFPROBE_AVAILABLE.lock() {
        if let Some(val) = *guard {
            return val;
        }
        let cmd = ffprobe_cmd();
        let result = std::process::Command::new(&cmd)
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or_else(|err| {
                warn!("Failed to run ffprobe command {:?}: {err}", cmd);
                false
            });
        *guard = Some(result);
        result
    } else {
        false
    }
}

fn ffprobe_cached_resolution(path: &Path) -> Option<(u32, u32)> {
    let cache_key = ProbeCache::key_for(path);
    let (mtime, size) = file_modified_and_size(path);

    if let Some(ref key) = cache_key {
        let lookup_result = FFPROBE_CACHE
            .lock()
            .expect("ffprobe cache mutex poisoned")
            .lookup(key, mtime, size);
        if let Some(hit) = lookup_result {
            debug!("ffprobe cache hit for {key}");
            return Some((hit.width, hit.height));
        }
    }

    None
}

fn ffprobe_resolution(path: &Path) -> Option<(u32, u32)> {
    if let Some(hit) = ffprobe_cached_resolution(path) {
        return Some(hit);
    }

    let cache_key = ProbeCache::key_for(path);
    let (mtime, size) = file_modified_and_size(path);

    if !ffprobe_available() {
        return None;
    }

    let cmd = ffprobe_cmd();
    let output = std::process::Command::new(&cmd)
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=width,height")
        .arg("-of")
        .arg("csv=p=0:s=x")
        .arg(path.as_os_str())
        .output()
        .map_err(|err| {
            warn!(
                "Failed to run ffprobe command {:?} on {}: {err}",
                cmd,
                path.display()
            );
        })
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    let dims = stdout.lines().find(|line| !line.trim().is_empty())?.trim();
    let (w, h) = dims.split_once('x')?;
    let width: u32 = w.trim().parse().ok()?;
    let height: u32 = h.trim().parse().ok()?;
    let entry = ProbeEntry {
        mtime,
        size,
        width,
        height,
    };

    if let Some(key) = cache_key {
        let mut cache = FFPROBE_CACHE.lock().expect("ffprobe cache mutex poisoned");
        cache.update(key.clone(), entry);
        if let Err(err) = cache.save() {
            warn!(
                "Failed to persist ffprobe cache {}: {err}",
                ProbeCache::cache_path().display()
            );
        } else {
            debug!("Cached ffprobe result for {key}");
        }
    }

    Some((width, height))
}

/// Heuristic for "is this file HD?" based on filename/path only (>=720p).
/// Returns Some(true/false) if we can tell, or None if unknown.
pub fn is_path_hd(p: &Path) -> Option<bool> {
    // Look at filename *and* parent dir for quality hints.
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let parent = p
        .parent()
        .and_then(|pp| pp.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let hay = format!("{stem} {parent}");

    // Strong SD indicators first — no probing needed.
    for n in [
        "480p", "576p", "sd ", "vhs", "svcd", "xvid", "divx", "dvdrip",
    ] {
        if hay.contains(n) {
            return Some(false);
        }
    }

    if let Some((width, height)) = ffprobe_cached_resolution(p) {
        if width > 0 && height > 0 {
            return Some(width >= 1_280 || height >= 720);
        }
    }

    // Positive HD/UHD signals
    for n in [
        "2160p",
        "uhd",
        "4k",
        "hdr",
        "dolby vision",
        "dv",
        "1080p",
        "720p",
        "blu-ray",
        "bluray",
        "bdrip",
        "hdtv",
        "web-dl",
        "webrip",
    ] {
        if hay.contains(n) {
            return Some(true);
        }
    }

    if let Some((width, height)) = ffprobe_resolution(p) {
        if width > 0 && height > 0 {
            return Some(width >= 1_280 || height >= 720);
        }
    }

    None
}
