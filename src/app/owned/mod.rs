pub(crate) mod owned_scan_plex;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui as eg;

use self::owned_scan_plex::OwnedScanPlex;
use crate::app::types::OwnedMsg;

impl crate::app::PexApp {
    /// Kick off a non-blocking owned scan against the Plex library database.
    pub(crate) fn start_owned_scan(&mut self) {
        if self.owned_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<OwnedMsg>();
        self.owned_rx = Some(rx);

        self.owned_scan_in_progress = true;

        self.record_owned_message(
            "Stage 3/4 - Loading owned titles from the Plex library database.",
        );
        self.set_status(
            "Stage 3/4 - Loading owned titles from Plex (marks Owned titles and HD upgrades).",
        );
        OwnedScanPlex::spawn_scan(tx);
    }

    /// Apply the owned flags using the computed key set (no-ops if not ready).
    pub(crate) fn apply_owned_flags(&mut self) {
        let Some(keys) = &self.owned_keys else {
            return;
        };
        let modified = self.owned_modified.as_ref();
        for row in &mut self.rows {
            let base_key = row.owned_key.clone();
            let mut matched_key: Option<String> = None;

            for candidate in Self::owned_key_variants(&row.title, row.year) {
                if keys.contains(&candidate) {
                    matched_key = Some(candidate);
                    break;
                }
            }

            if matched_key.is_none() && keys.contains(&base_key) {
                matched_key = Some(base_key.clone());
            }

            if let Some(found) = matched_key {
                row.owned = true;
                row.owned_key = found.clone();
                row.owned_modified = modified.and_then(|m| m.get(&found)).and_then(|v| *v);
            } else {
                row.owned = false;
                row.owned_key = base_key;
                row.owned_modified = None;
            }
        }
    }

    pub(crate) fn owned_key_variants(title: &str, year: Option<i32>) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut variants: Vec<String> = Vec::new();

        let titles = Self::owned_title_variants(title);
        let mut year_candidates: Vec<Option<i32>> = Vec::new();
        if let Some(y) = year {
            for offset in [0, -1, 1] {
                let candidate = y.saturating_add(offset);
                if candidate > 0 {
                    year_candidates.push(Some(candidate));
                }
            }
        }
        year_candidates.push(None);

        for variant_title in titles {
            for candidate_year in &year_candidates {
                let key = Self::make_owned_key(&variant_title, *candidate_year);
                if seen.insert(key.clone()) {
                    variants.push(key);
                }
            }
        }

        if variants.is_empty() {
            let key = Self::make_owned_key(title, year);
            if seen.insert(key.clone()) {
                variants.push(key);
            }
        }

        variants
    }

    fn owned_title_variants(title: &str) -> Vec<String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut titles = Vec::new();
        titles.push(trimmed.to_string());

        if let Some(idx) = trimmed.find(':') {
            let head = trimmed[..idx].trim();
            if !head.is_empty() && head != trimmed {
                titles.push(head.to_string());
            }
        }

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
            if let Some(candidate) = Self::variant_strip_trailing_year(existing) {
                extra.push(candidate);
            }
            if let Some(candidate) = Self::variant_swap_word(existing, "thru", "through") {
                extra.push(candidate);
            }
            if let Some(candidate) = Self::variant_swap_word(existing, "through", "thru") {
                extra.push(candidate);
            }
        }
        titles.extend(extra);
        titles.sort();
        titles.dedup();
        titles
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

    fn variant_strip_trailing_year(input: &str) -> Option<String> {
        let trimmed = input.trim();
        if trimmed.ends_with(')') {
            if let Some(open) = trimmed.rfind('(') {
                let candidate = trimmed[..open].trim_end();
                let year_part = &trimmed[open + 1..trimmed.len() - 1];
                if year_part.len() == 4
                    && year_part.chars().all(|c| c.is_ascii_digit())
                    && !candidate.is_empty()
                {
                    return Some(candidate.to_string());
                }
            }
        }
        None
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
                        self.owned_scan_in_progress = false;
                        if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                            self.boot_phase = crate::app::BootPhase::Ready;
                        }
                        break;
                    }
                }
            };

            match msg {
                Info(s) => {
                    self.record_owned_message(s.clone());
                    self.owned_scan_in_progress = true;
                    self.set_status(s);
                }
                Error(e) => {
                    let msg = format!("Owned scan error: {e}");
                    self.record_owned_message(msg.clone());
                    self.owned_scan_in_progress = false;
                    self.set_status(msg);
                    let should_retry =
                        if self.owned_retry_attempts < crate::app::OWNED_AUTO_RETRY_MAX {
                            let lowered = e.to_ascii_lowercase();
                            lowered.contains("unable to open database")
                                || lowered.contains("database is locked")
                                || lowered.contains("busy")
                        } else {
                            false
                        };

                    if should_retry {
                        self.owned_retry_attempts = self.owned_retry_attempts.saturating_add(1);
                        let attempt = self.owned_retry_attempts;
                        self.owned_retry_next = Some(Instant::now() + Duration::from_secs(3));
                        self.record_owned_message(format!(
                            "Owned scan error; retry {attempt}/{} scheduled…",
                            crate::app::OWNED_AUTO_RETRY_MAX
                        ));
                    } else {
                        self.owned_retry_next = None;
                    }

                    if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                        self.boot_phase = crate::app::BootPhase::Ready;
                    }
                }
                Done { keys, modified } => {
                    if keys.is_empty() {
                        self.owned_scan_in_progress = false;
                        let has_source = crate::config::load_config()
                            .plex_library_db_source
                            .as_ref()
                            .is_some();

                        if has_source {
                            if self.owned_retry_attempts < crate::app::OWNED_AUTO_RETRY_MAX {
                                self.owned_retry_attempts =
                                    self.owned_retry_attempts.saturating_add(1);
                                let attempt = self.owned_retry_attempts;
                                self.owned_retry_next =
                                    Some(Instant::now() + Duration::from_secs(3));
                                self.record_owned_message(format!(
                                    "Owned scan returned no entries (attempt {attempt}/{}) – retrying after copying Plex library…",
                                    crate::app::OWNED_AUTO_RETRY_MAX
                                ));
                                self.set_status("Owned scan retry scheduled…");
                            } else {
                                self.record_owned_message(
                                    "Owned scan returned no entries after automatic retries.",
                                );
                                self.set_status(
                                    "Owned scan completed with no matches. Verify plex_library_db_source in config.json.",
                                );
                                self.owned_keys = Some(HashSet::new());
                                self.owned_retry_next = None;
                            }
                        } else {
                            self.record_owned_message(
                                "Owned scan returned no entries (plex_library_db_source not configured).",
                            );
                            self.set_status(crate::app::OWNED_SCAN_COMPLETE_STATUS);
                            self.owned_keys = Some(HashSet::new());
                            self.owned_retry_next = None;
                        }

                        if !matches!(self.boot_phase, crate::app::BootPhase::Ready) {
                            self.boot_phase = crate::app::BootPhase::Ready;
                        }
                        continue;
                    }

                    self.owned_retry_attempts = 0;
                    self.owned_retry_next = None;

                    let count = keys.len();
                    self.owned_keys = Some(keys);
                    self.owned_hd_keys = Self::load_owned_hd_sidecar();
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
