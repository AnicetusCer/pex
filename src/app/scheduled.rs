use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use tracing::warn;
use urlencoding::decode;

use crate::app::utils;
use crate::config::local_library_db_path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordingStatus {
    #[default]
    None,
    Film,
    Slot,
    Exact,
}

impl RecordingStatus {
    pub(super) fn detail(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Film => Some("Recording requested for this film"),
            Self::Slot => Some("Recording scheduled; channel unconfirmed"),
            Self::Exact => Some("Scheduled to record"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduledMatchKind {
    Guid,
    TitleSlot,
    None,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Channel {
    id: Option<String>,
    number: Option<String>,
    call_sign: Option<String>,
    title: Option<String>,
    // pv:airingChannels is untyped: it can name a number, identifier, or station.
    subscription_value: Option<String>,
}

fn field(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_ascii_lowercase()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Channel {
    fn from_metadata(value: &Value, prefix: &str) -> Self {
        let get = |suffix: &str| field(value, &format!("{prefix}:{suffix}"));
        Self {
            id: get("channelIdentifier")
                .or_else(|| get("channelID"))
                .or_else(|| get("channelId")),
            number: get("channelNumber"),
            call_sign: get("channelCallSign"),
            title: get("channelTitle"),
            subscription_value: None,
        }
    }

    fn compare(&self, guide: &Self) -> Option<bool> {
        let pairs = [
            (&self.id, &guide.id),
            (&self.number, &guide.number),
            (&self.call_sign, &guide.call_sign),
            (&self.title, &guide.title),
        ];
        for (left, right) in pairs {
            if let (Some(a), Some(b)) = (left, right) {
                return Some(a == b);
            }
        }
        // Some guides supply only the call sign, even when the DVR calls it a title.
        if self.title.is_some() && self.title == guide.call_sign
            || self.call_sign.is_some() && self.call_sign == guide.title
        {
            return Some(true);
        }
        if let Some(token) = &self.subscription_value {
            if [&guide.id, &guide.number, &guide.call_sign, &guide.title]
                .into_iter()
                .flatten()
                .any(|v| v == token)
            {
                return Some(true);
            }
        }
        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct GuideChannel(Channel);

impl GuideChannel {
    pub(super) fn read(metadata: &Value) -> Self {
        Self(Channel::from_metadata(metadata, "at"))
    }
}

#[derive(Default)]
struct Slots(HashMap<(String, i64), Vec<Channel>>);

impl Slots {
    fn add(&mut self, identity: &str, time: i64, channel: &Channel) {
        let channels = self.0.entry((identity.to_owned(), time)).or_default();
        if !channels.contains(channel) {
            channels.push(channel.clone());
        }
    }

    fn channels<'a>(
        &'a self,
        guid: Option<&str>,
        title: Option<&str>,
        time: i64,
    ) -> Option<(ScheduledMatchKind, &'a [Channel])> {
        if let Some(channels) = guid.and_then(|id| self.0.get(&(id.to_string(), time))) {
            return Some((ScheduledMatchKind::Guid, channels));
        }
        let channels = self.0.get(&(title?.to_string(), time))?;
        Some((ScheduledMatchKind::TitleSlot, channels))
    }
}

#[derive(Default)]
pub struct ScheduledIndex {
    films: HashSet<String>,
    grabs: Slots,
    subscriptions: Slots,
}

impl ScheduledIndex {
    pub fn is_empty(&self) -> bool {
        self.films.is_empty() && self.grabs.0.is_empty() && self.subscriptions.0.is_empty()
    }

    pub(super) fn apply(&self, rows: &mut [super::PosterRow]) -> super::ScheduledMatchStats {
        let mut stats = super::ScheduledMatchStats::default();
        // Resolve against the complete guide before UI/channel filters hide simulcasts.
        let mut simultaneous: HashMap<(&str, Option<SystemTime>), Vec<usize>> = HashMap::new();
        for (idx, row) in rows.iter().enumerate() {
            simultaneous
                .entry((&row.film_key, row.airing))
                .or_default()
                .push(idx);
        }
        let mut flags = vec![RecordingStatus::None; rows.len()];
        for ((_, airing), group) in simultaneous {
            let first = &rows[group[0]];
            let guid = first.guid.as_deref().and_then(utils::canonicalize_guid);
            let title = make_title_key(&first.title, first.year);
            let time = airing.and_then(system_time_to_unix);
            let stations: Vec<_> = group.iter().map(|&i| &rows[i].guide_channel).collect();
            let (statuses, kind) =
                self.match_slot(guid.as_deref(), title.as_deref(), time, &stations);
            for (idx, status) in group.into_iter().zip(statuses) {
                flags[idx] = status;
                if status == RecordingStatus::None {
                    stats.misses += 1;
                } else if kind == ScheduledMatchKind::Guid {
                    stats.by_guid += 1;
                } else {
                    stats.by_title_slot += 1;
                }
            }
        }
        for (row, status) in rows.iter_mut().zip(flags) {
            row.scheduled = status;
        }
        stats
    }

    fn match_slot(
        &self,
        guid: Option<&str>,
        title_key: Option<&str>,
        time: Option<i64>,
        stations: &[&GuideChannel],
    ) -> (Vec<RecordingStatus>, ScheduledMatchKind) {
        if let Some(time) = time {
            let bookings = [&self.grabs, &self.subscriptions]
                .map(|slots| slots.channels(guid, title_key, time));
            // A concrete DVR grab identifies the actual station. A less specific
            // subscription for the same slot mustn't mark its simulcast too.
            let selected = bookings
                .iter()
                .flatten()
                .find(|(_, channels)| channels.iter().any(|c| c != &Channel::default()))
                .or_else(|| bookings.iter().flatten().next());
            if let Some((kind, channels)) = selected {
                let mut flags = vec![RecordingStatus::None; stations.len()];
                for record in *channels {
                    let matches: Vec<_> = stations.iter().map(|s| record.compare(&s.0)).collect();
                    let positives = matches.iter().filter(|m| **m == Some(true)).count();
                    for (flag, hit) in flags.iter_mut().zip(matches) {
                        let status = match (positives, hit) {
                            (1, Some(true)) => RecordingStatus::Exact,
                            (_, Some(true)) => RecordingStatus::Slot,
                            (0, None) if stations.len() == 1 && *record == Channel::default() => {
                                RecordingStatus::Exact
                            }
                            (0, None) => RecordingStatus::Slot,
                            _ => RecordingStatus::None,
                        };
                        *flag = (*flag).max(status);
                    }
                }
                return (flags, *kind);
            }
        }
        if guid.is_some_and(|g| self.films.contains(g)) {
            (
                vec![RecordingStatus::Film; stations.len()],
                ScheduledMatchKind::Guid,
            )
        } else {
            (
                vec![RecordingStatus::None; stations.len()],
                ScheduledMatchKind::None,
            )
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
        SELECT extra_data
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
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let data: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let Some(time) = integer(data.get("me:beginsAt")) else {
            continue;
        };
        if time < now.saturating_sub(86_400) {
            continue;
        }
        let mut channel = Channel::from_metadata(&data, "me");
        let at = Channel::from_metadata(&data, "at");
        channel.id = channel.id.or(at.id);
        channel.number = channel.number.or(at.number);
        channel.call_sign = channel.call_sign.or(at.call_sign);
        channel.title = channel.title.or(at.title);
        let guid = data
            .get("mt:guid")
            .and_then(Value::as_str)
            .and_then(utils::canonicalize_guid)
            .or_else(|| {
                data.get("mt:key")
                    .and_then(Value::as_str)
                    .and_then(decode_mt_key)
                    .as_deref()
                    .and_then(utils::canonicalize_guid)
            });
        if let Some(guid) = guid {
            index.grabs.add(&guid, time, &channel);
        }
        if let Some(title) = data
            .get("mt:title")
            .and_then(Value::as_str)
            .and_then(|title| {
                make_title_key(
                    title,
                    integer(data.get("mt:year")).and_then(|y| i32::try_from(y).ok()),
                )
            })
        {
            index.grabs.add(&title, time, &channel);
        }
    }

    load_from_media_subscriptions(conn, &mut index, now)?;

    Ok(index)
}

fn integer(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn subscription_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(raw)) => decode(raw)
            .unwrap_or_else(|_| raw.as_str().into())
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .collect(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.trim().to_ascii_lowercase())
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                    .unwrap_or_default()
            })
            .collect(),
        Some(Value::Number(n)) => vec![n.to_string()],
        _ => vec![],
    }
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

        let times = subscription_list(parsed.get("pv:airingTimes"));
        let channels = subscription_list(parsed.get("pv:airingChannels"));
        let slots: Vec<_> = times
            .iter()
            .enumerate()
            .filter_map(|(i, raw)| {
                let time: i64 = raw.parse().ok()?;
                if time < now.saturating_sub(86_400) {
                    return None;
                }
                let token = (times.len() == channels.len())
                    .then(|| &channels[i])
                    .filter(|s| !s.is_empty())
                    .cloned();
                Some((
                    time,
                    Channel {
                        subscription_value: token,
                        ..Default::default()
                    },
                ))
            })
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
            if !one_shot {
                index.films.insert(guid.clone());
            }
            for (time, channel) in &slots {
                index.subscriptions.add(&guid, *time, channel);
            }
        }
        if let Some(title) = parsed.get("hi:title").and_then(Value::as_str) {
            let year = parsed.get("hi:year").and_then(|val| {
                val.as_i64()
                    .and_then(|y| i32::try_from(y).ok())
                    .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
            });
            if let Some(key) = make_title_key(title, year) {
                for (time, channel) in &slots {
                    index.subscriptions.add(&key, *time, channel);
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
        let status = |guid: Option<&str>, title, year, time| {
            index
                .match_slot(
                    guid,
                    make_title_key(title, year).as_deref(),
                    Some(time),
                    &[&GuideChannel::default()],
                )
                .0[0]
        };
        for guid in ["plex://one", "plex://desired", "plex://grab"] {
            assert_eq!(status(Some(guid), "", None, 200000), RecordingStatus::Exact);
            assert_eq!(status(Some(guid), "", None, 210000), RecordingStatus::None);
        }
        assert_eq!(
            status(None, "Guide title", Some(2023), 200000),
            RecordingStatus::Exact
        );
        assert_eq!(
            status(Some("plex://all"), "", None, 210000),
            RecordingStatus::Film
        );
        for guid in ["plex://past", "plex://orphan"] {
            assert_eq!(status(Some(guid), "", None, 200000), RecordingStatus::None);
        }
    }

    #[test]
    fn resolve_simulcasts_without_guessing_the_station() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(r#"
            CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
            CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
            CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);
            INSERT INTO media_grabs VALUES
              (0,'{"mt:guid":"plex://one","me:beginsAt":20,"me:channelTitle":"Movie HD"}'),
              (0,'{"mt:guid":"plex://one","me:beginsAt":30}'),
              (0,'{"mt:guid":"plex://one","me:beginsAt":40,"at:channelIdentifier":"hd","at:channelCallSign":"HD"}'),
              (0,'{"mt:guid":"plex://one","me:beginsAt":50,"at:channelCallSign":"HD"}');
            INSERT INTO media_subscriptions VALUES
              (1,'{"hi:guid":"plex://one","pr:oneShot":1,"pv:airingTimes":"20"}'),
              (2,'{"hi:guid":"plex://one","pr:oneShot":"1","pv:airingTimes":"60%2Cbad%2C70","pv:airingChannels":"200%2Cunused%2C100"}'),
              (3,'{"hi:guid":"plex://one","pr:oneShot":"1","pv:airingTimes":"80,90,100","pv:airingChannels":"100,200"}'),
              (4,'{"hi:guid":"plex://all","pr:oneShot":"0"}');
        "#).unwrap();
        let index = read_scheduled_index(&db, 1).unwrap();
        let sd = GuideChannel::read(
            &serde_json::json!({"at:channelIdentifier":"sd", "at:channelTitle":"Movie SD", "at:channelCallSign":"SD", "at:channelNumber":100}),
        );
        let hd = GuideChannel::read(
            &serde_json::json!({"at:channelIdentifier":"hd", "at:channelTitle":"Movie HD", "at:channelCallSign":"HD", "at:channelNumber":200}),
        );
        let pair = |t, a: &GuideChannel, b: &GuideChannel| {
            index
                .match_slot(Some("plex://one"), None, Some(t), &[a, b])
                .0
        };
        use RecordingStatus::{Exact, None as Absent, Slot};
        for t in [20, 40, 50, 60] {
            assert_eq!(pair(t, &sd, &hd), [Absent, Exact]);
        }
        assert_eq!(pair(70, &sd, &hd), [Exact, Absent]);
        assert_eq!(pair(30, &sd, &hd), [Slot, Slot]);
        assert_eq!(pair(80, &sd, &hd), [Slot, Slot]);
        assert_eq!(pair(95, &sd, &hd), [Absent, Absent]);
        let duplicate_title = GuideChannel::read(
            &serde_json::json!({"at:channelIdentifier":"sd", "at:channelCallSign":"SD", "at:channelTitle":"Movie HD"}),
        );
        assert_eq!(pair(20, &hd, &duplicate_title), [Slot, Slot]);
        let unknown = GuideChannel::default();
        assert_eq!(
            index
                .match_slot(Some("plex://one"), None, Some(20), &[&unknown])
                .0,
            [Slot]
        );
        assert_eq!(
            index
                .match_slot(Some("plex://one"), None, Some(30), &[&hd])
                .0,
            [Exact]
        );
        assert_eq!(
            index
                .match_slot(Some("plex://all"), None, Some(20), &[&sd])
                .0,
            [RecordingStatus::Film]
        );
    }
}
