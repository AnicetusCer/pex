// src/app/filters.rs
use std::time::SystemTime;

use super::{DayRange, SortKey};

impl crate::app::PexApp {
    /// Build grouped indices for the grid: per-day buckets with intra-day sorting applied.
    /// Returns Vec of (day_bucket, indices_for_that_day)
    pub(crate) fn build_grouped_indices(&self) -> Vec<(i64, Vec<usize>)> {
        use std::time::SystemTime;

        let now_bucket = crate::app::utils::day_bucket(SystemTime::now());
        // The helper is in this module — call it directly.
        let max_bucket_opt = max_bucket_for_range(self.current_range, now_bucket);

        // Precompute filters
        let query = self.search_query.to_ascii_lowercase();
        let use_query = !query.is_empty();
        let have_channel_filter = !self.selected_channels.is_empty(); // EMPTY = no filter (show all)

        // 1) Filter + attach day bucket
        let mut filtered: Vec<(usize, i64)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                // time window
                let ts = row.airing?;
                let b = crate::app::utils::day_bucket(ts);
                if b < now_bucket {
                    return None;
                }
                if let Some(max_b) = max_bucket_opt {
                    if b >= max_b {
                        return None;
                    }
                }

                // title search
                if use_query && !row.title.to_ascii_lowercase().contains(&query) {
                    return None;
                }

                // include-only channel filter
                if have_channel_filter {
                    // Compare against BOTH the raw channel string and the humanized label
                    let raw = row.channel.as_deref().unwrap_or("");
                    let human = row
                        .channel
                        .as_deref()
                        .map(crate::app::utils::humanize_channel);

                    let selected_match = self.selected_channels.contains(raw)
                        || human
                            .as_ref()
                            .is_some_and(|h| self.selected_channels.contains(h));

                    if !selected_match {
                        return None;
                    }
                }

                // hide-owned, but KEEP rows that are HD upgrades (airing HD while owned is SD)
                if self.hide_owned && row.owned {
                    let tags_joined = (!row.genres.is_empty()).then(|| row.genres.join("|"));
                    let broadcast_hd = crate::app::utils::infer_broadcast_hd(
                        tags_joined.as_deref(),
                        row.channel.as_deref(),
                    );

                    let owned_key = Self::make_owned_key(&row.title, row.year);
                    let owned_is_hd = self
                        .owned_hd_keys
                        .as_ref()
                        .map_or(false, |set| set.contains(&owned_key));

                    let is_upgrade = broadcast_hd && !owned_is_hd;
                    if !is_upgrade {
                        return None;
                    }
                }

                Some((idx, b))
            })
            .collect();

        // 2) Sort by (day bucket, then title) for stable grouping
        filtered.sort_by(|a, b| {
            let (ai, ab) = a;
            let (bi, bb) = b;
            ab.cmp(bb)
                .then_with(|| self.rows[*ai].title.cmp(&self.rows[*bi].title))
        });

        // 3) Group contiguous buckets
        let mut groups: Vec<(i64, Vec<usize>)> = Vec::new();
        let mut cur_key: Option<i64> = None;
        for (idx, bucket) in filtered {
            if cur_key != Some(bucket) {
                groups.push((bucket, Vec::new()));
                cur_key = Some(bucket);
            }
            if let Some((_, v)) = groups.last_mut() {
                v.push(idx);
            }
        }

        // 4) Intra-day sorting based on current SortKey (+ optional desc)
        for (_bucket, idxs) in groups.iter_mut() {
            self.sort_intra_day(idxs);
            if self.sort_desc {
                idxs.reverse();
            }
        }

        groups
    }

    /// Sort a day's indices according to the current SortKey.
    fn sort_intra_day(&self, idxs: &mut Vec<usize>) {
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

/// Map the current DayRange to a max day-bucket (exclusive upper bound).
fn max_bucket_for_range(range: DayRange, now_bucket: i64) -> Option<i64> {
    match range {
        DayRange::Two => Some(now_bucket + 2),
        DayRange::Four => Some(now_bucket + 4),
        DayRange::Five => Some(now_bucket + 5),
        DayRange::Seven => Some(now_bucket + 7),
        DayRange::Fourteen => Some(now_bucket + 14),
    }
}
