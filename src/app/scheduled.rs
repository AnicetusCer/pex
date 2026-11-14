use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{types::ValueRef, Connection, OpenFlags};
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
pub struct GuideChannel {
    id: Option<String>,
    number: Option<String>,
    call_sign: Option<String>,
    title: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum BookingChannel {
    #[default]
    Unspecified,
    Unconfirmed,
    Station(GuideChannel),
    Token(String),
    Catalog(Arc<[GuideChannel]>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum StationMatch {
    Different,
    Unknown,
    Name,
    Number,
    Id,
}

fn field(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_ascii_lowercase()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn same_number(a: &str, b: &str) -> bool {
    a == b
        || (a.bytes().all(|c| c.is_ascii_digit())
            && b.bytes().all(|c| c.is_ascii_digit())
            && a.trim_start_matches('0') == b.trim_start_matches('0'))
}

impl GuideChannel {
    pub(super) fn read(metadata: &Value) -> Self {
        Self::from_metadata(metadata, "at")
    }

    fn from_metadata(value: &Value, prefix: &str) -> Self {
        Self::from_fields(|suffix| field(value, &format!("{prefix}:{suffix}")))
    }

    fn from_fields(get: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            id: get("channelIdentifier")
                .or_else(|| get("channelID"))
                .or_else(|| get("channelId")),
            number: get("channelNumber").or_else(|| get("channelVcn")),
            call_sign: get("channelCallSign"),
            title: get("channelTitle"),
        }
    }

    fn compare(&self, guide: &Self, display_name: Option<&str>) -> StationMatch {
        use StationMatch::*;
        if let (Some(a), Some(b)) = (&self.id, &guide.id) {
            return if a == b { Id } else { Different };
        }
        if let (Some(a), Some(b)) = (&self.number, &guide.number) {
            return if same_number(a, b) { Number } else { Different };
        }
        // The DVR can store a call sign in its title, even if the guide has both fields.
        let names = [guide.title.as_deref(), guide.call_sign.as_deref()];
        if [
            self.title.as_deref(),
            self.call_sign.as_deref(),
            display_name,
        ]
        .into_iter()
        .flatten()
        .any(|name| names.into_iter().flatten().any(|other| name == other))
        {
            return Name;
        }
        if self.title.is_some() && guide.title.is_some()
            || self.call_sign.is_some() && guide.call_sign.is_some()
        {
            Different
        } else {
            Unknown
        }
    }

    fn compare_token(&self, token: &str) -> StationMatch {
        use StationMatch::*;
        if self.id.as_deref() == Some(token) {
            Id
        } else if self
            .number
            .as_deref()
            .is_some_and(|n| same_number(n, token))
        {
            Number
        } else if self.call_sign.as_deref() == Some(token) || self.title.as_deref() == Some(token) {
            Name
        } else {
            Unknown
        }
    }
}

impl BookingChannel {
    fn matches(&self, stations: &[&GuideChannel]) -> Vec<StationMatch> {
        use StationMatch::*;
        let catalog = match self {
            Self::Unspecified | Self::Unconfirmed => return vec![Unknown; stations.len()],
            Self::Token(token) => return stations.iter().map(|s| s.compare_token(token)).collect(),
            Self::Station(station) => std::slice::from_ref(station),
            Self::Catalog(catalog) => catalog.as_ref(),
        };
        let mut matches = vec![Different; stations.len()];
        for station in catalog {
            let display_name = station.title.as_deref().and_then(|title| {
                if !matches!(self, Self::Catalog(_)) || station.number.is_none() {
                    return None;
                }
                title.split_once(" (")?.1.strip_suffix(')')
            });
            let evidence: Vec<_> = stations
                .iter()
                .map(|s| station.compare(s, display_name))
                .collect();
            let best = evidence.iter().copied().max().unwrap_or(Different);
            for (result, hit) in matches.iter_mut().zip(evidence) {
                // For this catalog entry an identified station outranks a shared display name.
                if best <= Unknown
                    || hit == best
                    || best == Name && hit == Unknown && catalog.len() > 1
                {
                    *result = (*result).max(hit);
                }
            }
        }
        matches
    }
}

#[derive(Default)]
struct Slots(HashMap<(String, i64), Vec<BookingChannel>>);

impl Slots {
    fn add(&mut self, identity: &str, time: i64, channel: &BookingChannel) {
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
    ) -> Option<(ScheduledMatchKind, &'a [BookingChannel])> {
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
    past_one_shots: HashMap<i64, HashSet<String>>,
    booked_identities: HashSet<String>,
    indexed_at: i64,
    live_airings: HashMap<i64, Vec<LiveAiring>>,
    live_films: HashSet<String>,
}

struct LiveAiring {
    guid: String,
    time: SystemTime,
    station: GuideChannel,
}

impl LiveAiring {
    fn matches_padded_start(&self, guid: Option<&str>, row: &super::PosterRow) -> bool {
        guid == Some(self.guid.as_str())
            && row.airing.is_some_and(|start| {
                start
                    .duration_since(self.time)
                    .is_ok_and(|padding| padding <= Duration::from_secs(3600))
            })
            && self.station.compare(&row.guide_channel, None) >= StationMatch::Number
    }
}

impl ScheduledIndex {
    pub(super) fn add_live_movie(&mut self, metadata: &Value, media: Option<&Value>) {
        // A numeric ratingKey identifies a row local to a provider, not a global film.
        let guid = ["guid", "ratingKey"].iter().find_map(|key| {
            utils::canonicalize_guid(metadata.get(key)?.as_str()?)
                .filter(|value| value.contains("://"))
        });
        if let Some(guid) = &guid {
            self.live_films.insert(format!("guid:{guid}"));
        }
        let Some((media, time)) =
            media.and_then(|m| Some((m, integer(m.get("beginsAt")).filter(|time| *time > 0)?)))
        else {
            return;
        };
        // Use title/year only for a booking without a global identity, so a different
        // film with the same name cannot inherit a known GUID's booking.
        let station = GuideChannel::from_fields(|key| field(media, key));
        if let (Some(guid), Some(id), Some(time)) = (
            guid.as_ref(),
            integer(media.get("id")).filter(|id| *id > 0),
            UNIX_EPOCH.checked_add(Duration::from_secs(time as u64)),
        ) {
            self.live_airings.entry(id).or_default().push(LiveAiring {
                guid: guid.clone(),
                time,
                station: station.clone(),
            });
        }
        let identity = guid.or_else(|| {
            let year = i32::try_from(integer(metadata.get("year"))?)
                .ok()
                .filter(|y| *y > 0)?;
            make_title_key(metadata.get("title")?.as_str()?, Some(year))
        });
        if let Some(identity) = identity {
            let channel = if station == GuideChannel::default() {
                BookingChannel::Unconfirmed
            } else {
                BookingChannel::Station(station)
            };
            self.grabs.add(&identity, time, &channel);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.films.is_empty()
            && self.grabs.0.is_empty()
            && self.subscriptions.0.is_empty()
            && self.past_one_shots.is_empty()
            && self.live_films.is_empty()
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
        let mut located_live_films = HashSet::new();
        for (&(film_key, airing), group) in &simultaneous {
            let first = &rows[group[0]];
            let guid = first.guid.as_deref().and_then(utils::canonicalize_guid);
            let title = make_title_key(&first.title, first.year);
            let time = airing.and_then(system_time_to_unix);
            let stations: Vec<_> = group.iter().map(|&i| &rows[i].guide_channel).collect();
            let (statuses, kind) =
                self.match_slot(guid.as_deref(), title.as_deref(), time, &stations);
            for (&idx, status) in group.iter().zip(statuses) {
                let (status, kind) = if self
                    .live_airings
                    .get(&rows[idx].guide_media_id)
                    .is_some_and(|candidates| {
                        candidates.iter().any(|candidate| {
                            candidate.matches_padded_start(guid.as_deref(), &rows[idx])
                                && !simultaneous.contains_key(&(film_key, Some(candidate.time)))
                        })
                    }) {
                    // Some servers return the recording start with pre-roll. A nearby
                    // guide media ID is usable only for the same global film and station.
                    (RecordingStatus::Exact, ScheduledMatchKind::Guid)
                } else if kind == ScheduledMatchKind::None
                    && self.has_linked_past_request(
                        rows[idx].movie_subscription_id,
                        guid.as_deref(),
                        title.as_deref(),
                        time,
                    )
                {
                    (RecordingStatus::Film, ScheduledMatchKind::Guid)
                } else {
                    (status, kind)
                };
                flags[idx] = status;
                if status >= RecordingStatus::Slot && self.live_films.contains(film_key) {
                    located_live_films.insert(film_key);
                }
                if status == RecordingStatus::None {
                    stats.misses += 1;
                } else if kind == ScheduledMatchKind::Guid {
                    stats.by_guid += 1;
                } else {
                    stats.by_title_slot += 1;
                }
            }
        }
        if self.live_films.len() > located_live_films.len() {
            let now = SystemTime::now();
            for (row, flag) in rows.iter().zip(&mut flags) {
                if *flag == RecordingStatus::None
                    && row.airing.is_some_and(|start| start >= now)
                    && self.live_films.contains(&row.film_key)
                    && !located_live_films.contains(row.film_key.as_str())
                {
                    *flag = RecordingStatus::Film;
                    stats.by_guid += 1;
                    stats.misses -= 1;
                }
            }
        }
        for (row, status) in rows.iter_mut().zip(flags) {
            row.scheduled = status;
        }
        stats
    }

    fn has_linked_past_request(
        &self,
        subscription_id: Option<i64>,
        guid: Option<&str>,
        title: Option<&str>,
        time: Option<i64>,
    ) -> bool {
        let (Some(id), Some(guid), Some(time)) = (subscription_id, guid, time) else {
            return false;
        };
        time >= self.indexed_at
            && self
                .past_one_shots
                .get(&id)
                .is_some_and(|ids| ids.contains(guid))
            && !title.is_some_and(|key| self.booked_identities.contains(key))
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
                .find(|(_, channels)| {
                    channels.iter().any(|c| {
                        matches!(
                            c,
                            BookingChannel::Station(_)
                                | BookingChannel::Token(_)
                                | BookingChannel::Catalog(_)
                        )
                    })
                })
                .or_else(|| bookings.iter().flatten().next());
            if let Some((kind, channels)) = selected {
                let mut flags = vec![RecordingStatus::None; stations.len()];
                for record in *channels {
                    let matches = record.matches(stations);
                    let positives = matches
                        .iter()
                        .filter(|m| **m > StationMatch::Unknown)
                        .count();
                    let unresolved_alternative = matches!(record, BookingChannel::Catalog(_))
                        && matches.contains(&StationMatch::Unknown);
                    for (flag, hit) in flags.iter_mut().zip(matches) {
                        let status = match (positives, hit) {
                            (1, hit) if hit > StationMatch::Unknown && !unresolved_alternative => {
                                RecordingStatus::Exact
                            }
                            (_, hit) if hit > StationMatch::Unknown => RecordingStatus::Slot,
                            (0, StationMatch::Unknown)
                                if stations.len() == 1
                                    && *record == BookingChannel::Unspecified =>
                            {
                                RecordingStatus::Exact
                            }
                            (0, StationMatch::Unknown) => RecordingStatus::Slot,
                            (_, StationMatch::Unknown) if unresolved_alternative => {
                                RecordingStatus::Slot
                            }
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
        WHERE status IN (0, 1, 2, 3)
        "#,
        )
        .map_err(|err| format!("prepare scheduled query failed: {err}"))?;

    let mut index = ScheduledIndex {
        indexed_at: now,
        ..ScheduledIndex::default()
    };
    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query scheduled rows failed: {err}"))?;

    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let Some(data) = recording_metadata(row.get_ref(0).map_err(|err| err.to_string())?) else {
            continue;
        };
        let Some(time) = integer(data.get("me:beginsAt")) else {
            continue;
        };
        if time < now.saturating_sub(86_400) {
            continue;
        }
        let mut channel = GuideChannel::from_metadata(&data, "me");
        let at = GuideChannel::read(&data);
        channel.id = channel.id.or(at.id);
        channel.number = channel.number.or(at.number);
        channel.call_sign = channel.call_sign.or(at.call_sign);
        channel.title = channel.title.or(at.title);
        let channel = if channel == GuideChannel::default() {
            BookingChannel::Unspecified
        } else {
            BookingChannel::Station(channel)
        };
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
        let title = data
            .get("mt:title")
            .and_then(Value::as_str)
            .and_then(|title| {
                make_title_key(
                    title,
                    integer(data.get("mt:year")).and_then(|y| i32::try_from(y).ok()),
                )
            });
        for key in [guid, title].into_iter().flatten() {
            index.grabs.add(&key, time, &channel);
        }
    }

    load_from_media_subscriptions(conn, &mut index, now)?;
    if !index.past_one_shots.is_empty() {
        index.booked_identities = index
            .grabs
            .0
            .keys()
            .chain(index.subscriptions.0.keys())
            .map(|(identity, _)| identity.clone())
            .collect();
        // A retained booking under any GUID used by the subscription supersedes
        // the old film-level link, even when the guide uses a different GUID alias.
        index
            .past_one_shots
            .retain(|_, guids| guids.is_disjoint(&index.booked_identities));
    }

    Ok(index)
}

fn integer(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

// Plex databases can contain UTF-8 JSON in either TEXT or BLOB columns.
// A malformed record has no usable booking and mustn't discard other rows.
fn recording_metadata(value: ValueRef<'_>) -> Option<Value> {
    match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => serde_json::from_slice(bytes).ok(),
        _ => None,
    }
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

fn catalog_station(entry: &str) -> Option<GuideChannel> {
    let (id, label) = entry.split_once('=')?;
    let id = decode(id).ok()?.trim().to_ascii_lowercase();
    let title = decode(label).ok()?.trim().to_ascii_lowercase();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    // Plex's display label can include a VCN and call sign, e.g. "106 MOV4HD (Movie Four HD)".
    // Keep the complete label too: some guides store it verbatim as the station title.
    let numbered = title.split_once(char::is_whitespace).filter(|(number, _)| {
        number.as_bytes().first().is_some_and(u8::is_ascii_digit)
            && number
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    });
    let number = numbered.map(|(n, _)| n.to_owned());
    let call_sign = numbered.and_then(|(_, tail)| {
        let name = tail.split_once(" (").map_or(tail, |(name, _)| name).trim();
        (!name.is_empty()).then(|| name.to_owned())
    });
    Some(GuideChannel {
        id: Some(id),
        title: Some(title),
        number,
        call_sign,
    })
}

fn subscription_channels(value: Option<&Value>, count: usize) -> Vec<BookingChannel> {
    let Some(value) = value else {
        return vec![BookingChannel::Unspecified; count];
    };
    if let Some(raw) = value.as_str() {
        // The stored value encodes the whole map. Split its entries BEFORE decoding the
        // component strings, otherwise an encoded ampersand in a label becomes a separator.
        let entries = if raw.contains('=') {
            raw.into()
        } else {
            decode(raw).unwrap_or(raw.into())
        };
        if entries.contains('=') {
            let channel = entries
                .split('&')
                .map(catalog_station)
                .collect::<Option<Vec<_>>>()
                .map_or(BookingChannel::Unconfirmed, |stations| {
                    BookingChannel::Catalog(stations.into())
                });
            return vec![channel; count];
        }
    }
    // Legacy subscriptions use a positional list of bare station tokens.
    let tokens = subscription_list(Some(value));
    if tokens.len() != count {
        return vec![BookingChannel::Unconfirmed; count];
    }
    tokens
        .into_iter()
        .map(|token| {
            if token.is_empty() {
                BookingChannel::Unconfirmed
            } else {
                BookingChannel::Token(token)
            }
        })
        .collect()
}

fn load_from_media_subscriptions(
    conn: &Connection,
    index: &mut ScheduledIndex,
    now: i64,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.extra_data, d.remote_id, s.id FROM media_subscriptions s
                  LEFT JOIN metadata_subscription_desired_items d ON d.sub_id = s.id
                  WHERE s.extra_data IS NOT NULL",
        )
        .map_err(|err| format!("prepare media_subscriptions failed: {err}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query media_subscriptions failed: {err}"))?;

    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let Some(parsed) = recording_metadata(row.get_ref(0).map_err(|err| err.to_string())?)
        else {
            continue;
        };

        let times = subscription_list(parsed.get("pv:airingTimes"));
        let channels = subscription_channels(parsed.get("pv:airingChannels"), times.len());
        let times: Vec<_> = times.iter().map(|raw| raw.parse::<i64>().ok()).collect();
        let earliest = now.saturating_sub(86_400);
        let all_expired = !times.is_empty()
            && times
                .iter()
                .all(|t| t.is_some_and(|time| (1..earliest).contains(&time)));
        let slots: Vec<_> = times
            .into_iter()
            .zip(channels)
            .filter_map(|(raw, channel)| {
                let time = raw?;
                if time < earliest {
                    return None;
                }
                Some((time, channel))
            })
            .collect();

        let one_shot = parsed.get("pr:oneShot").is_some_and(|v| {
            v.as_bool() == Some(true)
                || v.as_i64() == Some(1)
                || v.as_str()
                    .is_some_and(|text| text == "1" || text.eq_ignore_ascii_case("true"))
        });
        let remote = match row.get_ref(1).map_err(|err| err.to_string())? {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        };
        let past_id = match row.get_ref(2).map_err(|err| err.to_string())? {
            ValueRef::Integer(id) if id > 0 && one_shot && all_expired => Some(id),
            _ => None,
        };
        for guid in [parsed.get("hi:guid").and_then(Value::as_str), remote]
            .into_iter()
            .flatten()
            .filter_map(utils::canonicalize_guid)
        {
            if !one_shot {
                index.films.insert(guid.clone());
            }
            if let Some(id) = past_id {
                index
                    .past_one_shots
                    .entry(id)
                    .or_default()
                    .insert(guid.clone());
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
    fn bad_dvr_rows_leave_other_text_and_blob_bookings_available() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            r#"
            CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
            CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
            CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);
            INSERT INTO media_grabs VALUES
              (0,'{"mt:guid":"plex://bad-grab","me:beginsAt":10,"unused":1e999}'),
              (0,NULL), (0,'not JSON'), (0,x'FF'),
              (0,CAST('{"mt:guid":"plex://blob-grab","me:beginsAt":10}' AS BLOB)),
              (0,'{"mt:guid":"plex://text-grab","me:beginsAt":10}');
            INSERT INTO media_subscriptions VALUES
              (1,'{"hi:guid":"plex://bad-sub","pv:airingTimes":"10","unused":1e999}'),
              (2,x'FF'), (3,NULL), (4,'not JSON'),
              (5,CAST('{"hi:guid":"plex://blob-sub","pv:airingTimes":"10"}' AS BLOB)),
              (6,'{"hi:guid":"plex://text-sub","pv:airingTimes":"10"}');
            INSERT INTO metadata_subscription_desired_items VALUES
              (5,CAST('plex://desired' AS BLOB)), (6,x'FF');
        "#,
        )
        .unwrap();
        let index = read_scheduled_index(&db, 1).unwrap();
        let flags = |guid| {
            index
                .match_slot(Some(guid), None, Some(10), &[&GuideChannel::default()])
                .0[0]
        };
        for guid in [
            "plex://text-grab",
            "plex://blob-grab",
            "plex://text-sub",
            "plex://blob-sub",
            "plex://desired",
        ] {
            assert_eq!(flags(guid), RecordingStatus::Exact, "{guid}");
        }
        for guid in ["plex://bad-sub", "plex://bad-grab"] {
            assert_eq!(flags(guid), RecordingStatus::None, "{guid}");
        }
        db.execute("DROP TABLE media_grabs", []).unwrap();
        assert!(read_scheduled_index(&db, 1).is_err());
    }

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
    fn expired_one_shots_need_a_matching_guide_link_and_no_retained_booking() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            r#"
            CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
            CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
            CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);
            INSERT INTO media_subscriptions VALUES
              (1, '{"hi:guid":"plex://movie","pr:oneShot":true,"pv:airingTimes":"10,20"}'),
              (2, '{"hi:guid":"plex://started","pr:oneShot":true,"pv:airingTimes":"199940"}'),
              (3, '{"hi:guid":"plex://malformed","pr:oneShot":true,"pv:airingTimes":"10,bad"}');
            INSERT INTO metadata_subscription_desired_items VALUES (1, 'plex%3A%2F%2Fdesired');
        "#,
        )
        .unwrap();
        let now = 200000;
        let linked = |index: &ScheduledIndex, id, guid, title, time| {
            index.has_linked_past_request(Some(id), Some(guid), title, Some(time))
        };
        let index = read_scheduled_index(&db, now).unwrap();
        assert!(linked(&index, 1, "plex://desired", None, now));
        assert!(linked(&index, 1, "plex://movie", None, now));
        assert!(!linked(&index, 2, "plex://movie", None, now));
        assert!(!linked(&index, 1, "plex://remake", None, now));
        assert!(!linked(&index, 1, "plex://movie", None, now - 1));
        assert!(!linked(&index, 2, "plex://started", None, now + 86400));
        assert!(!linked(&index, 3, "plex://malformed", None, now));

        db.execute(
            "INSERT INTO media_grabs VALUES (0, ?1)",
            [r#"{"mt:title":"Rescheduled Film","mt:year":1984,"me:beginsAt":220000}"#],
        )
        .unwrap();
        let title = make_title_key("Rescheduled Film", Some(1984));
        let index = read_scheduled_index(&db, now).unwrap();
        assert!(!linked(&index, 1, "plex://movie", title.as_deref(), now));

        db.execute(
            "INSERT INTO media_grabs VALUES (0, ?1)",
            [r#"{"mt:guid":"plex://desired","me:beginsAt":199940}"#],
        )
        .unwrap();
        let index = read_scheduled_index(&db, now).unwrap();
        assert!(!linked(&index, 1, "plex://movie", None, now));
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
              (4,'{"hi:guid":"plex://all","pr:oneShot":"0"}'),
              (5,'{"hi:guid":"plex://one","pr:oneShot":1,"pv:airingTimes":"110","pv:airingChannels":"049"}');
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
        let current = GuideChannel::read(&serde_json::json!({"at:channelVcn":49}));
        let sibling = GuideChannel::read(&serde_json::json!({
            "at:channelVcn":50,"at:channelIdentifier":"49","at:channelCallSign":"49","at:channelTitle":"49"
        }));
        assert_eq!(pair(110, &sibling, &current), [Absent, Exact]);
        assert_eq!(pair(110, &sibling, &GuideChannel::default()), [Slot, Slot]);
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

    #[test]
    fn plex_station_catalogs_select_guide_stations_at_each_airtime() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(r#"
            CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
            CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
            CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);
            INSERT INTO media_subscriptions VALUES
              (1,'{"hi:guid":"plex://single","pv:airingTimes":"10","pv:airingChannels":"station-hd%3D106%2520MOV4HD%2520%2528Movie%2520Four%2520HD%2529"}'),
              (2,'{"hi:guid":"plex://catalog","pr:oneShot":1,"pv:airingTimes":"30%2C20","pv:airingChannels":"sta-a%3D104%2520AX%26sta-b%3D049%2520BY"}'),
              (3,'{"hi:guid":"plex://special","pv:airingTimes":"40","pv:airingChannels":"lineup%252Fstation%253Da%3DFilms%252C%2520Arts%2520%2526%2520Music"}'),
              (4,'{"hi:guid":"plex://malformed","pv:airingTimes":"50","pv:airingChannels":"sta-a%3D104%2520AX%26"}'),
              (5,'{"hi:guid":"plex://empty","pv:airingTimes":"50","pv:airingChannels":""}'),
              (6,'{"hi:guid":"plex://mismatched","pv:airingTimes":"50","pv:airingChannels":"100,200"}'),
              (7,'{"hi:guid":"plex://regional","pv:airingTimes":"70","pv:airingChannels":"sta-a%3D101%2520MOV%26sta-b%3D102%2520MOV"}');
            INSERT INTO media_grabs VALUES
              (0,'{"mt:guid":"plex://grab","me:beginsAt":60,"me:channelTitle":"MOV4HD"}');
        "#).unwrap();
        let index = read_scheduled_index(&db, 1).unwrap();
        let flags = |guid: &str, time: i64, guide: &[&GuideChannel]| {
            index.match_slot(Some(guid), None, Some(time), guide).0
        };
        let channel = |id, title, call_sign, vcn| {
            GuideChannel::read(&serde_json::json!({
                "at:channelIdentifier":id, "at:channelTitle":title, "at:channelCallSign":call_sign,
                "at:channelVcn":vcn,
            }))
        };
        use RecordingStatus::{Exact, None as Absent, Slot};
        let hd = channel("station-hd", "Movie Four HD", "MOV4HD", "106");
        let shared_title = channel("", "106 MOV4HD (Movie Four HD)", "", "");
        let other = channel("other", "106 MOV4HD (Movie Four HD)", "MOV4HD", "106");
        assert_eq!(
            flags("plex://single", 10, &[&shared_title, &hd, &other]),
            [Absent, Exact, Absent]
        );
        assert_eq!(
            flags("plex://single", 10, &[&shared_title, &other]),
            [Exact, Absent]
        );
        let by_vcn = channel("", "", "", "106");
        let wrong_vcn = channel("", "106 MOV4HD (Movie Four HD)", "", "206");
        assert_eq!(
            flags("plex://single", 10, &[&by_vcn, &wrong_vcn]),
            [Exact, Absent]
        );
        let call_sign = channel("", "Movie Four HD", "MOV4HD", "");
        assert_eq!(flags("plex://single", 10, &[&call_sign]), [Exact]);
        assert_eq!(flags("plex://grab", 60, &[&call_sign]), [Exact]);
        let title_only = channel("", "Movie Four HD", "", "");
        assert_eq!(flags("plex://single", 10, &[&title_only]), [Exact]);

        let a = channel("sta-a", "", "", "");
        let b = channel("sta-b", "", "", "");
        let by_name_only = channel("", "049 BY", "", "");
        let numbered = GuideChannel::read(&serde_json::json!({"at:channelVcn":49}));
        assert_eq!(flags("plex://catalog", 30, &[&numbered]), [Exact]);
        assert_eq!(flags("plex://catalog", 30, &[&b, &other]), [Exact, Absent]);
        assert_eq!(flags("plex://catalog", 20, &[&other, &a]), [Absent, Exact]);
        assert_eq!(
            flags("plex://catalog", 30, &[&b, &a, &other]),
            [Slot, Slot, Absent]
        );
        assert_eq!(
            flags("plex://catalog", 30, &[&by_name_only, &a]),
            [Slot, Slot]
        );
        assert_eq!(flags("plex://catalog", 20, &[&other]), [Absent]);
        assert_eq!(flags("plex://catalog", 25, &[&a]), [Absent]);
        let unknown = GuideChannel::default();
        let shared_call_sign = channel("", "", "MOV", "");
        assert_eq!(
            flags("plex://regional", 70, &[&shared_call_sign, &unknown]),
            [Slot, Slot]
        );

        let special_id = channel("lineup/station=a", "", "", "");
        let special_label = channel("", "Films, Arts & Music", "", "");
        assert_eq!(flags("plex://special", 40, &[&special_id]), [Exact]);
        assert_eq!(flags("plex://special", 40, &[&special_label]), [Exact]);
        for guid in ["plex://malformed", "plex://empty", "plex://mismatched"] {
            assert_eq!(flags(guid, 50, &[&a]), [Slot]);
        }
    }
}
