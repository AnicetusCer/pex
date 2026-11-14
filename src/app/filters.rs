// src/app/filters.rs
use chrono::{NaiveDate, TimeZone, Utc};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use super::SortKey;

pub(crate) const OWNED_BEFORE_CUTOFF_DEFAULT_STR: &str = "2022-12-25";
pub(crate) const OWNED_BEFORE_CUTOFF_DEFAULT_TS: u64 = 1_671_926_400; // 2022-12-25 00:00:00 UTC

pub(crate) fn parse_owned_cutoff(input: &str) -> Option<u64> {
    let date = NaiveDate::parse_from_str(input.trim(), "%Y-%m-%d").ok()?;
    let dt = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&dt).timestamp().max(0) as u64)
}

pub(super) struct FilteredGuide {
    pub(super) days: Vec<(i64, Vec<usize>)>,
    pub(super) scheduled_elsewhere: HashMap<usize, super::scheduled::RecordingStatus>,
}

impl crate::app::PexApp {
    /// Group only showings that pass the current filters. Pick the next one
    /// still to air, or the most recent one if all its showings have started.
    pub(super) fn filtered_guide(&self) -> FilteredGuide {
        let now = SystemTime::now();
        let mut films = HashMap::new();
        for (idx, starts) in self.matching_airings(now, None) {
            let row = &self.rows[idx];
            let distance = starts
                .duration_since(now)
                .unwrap_or_else(|past| past.duration());
            let rank = (starts < now, distance, idx, starts);
            let (best, scheduled) = films
                .entry(row.film_key.as_str())
                .or_insert((rank, super::scheduled::RecordingStatus::None));
            *best = (*best).min(rank);
            *scheduled = (*scheduled).max(row.scheduled);
        }

        let mut days = BTreeMap::<i64, Vec<usize>>::new();
        let mut scheduled_elsewhere = HashMap::new();
        for ((_, _, idx, starts), scheduled) in films.into_values() {
            let row = &self.rows[idx];
            let day = crate::app::utils::day_bucket(starts);
            days.entry(day).or_default().push(idx);
            if scheduled > row.scheduled {
                scheduled_elsewhere.insert(idx, scheduled);
            }
        }
        for idxs in days.values_mut() {
            // Deterministic tie order even when several films share a slot.
            idxs.sort_unstable_by(|&a, &b| {
                self.rows[a].title.cmp(&self.rows[b].title).then(a.cmp(&b))
            });
            self.sort_intra_day(idxs);
            if self.sort_desc {
                idxs.reverse();
            }
        }
        FilteredGuide {
            days: days.into_iter().collect(),
            scheduled_elsewhere,
        }
    }

    pub(super) fn filtered_showings_for(&self, idx: usize) -> Vec<usize> {
        let Some(row) = self.rows.get(idx) else {
            return Vec::new();
        };
        let mut airings: Vec<_> = self
            .matching_airings(SystemTime::now(), Some(&row.film_key))
            .collect();
        airings.sort_unstable_by_key(|&(idx, starts)| (starts, idx));
        airings.into_iter().map(|(idx, _)| idx).collect()
    }

    fn matching_airings<'a>(
        &'a self,
        now: SystemTime,
        film: Option<&'a str>,
    ) -> impl Iterator<Item = (usize, SystemTime)> + 'a {
        let today = crate::app::utils::day_bucket(now);
        let end = self.current_range.max_bucket(today);
        let query = self.search_query.trim().to_lowercase();
        self.rows.iter().enumerate().filter_map(move |(idx, row)| {
            if film.is_some_and(|key| key != row.film_key) {
                return None;
            }
            let starts = row.airing?;
            let day = crate::app::utils::day_bucket(starts);
            if day < today || end.is_some_and(|end| day >= end) {
                return None;
            }
            if !query.is_empty() && !row.title.to_lowercase().contains(&query) {
                return None;
            }
            if !self.selected_channels.is_empty()
                && !row
                    .channel_raw
                    .as_ref()
                    .is_some_and(|channel| self.selected_channels.contains(channel))
            {
                return None;
            }
            if !self.selected_genres.is_empty()
                && !row
                    .genres
                    .iter()
                    .any(|genre| self.selected_genres.contains(genre))
            {
                return None;
            }
            if !self.selected_decades.is_empty()
                && !row
                    .year
                    .is_some_and(|year| self.selected_decades.contains(&(year / 10 * 10)))
            {
                return None;
            }
            let hd = row.airing_quality == super::types::AiringQuality::Hd;
            if self.filter_hd_only && !hd {
                return None;
            }
            if self.hide_owned && row.owned && !row.hd_upgrade() {
                return None;
            }
            if self.filter_owned_before_cutoff
                && !(row.owned
                    && row
                        .owned_modified
                        .is_some_and(|ts| ts < self.owned_before_cutoff_ts))
            {
                return None;
            }
            Some((idx, starts))
        })
    }

    pub(crate) fn available_decades(&self) -> Vec<i32> {
        let mut decades: BTreeSet<i32> = BTreeSet::new();
        for row in &self.rows {
            if let Some(year) = row.year {
                decades.insert((year / 10) * 10);
            }
        }
        decades.into_iter().collect()
    }

    /// Sort a day's indices according to the current SortKey.
    fn sort_intra_day(&self, idxs: &mut [usize]) {
        match self.sort_key {
            SortKey::Time => {
                idxs.sort_by_key(|&i| {
                    self.rows[i]
                        .airing
                        .map(|ts| {
                            ts.duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                        })
                        .unwrap_or(u64::MAX)
                });
            }
            SortKey::Title => idxs.sort_by(|&a, &b| self.rows[a].title.cmp(&self.rows[b].title)),
            SortKey::Channel => {
                idxs.sort_by(|&a, &b| {
                    let ca = self.rows[a].channel.as_deref().unwrap_or("");
                    let cb = self.rows[b].channel.as_deref().unwrap_or("");
                    ca.cmp(cb)
                        .then_with(|| self.rows[a].title.cmp(&self.rows[b].title))
                });
            }
            SortKey::Genre => {
                idxs.sort_by(|&a, &b| {
                    let ga = self.rows[a]
                        .genres
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let gb = self.rows[b]
                        .genres
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    ga.cmp(gb)
                        .then_with(|| self.rows[a].title.cmp(&self.rows[b].title))
                });
            }
        }
    }
}
