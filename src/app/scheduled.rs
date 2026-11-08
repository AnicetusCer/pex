use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use tracing::warn;
use urlencoding::decode;

use crate::app::utils;
use crate::config::local_library_db_path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduledMatchKind {
    Guid,
    TitleSlot,
    None,
}

/// Snapshot of Plex DVR scheduled recordings pulled from media_grabs.
#[derive(Default)]
pub struct ScheduledIndex {
    guids: HashSet<String>,
    guid_slots: HashMap<String, HashSet<i64>>,
    title_slots: HashMap<String, HashSet<i64>>,
}

impl ScheduledIndex {
    pub fn is_empty(&self) -> bool {
        self.guids.is_empty() && self.guid_slots.is_empty() && self.title_slots.is_empty()
    }

    pub(crate) fn match_kind(
        &self,
        guid: Option<&str>,
        title: &str,
        year: Option<i32>,
        airing: Option<SystemTime>,
    ) -> ScheduledMatchKind {
        let airing_ts = airing.and_then(system_time_to_unix);
        if let Some(guid) = guid.and_then(utils::canonicalize_guid) {
            if self.guids.contains(&guid)
                || airing_ts.is_some_and(|slot| {
                    self.guid_slots
                        .get(&guid)
                        .is_some_and(|slots| slots.contains(&slot))
                })
            {
                return ScheduledMatchKind::Guid;
            }
        }
        let Some(airing_ts) = airing_ts else {
            return ScheduledMatchKind::None;
        };
        let Some(key) = make_title_key(title, year) else {
            return ScheduledMatchKind::None;
        };
        if self
            .title_slots
            .get(&key)
            .is_some_and(|set| set.contains(&airing_ts))
        {
            ScheduledMatchKind::TitleSlot
        } else {
            ScheduledMatchKind::None
        }
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

    let now = system_time_to_unix(SystemTime::now()).unwrap_or_default();
    read_scheduled_index(&conn, now)
}

fn read_scheduled_index(conn: &Connection, now: i64) -> Result<ScheduledIndex, String> {
    let mut stmt = conn
        .prepare(
            r#"
        SELECT
            status,
            json_extract(extra_data, '$."mt:guid"')   AS mt_guid,
            json_extract(extra_data, '$."mt:key"')    AS mt_key,
            json_extract(extra_data, '$."mt:title"')  AS mt_title,
            CAST(json_extract(extra_data, '$."mt:year"') AS TEXT) AS mt_year,
            CAST(json_extract(extra_data, '$."me:beginsAt"') AS TEXT) AS begins_at
        FROM media_grabs
        WHERE json_valid(extra_data)
          AND status IN (0, 1, 2, 3)
        "#,
        )
        .map_err(|err| format!("prepare scheduled query failed: {err}"))?;

    let mut index = ScheduledIndex::default();
    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query scheduled rows failed: {err}"))?;

    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let begins_at_str: Option<String> = row.get("begins_at").map_err(|err| err.to_string())?;
        let Some(begins_at) = begins_at_str.as_deref().and_then(|s| s.parse::<i64>().ok()) else {
            continue;
        };

        // Ignore recordings that are far in the past (already completed long ago)
        if begins_at < now.saturating_sub(86_400) {
            continue;
        }

        let guid: Option<String> = row.get("mt_guid").map_err(|err| err.to_string())?;
        let mt_key: Option<String> = row.get("mt_key").map_err(|err| err.to_string())?;
        if let Some(guid) = guid
            .as_deref()
            .and_then(utils::canonicalize_guid)
            .or_else(|| {
                mt_key
                    .as_deref()
                    .and_then(decode_mt_key)
                    .as_deref()
                    .and_then(utils::canonicalize_guid)
            })
        {
            index.guid_slots.entry(guid).or_default().insert(begins_at);
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

    load_from_media_subscriptions(conn, &mut index, now)?;

    Ok(index)
}

fn load_from_media_subscriptions(
    conn: &Connection,
    index: &mut ScheduledIndex,
    now: i64,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.extra_data, d.remote_id FROM media_subscriptions s
                  LEFT JOIN metadata_subscription_desired_items d ON d.sub_id = s.id
                  WHERE s.extra_data IS NOT NULL",
        )
        .map_err(|err| format!("prepare media_subscriptions failed: {err}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query media_subscriptions failed: {err}"))?;

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

        let times = parsed
            .get("pv:airingTimes")
            .and_then(Value::as_str)
            .and_then(|raw| decode(raw).ok());
        let slots: Vec<i64> = times
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .filter(|&ts| ts >= now.saturating_sub(86_400))
            .collect();

        let one_shot = parsed.get("pr:oneShot").is_some_and(|v| {
            v.as_bool() == Some(true)
                || v.as_i64() == Some(1)
                || v.as_str()
                    .is_some_and(|text| text == "1" || text.eq_ignore_ascii_case("true"))
        });
        let remote: Option<String> = row.get(1).map_err(|err| err.to_string())?;
        for guid in [
            parsed.get("hi:guid").and_then(Value::as_str),
            remote.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(utils::canonicalize_guid)
        {
            if one_shot {
                if !slots.is_empty() {
                    index.guid_slots.entry(guid).or_default().extend(&slots);
                }
            } else {
                index.guids.insert(guid);
            }
        }
        if let Some(title) = parsed.get("hi:title").and_then(Value::as_str) {
            let year = parsed.get("hi:year").and_then(|val| {
                val.as_i64()
                    .and_then(|y| i32::try_from(y).ok())
                    .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
            });
            if let Some(key) = make_title_key(title, year) {
                if !slots.is_empty() {
                    index.title_slots.entry(key).or_default().extend(slots);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_shot_recording_marks_only_its_selected_airing() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(r#"
            CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
            CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
            CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);
            INSERT INTO media_grabs VALUES
                (0, '{"mt:guid":"plex://grab","mt:title":"Guide title","mt:year":2023,"me:beginsAt":200000}'),
                (0, 'malformed legacy entry');
            INSERT INTO media_subscriptions VALUES
                (1, '{"pr:oneShot":"1","hi:guid":"plex://one","pv:airingTimes":"200000"}'),
                (2, '{"pr:oneShot":"0","hi:guid":"plex://all"}'),
                (3, '{"pr:oneShot":"1","hi:guid":"plex://past","pv:airingTimes":"10"}');
            INSERT INTO metadata_subscription_desired_items VALUES
                (1, 'plex%3A%2F%2Fdesired'),
                (99, 'plex%3A%2F%2Forphan');
        "#).unwrap();
        let index = read_scheduled_index(&db, 190000).unwrap();
        let slot = |ts| Some(UNIX_EPOCH + std::time::Duration::from_secs(ts));
        for guid in ["plex://one", "plex://desired", "plex://grab"] {
            assert_eq!(
                index.match_kind(Some(guid), "", None, slot(200000)),
                ScheduledMatchKind::Guid
            );
            assert_eq!(
                index.match_kind(Some(guid), "", None, slot(210000)),
                ScheduledMatchKind::None
            );
        }
        assert_eq!(
            index.match_kind(None, "Guide title", Some(2023), slot(200000)),
            ScheduledMatchKind::TitleSlot
        );
        assert_eq!(
            index.match_kind(Some("plex://all"), "", None, slot(210000)),
            ScheduledMatchKind::Guid
        );
        for guid in ["plex://past", "plex://orphan"] {
            assert_eq!(
                index.match_kind(Some(guid), "", None, slot(200000)),
                ScheduledMatchKind::None
            );
        }
    }
}
