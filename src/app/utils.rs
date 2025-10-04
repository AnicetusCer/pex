// src/app/util.rs
use std::time::SystemTime;

pub(crate) fn normalize_title(s: &str) -> String {
    let s = s.to_lowercase();
    let s = s.replace(['.', '_', '-', ':', '–', '—', '(', ')', '[', ']'], " ");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn find_year_in_str(s: &str) -> Option<i32> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i].is_ascii_digit() && bytes[i+1].is_ascii_digit()
            && bytes[i+2].is_ascii_digit() && bytes[i+3].is_ascii_digit() {
            if let Ok(val) = s[i..i+4].parse::<i32>() {
                if (1900..=2099).contains(&val) { return Some(val); }
            }
        }
    }
    None
}

pub(crate) fn day_bucket(ts: SystemTime) -> i64 {
    let secs = ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    secs / 86_400
}

pub(crate) fn weekday_full_from_bucket(bucket: i64) -> &'static str {
    let idx = ((bucket + 4).rem_euclid(7)) as usize; // 1970-01-01 was Thursday
    const NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    NAMES[idx]
}

pub(crate) fn month_short_name(m: u32) -> &'static str {
    const M: [&str; 12] = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    M[(m.saturating_sub(1)).min(11) as usize]
}

pub(crate) fn ordinal_suffix(d: u32) -> &'static str {
    if (11..=13).contains(&(d % 100)) { return "th"; }
    match d % 10 { 1 => "st", 2 => "nd", 3 => "rd", _ => "th" }
}

pub(crate) fn civil_from_days(z0: i64) -> (i32, u32, u32) {
    let z = z0 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + (m <= 2) as i64;
    (y as i32, m as u32, d as u32)
}

pub(crate) fn format_day_label(bucket: i64) -> String {
    let (_y, m, d) = civil_from_days(bucket);
    let wd = weekday_full_from_bucket(bucket);
    format!("{} {}{} {}", wd, d, ordinal_suffix(d), month_short_name(m))
}

pub(crate) fn hhmm_utc(ts: SystemTime) -> String {
    let secs = ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let hm = (secs % 86_400 + 86_400) % 86_400;
    let h = hm / 3600;
    let m = (hm % 3600) / 60;
    format!("{:02}:{:02}", h, m)
}

/// Very light hostname extraction for channel hint (no extra deps).
pub(crate) fn host_from_url(u: &str) -> Option<String> {
    let start = u.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &u[start..];
    let end = rest.find('/').unwrap_or(rest.len());
    if end == 0 { return None; }
    let host = &rest[..end];
    if host.is_empty() { return None; }
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
