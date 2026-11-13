pub(crate) mod owned_scan_plex;

use std::collections::HashSet;

use eframe::egui as eg;

use self::owned_scan_plex::OwnedScanPlex;
use crate::app::types::OwnedMsg;

// v1 sidecars contain franchise prefixes and synthetic years indistinguishable from real titles.
pub(super) const ALL_SIDECAR: &str = "owned_all_v2.txt";
pub(super) const HD_SIDECAR: &str = "owned_hd_v2.txt";

impl crate::app::PexApp {
    /// Kick off a non-blocking owned scan against the Plex library database.
    pub(crate) fn start_owned_scan(&mut self, force_copy: bool) {
        if self.owned_scan_in_progress {
            return;
        }
        let cfg = crate::config::load_config();
        if cfg.guide_only
            || (cfg.plex_library_db_source.is_none()
                && !crate::config::local_library_db_path().exists())
        {
            self.owned_scan_available = false;
            self.owned_notice = Some("Guide only: owned and recording badges are disabled. Use Connect to Plex to enable them.".into());
            self.owned_keys = None;
            self.owned_hd_keys = None;
            self.scheduled_index = None;
            self.apply_owned_flags();
            self.apply_scheduled_flags();
            return;
        }
        self.owned_scan_available = true;
        self.owned_notice = None;
        let (tx, rx) = std::sync::mpsc::channel::<OwnedMsg>();
        self.owned_rx = Some(rx);

        self.owned_scan_in_progress = true;

        self.record_owned_message(
            "Stage 3/4 - Loading owned titles from the Plex library database.",
        );
        self.set_status(
            "Stage 3/4 - Loading owned titles from Plex (marks Owned titles and HD upgrades).",
        );
        OwnedScanPlex::spawn_scan(tx, force_copy);
    }

    /// Apply owned flags, clearing them when the scan is disabled or unavailable.
    pub(crate) fn apply_owned_flags(&mut self) {
        let Some(keys) = &self.owned_keys else {
            self.owned_match_stats = crate::app::OwnedMatchStats::default();
            for row in &mut self.rows {
                row.owned = false;
                row.owned_hd = false;
                row.owned_modified = None;
            }
            return;
        };
        self.owned_match_stats = crate::app::OwnedMatchStats::default();
        let owned_guids = self.owned_guids.as_ref();
        let guid_modified = self.owned_guid_modified.as_ref();
        let modified = self.owned_modified.as_ref();
        for row in &mut self.rows {
            let guid = row
                .guid
                .as_deref()
                .and_then(crate::app::utils::canonicalize_guid)
                .filter(|guid| owned_guids.is_some_and(|set| set.contains(guid)));
            if let Some(guid) = guid {
                row.owned = true;
                row.owned_hd = self
                    .owned_hd_guids
                    .as_ref()
                    .is_some_and(|set| set.contains(&guid));
                row.owned_modified = guid_modified.and_then(|m| m.get(&guid)).copied().flatten();
                self.owned_match_stats.by_guid += 1;
            } else if let Some(key) = Self::match_owned_title(&row.title, row.year, keys) {
                row.owned = true;
                row.owned_hd = self
                    .owned_hd_keys
                    .as_ref()
                    .is_some_and(|set| set.contains(&key));
                row.owned_modified = modified.and_then(|m| m.get(&key)).copied().flatten();
                self.owned_match_stats.by_key += 1;
            } else {
                row.owned = false;
                row.owned_hd = false;
                row.owned_modified = None;
                self.owned_match_stats.misses += 1;
            }
        }
    }

    // The library records the real year. Only a lookup may try adjacent years.
    fn library_title_keys(title: &str, year: Option<i32>) -> Vec<String> {
        let mut keys = Vec::new();
        let suffix = year.map_or_else(|| "undated".to_string(), |y| y.to_string());
        for name in Self::owned_title_variants(title) {
            keys.push(format!("{name}:any"));
            keys.push(format!("{name}:{suffix}"));
        }
        keys
    }

    fn match_owned_title(title: &str, year: Option<i32>, keys: &HashSet<String>) -> Option<String> {
        let year = year.or_else(|| Self::trailing_title_year(title).map(|(_, year)| year));
        let names = Self::owned_title_variants(title);
        let find = |suffix: &str| {
            names
                .iter()
                .map(|name| format!("{name}:{suffix}"))
                .find(|key| keys.contains(key))
        };
        let Some(year) = year else {
            return find("any");
        };
        for candidate in [Some(year), year.checked_sub(1), year.checked_add(1)]
            .into_iter()
            .flatten()
        {
            if let Some(key) = find(&candidate.to_string()) {
                return Some(key);
            }
        }

        // One source may omit a subtitle. Require the same year and a bare title on
        // one side, so two different subtitled films never meet through a prefix.
        let (kind, name) = match title.split_once(':') {
            Some((head, _)) => ("bare", head),
            None => ("prefix", title),
        };
        let short_key = Self::short_title_key(kind, name, year);
        if keys.contains(&short_key) {
            return Some(short_key);
        }
        find("undated")
    }

    fn short_title_key(kind: &str, title: &str, year: i32) -> String {
        format!(
            "{kind}:{}:{year}",
            crate::app::utils::normalize_title(title)
        )
    }

    fn owned_title_variants(title: &str) -> Vec<String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut titles = Vec::new();
        titles.push(trimmed.to_string());

        // Handle possessive prefixes like "Lemony Snicket's A Series of ..."
        // so we also match library entries that drop the leading proper name.
        for needle in ["'s ", "’s "] {
            if let Some(pos) = trimmed.find(needle) {
                let before = &trimmed[..pos];
                if !before.contains(' ') {
                    continue;
                }
                let candidate = trimmed[pos + needle.len()..].trim_start();
                if !candidate.is_empty() && candidate != trimmed {
                    titles.push(candidate.to_string());
                }
            }
        }

        // Drop leading English articles so "The Return of Sabata" matches "Return of Sabata".
        let lower = trimmed.to_ascii_lowercase();
        for article in ["the ", "a ", "an "] {
            if lower.starts_with(article) && trimmed.len() > article.len() {
                let candidate = trimmed[article.len()..].trim_start();
                if !candidate.is_empty() && candidate != trimmed {
                    titles.push(candidate.to_string());
                }
            }
        }

        let mut extra: Vec<String> = Vec::new();
        for existing in &titles {
            if let Some(candidate) = Self::variant_drop_g_suffix(existing) {
                extra.push(candidate);
            }
            if let Some((candidate, _)) = Self::trailing_title_year(existing) {
                extra.push(candidate.to_string());
            }
            if let Some(candidate) = Self::variant_swap_word(existing, "thru", "through") {
                extra.push(candidate);
            }
            if let Some(candidate) = Self::variant_swap_word(existing, "through", "thru") {
                extra.push(candidate);
            }
        }
        titles.extend(extra);
        let mut seen = HashSet::new();
        titles
            .into_iter()
            .map(|title| crate::app::utils::normalize_title(&title))
            .filter(|name| !name.is_empty() && seen.insert(name.clone()))
            .collect()
    }

    fn variant_drop_g_suffix(input: &str) -> Option<String> {
        let mut changed = false;
        let mut words: Vec<String> = Vec::new();
        for token in input.split_whitespace() {
            let mut replaced = false;
            if let Some(last) = token.chars().last() {
                if last == '\'' || last == '’' {
                    let base = &token[..token.len() - last.len_utf8()];
                    let lower = base.to_ascii_lowercase();
                    if lower.ends_with("in") && !lower.ends_with("ing") {
                        words.push(format!("{base}g"));
                        changed = true;
                        replaced = true;
                    }
                }
            }
            if !replaced {
                words.push(token.to_string());
            }
        }
        changed.then(|| words.join(" "))
    }

    fn variant_swap_word(input: &str, needle: &str, replacement: &str) -> Option<String> {
        let mut changed = false;
        let mut words: Vec<String> = Vec::new();
        for token in input.split_whitespace() {
            if token.eq_ignore_ascii_case(needle) {
                words.push(replacement.to_string());
                changed = true;
            } else {
                words.push(token.to_string());
            }
        }
        changed.then(|| words.join(" "))
    }

    fn trailing_title_year(title: &str) -> Option<(&str, i32)> {
        let (name, digits) = title.trim().strip_suffix(')')?.rsplit_once('(')?;
        let name = name.trim_end();
        if name.is_empty() || digits.len() != 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some((name, digits.parse().ok()?))
    }

    /// Drain owned-scan messages without blocking the UI thread.
    pub(crate) fn poll_owned_scan(&mut self, _ctx: &eg::Context) {
        use crate::app::types::OwnedMsg::{Done, Error, Info};

        loop {
            let msg = {
                let rx = match self.owned_rx.as_ref() {
                    Some(r) => r,
                    None => return,
                };
                match rx.try_recv() {
                    Ok(m) => m,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.owned_rx = None;
                        self.owned_scan_in_progress = false;
                        self.set_startup_progress_floor(crate::app::STARTUP_STAGE3_DONE);
                        if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                            self.boot_phase = crate::app::BootPhase::Ready;
                        }
                        break;
                    }
                }
            };

            match msg {
                OwnedMsg::Snapshot(s) => {
                    self.owned_notice = Some(s.clone());
                    self.record_owned_message(s);
                }
                Info(s) => {
                    self.record_owned_message(s.clone());
                    self.owned_scan_in_progress = true;
                    self.set_status(s);
                }
                OwnedMsg::Scheduled(result) => match result {
                    Ok(index) => {
                        self.scheduled_index = (!index.is_empty()).then_some(index);
                        self.apply_scheduled_flags();
                        self.mark_dirty();
                    }
                    Err(err) => self.record_owned_message(format!(
                        "Loading scheduled recordings failed: {err}"
                    )),
                },
                Error(e) => {
                    let msg = format!("Owned scan error: {e}");
                    self.owned_notice = Some(msg.clone());
                    self.record_owned_message(msg.clone());
                    self.owned_scan_in_progress = false;
                    self.set_startup_progress_floor(crate::app::STARTUP_STAGE3_DONE);
                    self.set_status(msg);
                    if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                        self.boot_phase = crate::app::BootPhase::Ready;
                    }
                }
                Done(done) => {
                    let crate::app::types::OwnedDone {
                        keys,
                        guids,
                        hd_keys,
                        hd_guids,
                        guid_modified,
                        modified,
                    } = *done;
                    self.set_startup_progress_floor(crate::app::STARTUP_STAGE3_DONE);

                    let count = keys.len();
                    self.owned_keys = Some(keys);
                    self.owned_guids = Some(guids);
                    self.owned_hd_keys = Some(hd_keys);
                    self.owned_hd_guids = Some(hd_guids);
                    self.owned_guid_modified = Some(guid_modified);
                    self.owned_modified = Some(modified);
                    self.apply_owned_flags();
                    self.mark_dirty();
                    self.owned_scan_in_progress = false;
                    self.record_owned_message(format!("Owned scan complete ({count} titles)."));
                    if let Some(msg) = self.stage4_complete_message.clone() {
                        self.set_status(msg);
                    } else {
                        self.set_status(crate::app::OWNED_SCAN_COMPLETE_STATUS);
                    }
                    if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                        self.boot_phase = crate::app::BootPhase::Ready;
                    }
                }
            }
        }
    }
}
