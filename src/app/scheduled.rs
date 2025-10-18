use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use tracing::warn;
use urlencoding::decode;

use crate::app::utils;
use crate::config::local_library_db_path;

/// Snapshot of Plex DVR scheduled recordings pulled from media_grabs.
pub(crate) struct ScheduledIndex {
    guids: HashSet<String>,
    title_slots: HashMap<String, HashSet<i64>>,
}

impl Default for ScheduledIndex {
    fn default() -> Self {
        Self {
            guids: HashSet::new(),
            title_slots: HashMap::new(),
        }
    }
}

impl ScheduledIndex {
    pub fn is_empty(&self) -> bool {
        self.guids.is_empty() && self.title_slots.is_empty()
    }

    pub fn is_scheduled(
        &self,
        guid: Option<&str>,
        title: &str,
        year: Option<i32>,
        airing: Option<SystemTime>,
    ) -> bool {
        if let Some(g) = guid {
            if self.guids.contains(g) {
                return true;
            }
        }
        let Some(airing_ts) = airing.and_then(system_time_to_unix) else {
            return false;
        };
        let Some(key) = make_title_key(title, year) else {
            return false;
        };
        self.title_slots
            .get(&key)
            .is_some_and(|set| set.contains(&airing_ts))
    }
}

fn system_time_to_unix(ts: SystemTime) -> Option<i64> {
    ts.duration_since(UNIX_EPOCH)
        .ok()
        .map(|dur| dur.as_secs() as i64)
}

fn make_title_key(title: &str, year: Option<i32>) -> Option<String> {
    let normalized = utils::normalize_title(title);
    if normalized.trim().is_empty() {
        return None;
    }
    let lowered = normalized.to_ascii_lowercase();
    let year = year.unwrap_or_default();
    Some(format!("{lowered}:{year}"))
}

pub(crate) fn load_scheduled_index() -> Result<ScheduledIndex, String> {
    let path = local_library_db_path();
    if !path.exists() {
        return Ok(ScheduledIndex::default());
    }

    let flags_common = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    #[cfg(not(windows))]
    let flags = flags_common | OpenFlags::SQLITE_OPEN_URI;
    #[cfg(windows)]
    let flags = flags_common;

    let conn = Connection::open_with_flags(&path, flags)
        .map_err(|err| format!("open library db failed: {err}"))?;

    let mut stmt = conn
        .prepare(
            r#"
        SELECT
            status,
            json_extract(extra_data, '$."mt:guid"')   AS mt_guid,
            json_extract(extra_data, '$."mt:key"')    AS mt_key,
            json_extract(extra_data, '$."mt:title"')  AS mt_title,
            json_extract(extra_data, '$."mt:year"')   AS mt_year,
            json_extract(extra_data, '$."me:beginsAt"') AS begins_at
        FROM media_grabs
        WHERE extra_data IS NOT NULL
          AND json_extract(extra_data, '$."me:beginsAt"') IS NOT NULL
          AND status IN (0, 1, 2, 3)
        "#,
        )
        .map_err(|err| format!("prepare scheduled query failed: {err}"))?;

    let mut index = ScheduledIndex::default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query scheduled rows failed: {err}"))?;

    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let begins_at_str: Option<String> = row.get("begins_at").map_err(|err| err.to_string())?;
        let Some(begins_at) = begins_at_str.as_deref().and_then(|s| s.parse::<i64>().ok()) else {
            continue;
        };

        // Ignore recordings that are far in the past (already completed long ago)
        if begins_at + 86_400 < now {
            continue;
        }

        if let Some(guid) = row
            .get::<_, Option<String>>("mt_guid")
            .map_err(|err| err.to_string())?
            .filter(|g| !g.is_empty())
        {
            index.guids.insert(guid);
            continue;
        }

        if let Some(mt_key) = row
            .get::<_, Option<String>>("mt_key")
            .map_err(|err| err.to_string())?
            .filter(|k| !k.is_empty())
        {
            if let Some(decoded) = decode_mt_key(&mt_key) {
                index.guids.insert(decoded);
                continue;
            }
        }

        let title_opt: Option<String> = row.get("mt_title").map_err(|err| err.to_string())?;
        let title = match title_opt {
            Some(t) if !t.trim().is_empty() => t,
            _ => continue,
        };
        let year_opt: Option<String> = row.get("mt_year").map_err(|err| err.to_string())?;
        let year = year_opt.and_then(|s| s.parse::<i32>().ok());

        if let Some(key) = make_title_key(&title, year) {
            index.title_slots.entry(key).or_default().insert(begins_at);
        }
    }

    load_from_media_subscriptions(&conn, &mut index)?;
    load_from_subscription_desired(&conn, &mut index)?;

    Ok(index)
}

fn load_from_media_subscriptions(
    conn: &Connection,
    index: &mut ScheduledIndex,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT extra_data FROM media_subscriptions WHERE extra_data IS NOT NULL")
        .map_err(|err| format!("prepare media_subscriptions failed: {err}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query media_subscriptions failed: {err}"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let blob: Option<String> = row.get(0).map_err(|err| err.to_string())?;
        let Some(blob) = blob else {
            continue;
        };
        if blob.trim().is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(&blob) {
            Ok(val) => val,
            Err(err) => {
                warn!("Failed to parse media_subscriptions.extra_data: {err}");
                continue;
            }
        };

        let guid = parsed
            .get("hi:guid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if let Some(g) = &guid {
            index.guids.insert(g.clone());
        }

        let title = parsed
            .get("hi:title")
            .and_then(Value::as_str)
            .map(str::to_string);
        let year = parsed
            .get("hi:year")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<i32>().ok());
        let Some(title) = title else {
            continue;
        };

        let airing_times_raw = parsed
            .get("pv:airingTimes")
            .and_then(Value::as_str)
            .unwrap_or("");
        let decoded_times = match decode(airing_times_raw) {
            Ok(cow) => cow,
            Err(_) => continue,
        };

        for ts_str in decoded_times.split(',') {
            let Ok(ts) = ts_str.trim().parse::<i64>() else {
                continue;
            };
            if ts + 86_400 < now {
                continue;
            }
            if let Some(key) = make_title_key(&title, year) {
                index.title_slots.entry(key).or_default().insert(ts);
            }
        }
    }

    Ok(())
}

fn load_from_subscription_desired(
    conn: &Connection,
    index: &mut ScheduledIndex,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT remote_id FROM metadata_subscription_desired_items")
        .map_err(|err| format!("prepare metadata_subscription_desired_items failed: {err}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query metadata_subscription_desired_items failed: {err}"))?;

    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let remote: Option<String> = row.get(0).map_err(|err| err.to_string())?;
        let Some(remote) = remote else {
            continue;
        };
        if remote.trim().is_empty() {
            continue;
        }
        match decode(&remote) {
            Ok(cow) => {
                index.guids.insert(cow.into_owned());
            }
            Err(err) => {
                warn!("Failed to decode metadata_subscription_desired_items remote_id {remote}: {err}");
            }
        }
    }

    Ok(())
}

fn decode_mt_key(mt_key: &str) -> Option<String> {
    let encoded = mt_key.split("/metadata/").nth(1)?;
    match decode(encoded) {
        Ok(cow) => Some(cow.into_owned()),
        Err(err) => {
            warn!("Failed to decode mt:key {encoded}: {err}");
            None
        }
    }
}
