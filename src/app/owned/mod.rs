pub(crate) mod owned_scan_plex;

use std::collections::HashSet;

use eframe::egui as eg;

use self::owned_scan_plex::OwnedScanPlex;
use crate::app::types::{
    OwnedCandidate, OwnedCandidates, OwnedField, OwnedLibrary, OwnedMsg, PosterRow,
};

// v1 sidecars contain franchise prefixes and synthetic years indistinguishable from real titles.
pub(super) const ALL_SIDECAR: &str = "owned_all_v3.txt";
pub(super) const HD_SIDECAR: &str = "owned_hd_v3.txt";

struct OwnedMatch<'a> {
    // A GUID match always comes from the scanned library, even if a title also matches.
    title_key: Option<String>,
    candidates: &'a [OwnedCandidate],
}

impl OwnedMatch<'_> {
    fn explanation(&self, guide_year: Option<i32>) -> &'static str {
        let Some(key) = &self.title_key else {
            return "Matched by the same Plex metadata identifier.";
        };
        if key.ends_with(":any") || key.ends_with(":undated") {
            "Matched by name. A release year was unavailable."
        } else if key.starts_with("bare:")
            || key.starts_with("credited:")
            || key.starts_with("prefix:")
        {
            "Matched a shortened title and the same release year."
        } else if key
            .rsplit_once(':')
            .and_then(|(_, y)| y.parse::<i32>().ok())
            .zip(guide_year)
            .is_some_and(|(year, guide_year)| year != guide_year)
        {
            "Matched by name with a one-year release date difference."
        } else {
            "Matched by name and release year."
        }
    }
}

fn lookup<'a>(
    title: &str,
    year: Option<i32>,
    guid: Option<&str>,
    keys: &HashSet<String>,
    library: Option<&'a OwnedLibrary>,
) -> Option<OwnedMatch<'a>> {
    if let Some(candidates) = library.and_then(|lib| {
        let guid = crate::app::utils::canonicalize_guid(guid?)?;
        lib.by_guid.get(&guid)
    }) {
        return Some(OwnedMatch {
            title_key: None,
            candidates: candidates.as_slice(),
        });
    }
    let key = crate::app::PexApp::match_owned_title(title, year, keys)?;
    let candidates = library
        .and_then(|lib| lib.by_title.get(&key))
        .map_or(&[][..], OwnedCandidates::as_slice);
    Some(OwnedMatch {
        title_key: Some(key),
        candidates,
    })
}

impl crate::app::PexApp {
    /// Kick off a non-blocking owned scan against the Plex library database.
    pub(crate) fn start_owned_scan(&mut self, force_copy: bool) {
        if self.owned_scan_in_progress {
            return;
        }
        let cfg = crate::config::load_config();
        if cfg.guide_only
            || (cfg.plex_library_db_source.is_none()
                && cfg.library_snapshot_dir.is_none()
                && !crate::config::local_library_db_path().exists())
        {
            self.owned_scan_available = false;
            self.owned_notice = Some("Guide only: owned and recording badges are disabled. Use Connect to Plex to enable them.".into());
            self.owned_keys = None;
            self.owned_hd_keys = None;
            self.owned_library = None;
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
        let library = self.owned_library.as_ref();
        for row in &mut self.rows {
            let Some(found) = lookup(&row.title, row.year, row.guid.as_deref(), keys, library)
            else {
                row.owned = false;
                row.owned_hd = false;
                row.owned_modified = None;
                self.owned_match_stats.misses += 1;
                continue;
            };
            row.owned = true;
            if let Some(key) = found.title_key {
                row.owned_hd = self
                    .owned_hd_keys
                    .as_ref()
                    .is_some_and(|set| set.contains(&key));
                self.owned_match_stats.by_key += 1;
            } else {
                row.owned_hd = library
                    .is_some_and(|lib| found.candidates.iter().any(|c| lib.movies[c.movie].hd));
                self.owned_match_stats.by_guid += 1;
            }
            // A single file date is meaningful only for an unambiguous match.
            row.owned_modified = match (library, found.candidates) {
                (Some(lib), [c]) => lib.movies[c.movie].updated_at,
                _ => None,
            };
        }
    }

    pub(super) fn ui_owned_details(&self, ui: &mut eg::Ui, row: &PosterRow) {
        let Some(library) = &self.owned_library else {
            ui.label(if self.owned_scan_in_progress {
                "This badge comes from the saved title list. The current scan will look up the Plex file."
            } else {
                "This badge comes from the saved title list. Choose Refresh owned scan in Advanced to look up the Plex file."
            });
            return;
        };
        let Some(found) = self.owned_keys.as_ref().and_then(|keys| {
            lookup(
                &row.title,
                row.year,
                row.guid.as_deref(),
                keys,
                Some(library),
            )
        }) else {
            return;
        };
        ui.label(found.explanation(row.year));
        if found.candidates.len() > 1 {
            ui.label(format!(
                "{} Plex movies match this airing.",
                found.candidates.len()
            ));
        }
        for candidate in found.candidates {
            let movie = &library.movies[candidate.movie];
            let title = match movie.year {
                Some(year) => format!("{} ({year})", movie.title),
                None => format!("{} (year unknown)", movie.title),
            };
            ui.strong(title);
            match candidate.field {
                OwnedField::OriginalTitle => {
                    ui.label("Matched the original title in Plex.");
                }
                OwnedField::Filename => {
                    ui.label("Matched the filename in Plex.");
                }
                _ => {}
            }
            ui.add(eg::Label::new(&movie.path).wrap().selectable(true));
            if ui.small_button("Copy file path").clicked() {
                ui.output_mut(|o| o.copied_text = movie.path.clone());
            }
        }
        if let Some(snapshot) = &library.snapshot {
            ui.weak(snapshot);
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
        // Only one side may omit a possessive credit, and both must agree on year.
        let (kind, name) = match Self::credited_title_remainder(title) {
            Some(remainder) => ("bare", remainder),
            None => ("credited", title),
        };
        let credit_key = Self::short_title_key(kind, name, year);
        if keys.contains(&credit_key) {
            return Some(credit_key);
        }
        find("undated")
    }

    fn short_title_key(kind: &str, title: &str, year: i32) -> String {
        format!(
            "{kind}:{}:{year}",
            crate::app::utils::normalize_title(title)
        )
    }

    fn credited_title_remainder(title: &str) -> Option<&str> {
        let title = Self::trailing_title_year(title).map_or(title, |(text, _)| text);
        let (credit, remainder) = title
            .split_once("'s ")
            .or_else(|| title.split_once("’s "))?;
        if credit.split_whitespace().count() < 2 {
            return None;
        }
        let normalized = crate::app::utils::normalize_title(remainder);
        let words = normalized
            .split_whitespace()
            .filter(|w| !matches!(*w, "a" | "an" | "the"));
        (words.take(2).count() == 2).then_some(remainder.trim())
    }

    fn owned_title_variants(title: &str) -> Vec<String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut titles = Vec::new();
        titles.push(trimmed.to_string());

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
                OwnedMsg::Scheduled(result) => {
                    self.scheduled_index = match result {
                        Ok(index) => (!index.is_empty()).then_some(index),
                        Err(err) => {
                            self.record_owned_message(format!(
                                "Could not read recording schedules from the local library copy: {err}. REC badges cleared."
                            ));
                            None
                        }
                    };
                    self.apply_scheduled_flags();
                    self.mark_dirty();
                }
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
                        hd_keys,
                        library,
                    } = *done;
                    self.set_startup_progress_floor(crate::app::STARTUP_STAGE3_DONE);

                    let count = library.movies.len();
                    self.owned_keys = Some(keys);
                    self.owned_hd_keys = Some(hd_keys);
                    self.owned_library = Some(library);
                    self.apply_owned_flags();
                    self.mark_dirty();
                    self.owned_scan_in_progress = false;
                    let unit = if count == 1 {
                        "Plex movie"
                    } else {
                        "Plex movies"
                    };
                    self.record_owned_message(format!("Owned scan complete ({count} {unit})."));
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
