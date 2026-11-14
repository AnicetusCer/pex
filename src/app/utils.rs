// src/app/util.rs
use chrono::{Local, TimeZone};
use std::time::SystemTime;
use urlencoding::decode;
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

pub(crate) fn canonicalize_guid(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let decoded = decode(trimmed)
        .map(|cow| cow.into_owned())
        .unwrap_or_else(|_| trimmed.to_string());
    let base = decoded
        .split('#')
        .next()
        .unwrap_or(decoded.as_str())
        .split('?')
        .next()
        .unwrap_or(decoded.as_str())
        .trim();
    if base.is_empty() {
        return None;
    }
    Some(base.to_ascii_lowercase())
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

/// Use dimensions from analysed guide media first; some guides have only station metadata.
pub(super) fn airing_quality(
    width: Option<i64>,
    height: Option<i64>,
    resolution: Option<&str>,
    call_sign: Option<&str>,
    channel_title: Option<&str>,
) -> super::types::AiringQuality {
    use super::types::AiringQuality::{Hd, Sd};

    if let Some(height) = height.filter(|&n| n > 0) {
        return if height >= 720 { Hd } else { Sd };
    }

    // Width alone is sufficient for the usual HD and SD broadcast formats.
    // Intermediate widths are ambiguous without the height.
    match width {
        Some(n) if n >= 1280 => return Hd,
        Some(1..=720) => return Sd,
        _ => {}
    }
    station_quality(resolution, call_sign, channel_title)
}

/// Use the station attached to this showing or recorded file. A named station
/// defaults to SD when Plex supplies no HD designation or explicit resolution.
pub(super) fn station_quality(
    resolution: Option<&str>,
    call_sign: Option<&str>,
    channel_title: Option<&str>,
) -> super::types::AiringQuality {
    use super::types::AiringQuality::{Hd, Sd, Unknown};

    let from_resolution = |label: &str| match label {
        "hd" | "uhd" | "720" | "720i" | "720p" | "1080" | "1080i" | "1080p" | "2160" | "2160p"
        | "4k" | "4320" | "4320p" | "8k" => Some(Hd),
        "sd" | "240" | "240p" | "360" | "360p" | "480" | "480i" | "480p" | "576" | "576i"
        | "576p" => Some(Sd),
        _ => None,
    };
    let mut reported = resolution.and_then(|s| from_resolution(&s.trim().to_ascii_lowercase()));
    let mut named_station = false;
    for name in [call_sign, channel_title].into_iter().flatten() {
        let name = name.trim().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "unknown" | "unknown channel" | "n/a" | "none" | "null" | "unnamed"
        ) {
            continue;
        }
        named_station |= name.chars().any(char::is_alphabetic);
        for quality in name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| token.chars().any(|c| c.is_ascii_alphabetic()))
            .filter_map(|token| {
                // Plex may combine a VCN, a compact call sign and a display name.
                if token.ends_with("hd") || token.ends_with("4k") {
                    Some(Hd)
                } else if token.ends_with("sd") {
                    Some(Sd)
                } else {
                    from_resolution(token)
                }
            })
        {
            if reported.is_some_and(|existing| existing != quality) {
                return Unknown;
            }
            reported = Some(quality);
        }
    }
    reported.unwrap_or(if named_station { Sd } else { Unknown })
}
