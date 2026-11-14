// src/app/ui/topbar.rs
use super::super::{DayRange, SortKey};
use eframe::egui as eg;

impl crate::app::PexApp {
    // ---------- TOP BAR ----------
    pub(crate) fn ui_render_topbar(&mut self, ui: &mut eg::Ui) {
        ui.horizontal(|ui| {
            let mut dirty = false;
            const DAY_OPTIONS: [(DayRange, &str); 5] = [
                (DayRange::Two, "2 days"),
                (DayRange::Four, "4 days"),
                (DayRange::Five, "5 days"),
                (DayRange::Seven, "7 days"),
                (DayRange::Fourteen, "14 days"),
            ];

            let current_label = DAY_OPTIONS
                .iter()
                .find(|(range, _)| *range == self.current_range)
                .map(|(_, label)| *label)
                .unwrap_or("Days");
            eg::ComboBox::from_id_source("day_window_combo")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    for (range, label) in DAY_OPTIONS {
                        if ui
                            .selectable_value(&mut self.current_range, range, label)
                            .clicked()
                        {
                            dirty = true;
                        }
                    }
                });

            ui.separator();

            if ui
                .add(
                    eg::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Title.")
                        .desired_width(160.0),
                )
                .changed()
            {
                dirty = true;
            }
            if !self.search_query.is_empty()
                && ui.small_button("X").on_hover_text("Clear search").clicked()
            {
                self.search_query.clear();
                dirty = true;
            }

            ui.separator();

            let filters_menu_active = self.filter_hd_only
                || self.filter_owned_before_cutoff
                || !self.selected_decades.is_empty()
                || !self.selected_channels.is_empty()
                || !self.selected_genres.is_empty()
                || self.hide_owned
                || self.dim_owned;
            let filters_label: eg::WidgetText = if filters_menu_active {
                eg::RichText::new("Filters").strong().into()
            } else {
                "Filters".into()
            };
            let mut menu_dirty = false;
            ui.menu_button(filters_label, |ui| {
                if ui
                    .checkbox(&mut self.filter_hd_only, "HD only")
                    .on_hover_text("Show only broadcast HD airings")
                    .changed()
                {
                    menu_dirty = true;
                }

                let decades = self.available_decades();
                if !decades.is_empty() {
                    ui.separator();
                    ui.label(eg::RichText::new("Decades").strong());
                    ui.horizontal_wrapped(|ui| {
                        for decade in decades {
                            let mut selected = self.selected_decades.contains(&decade);
                            let label = format!("{decade}s");
                            if ui.checkbox(&mut selected, label).changed() {
                                if selected {
                                    self.selected_decades.insert(decade);
                                } else {
                                    self.selected_decades.remove(&decade);
                                }
                                menu_dirty = true;
                            }
                        }
                    });
                    if !self.selected_decades.is_empty()
                        && ui.small_button("Clear decades").clicked()
                    {
                        self.selected_decades.clear();
                        menu_dirty = true;
                    }
                }

                ui.separator();
                ui.label(eg::RichText::new("Channel & Genre").strong());
                if ui.button("Select channels…").clicked() {
                    self.show_channel_filter_popup = true;
                    ui.close_menu();
                }
                if ui.button("Select genres…").clicked() {
                    self.show_genre_filter_popup = true;
                    ui.close_menu();
                }
                if !self.selected_channels.is_empty() && ui.small_button("Clear channels").clicked()
                {
                    self.selected_channels.clear();
                    menu_dirty = true;
                }
                if !self.selected_genres.is_empty() && ui.small_button("Clear genres").clicked() {
                    self.selected_genres.clear();
                    menu_dirty = true;
                }

                ui.separator();
                ui.label(eg::RichText::new("Owned recorded before").strong());
                let checkbox_label = format!("Enable cutoff ({})", self.owned_before_cutoff_input);
                if ui
                    .checkbox(&mut self.filter_owned_before_cutoff, checkbox_label)
                    .on_hover_text("Only show owned titles recorded before the cutoff date")
                    .changed()
                {
                    menu_dirty = true;
                }
                let date_response = ui
                    .add(
                        eg::TextEdit::singleline(&mut self.owned_before_cutoff_input)
                            .hint_text("YYYY-MM-DD"),
                    )
                    .on_hover_text("Set the owned recording cutoff date (UTC midnight)");
                if date_response.changed() {
                    let input = self.owned_before_cutoff_input.clone();
                    self.set_owned_cutoff_from_str(&input);
                    // Always mark dirty so prefs capture the user's input.
                    menu_dirty = true;
                }
                if !self.owned_before_cutoff_valid {
                    ui.colored_label(
                        eg::Color32::from_rgb(200, 80, 80),
                        "Use YYYY-MM-DD (e.g. 2022-12-25)",
                    );
                }
                if ui.small_button("Reset cutoff").clicked() {
                    self.reset_owned_cutoff_to_default();
                    menu_dirty = true;
                }

                ui.separator();
                ui.label(eg::RichText::new("Owned view").strong());
                let hide_resp =
                    ui.checkbox(&mut self.hide_owned, "Hide owned (except HD upgrades)");
                if hide_resp.changed() {
                    menu_dirty = true;
                }
                let dim_resp = ui.checkbox(&mut self.dim_owned, "Dim owned");
                let dim_toggled = dim_resp.changed();
                let slider_changed = if self.dim_owned {
                    ui.add(eg::Slider::new(&mut self.dim_strength_ui, 0.10..=0.90).text("Darken %"))
                        .changed()
                } else {
                    false
                };
                if dim_toggled || slider_changed {
                    menu_dirty = true;
                }
            });
            if menu_dirty {
                dirty = true;
            }

            ui.separator();

            if ui.button("Advanced.").clicked() {
                self.advanced_paths.start(ui.ctx());
                self.show_advanced_popup = true;
            }
            if ui.button("Connect to Plex").clicked() {
                self.open_connection_setup();
            }

            ui.separator();

            const SORT_OPTIONS: [(SortKey, &str); 4] = [
                (SortKey::Time, "Sort: Time"),
                (SortKey::Title, "Sort: Title"),
                (SortKey::Channel, "Sort: Channel"),
                (SortKey::Genre, "Sort: Genre"),
            ];
            let sort_label = SORT_OPTIONS
                .iter()
                .find(|(key, _)| *key == self.sort_key)
                .map(|(_, label)| *label)
                .unwrap_or("Sort");
            eg::ComboBox::from_id_source("sort_by_combo")
                .selected_text(sort_label)
                .show_ui(ui, |ui| {
                    for (key, label) in SORT_OPTIONS {
                        if ui
                            .selectable_value(&mut self.sort_key, key, label)
                            .clicked()
                        {
                            dirty = true;
                        }
                    }
                });
            if ui.checkbox(&mut self.sort_desc, "Desc").changed() {
                dirty = true;
            }

            ui.separator();

            ui.label("Poster:");
            if ui
                .add(eg::Slider::new(&mut self.poster_width_ui, 120.0..=220.0).suffix(" px"))
                .changed()
            {
                dirty = true;
            }

            if dirty {
                self.mark_dirty();
            }
        });
    }

    // ---------- CHANNEL FILTER POPUP ----------
    pub(crate) fn ui_render_channel_filter_popup(&mut self, ctx: &eg::Context) {
        if !self.show_channel_filter_popup {
            return;
        }

        // Build channel list from current rows (raw values; UI presents humanized label)
        let mut channels: Vec<String> = self
            .rows
            .iter()
            .filter_map(|r| r.channel_raw.clone())
            .collect();
        channels.sort();
        channels.dedup();

        let mut open = self.show_channel_filter_popup;
        eg::Window::new("Channel filter")
            .collapsible(false)
            .resizable(true)
            .default_width(320.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(eg::RichText::new("Include only these channels:").strong());
                    if ui.small_button("Select all").clicked() {
                        self.selected_channels = channels.iter().cloned().collect();
                        self.mark_dirty();
                    }
                    if ui.small_button("Select none").clicked() {
                        self.selected_channels.clear();
                        self.mark_dirty();
                    }
                    if !self.selected_channels.is_empty() && ui.small_button("Clear").clicked() {
                        self.selected_channels.clear();
                        self.mark_dirty();
                    }
                });

                ui.separator();
                eg::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for ch in channels.iter() {
                        let mut checked = self.selected_channels.contains(ch);
                        let label = self
                            .rows
                            .iter()
                            .find(|r| r.channel_raw.as_deref() == Some(ch.as_str()))
                            .and_then(|r| r.channel.clone())
                            .unwrap_or_else(|| crate::app::utils::humanize_channel(ch));
                        if ui.checkbox(&mut checked, label).clicked() {
                            if checked {
                                self.selected_channels.insert(ch.clone());
                            } else {
                                self.selected_channels.remove(ch);
                            }
                            self.mark_dirty();
                        }
                    }
                });
            });

        // Apply result (avoid E0499 by setting after .show)
        self.show_channel_filter_popup = open;
    }

    pub(crate) fn ui_render_genre_filter_popup(&mut self, ctx: &eg::Context) {
        if !self.show_genre_filter_popup {
            return;
        }

        let mut genres: Vec<String> = self.rows.iter().flat_map(|r| r.genres.clone()).collect();
        genres.sort();
        genres.dedup();

        let mut open = self.show_genre_filter_popup;
        eg::Window::new("Genre filter")
            .collapsible(false)
            .resizable(true)
            .default_width(280.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(eg::RichText::new("Include only these genres:").strong());
                    if ui.small_button("Select all").clicked() {
                        self.selected_genres = genres.iter().cloned().collect();
                        self.mark_dirty();
                    }
                    if ui.small_button("Select none").clicked() {
                        self.selected_genres.clear();
                        self.mark_dirty();
                    }
                    if !self.selected_genres.is_empty() && ui.small_button("Clear").clicked() {
                        self.selected_genres.clear();
                        self.mark_dirty();
                    }
                });

                ui.separator();
                eg::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for genre in genres.iter() {
                        let mut checked = self.selected_genres.contains(genre);
                        if ui.checkbox(&mut checked, genre).clicked() {
                            if checked {
                                self.selected_genres.insert(genre.clone());
                            } else {
                                self.selected_genres.remove(genre);
                            }
                            self.mark_dirty();
                        }
                    }
                });
            });

        self.show_genre_filter_popup = open;
    }

    pub(crate) fn ui_render_advanced_popup(&mut self, ctx: &eg::Context) {
        if !self.show_advanced_popup {
            return;
        }

        let mut open = self.show_advanced_popup;
        eg::Window::new("Advanced controls")
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .vscroll(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Ratings key…").clicked() {
                            self.open_rating_key_editor(None);
                        }
                        ui.label(self.rating_key_feedback.unwrap_or(if self.has_ratings_key {
                            "TMDb key saved"
                        } else {
                            "Optional TMDb film ratings"
                        }));
                    });
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Plex DVR Schedule…").clicked() {
                            self.open_live_dvr_settings();
                        }
                        if self.live_dvr_configured()
                            && ui
                                .add_enabled(
                                    !self.live_dvr_busy(),
                                    eg::Button::new("Refresh DVR schedule"),
                                )
                                .clicked()
                        {
                            self.refresh_live_dvr();
                        }
                    });
                    if let Some(status) = &self.live_dvr_status {
                        ui.label(status);
                    }
                    ui.separator();
                    ui.strong("Path status");
                    self.advanced_paths.show(ui);
                    ui.separator();
                    self.advanced_prefetch_controls(ui);
                    ui.separator();
                    self.advanced_poster_controls(ui, ctx);
                    ui.separator();
                    self.advanced_owned_controls(ui);
                    ui.separator();
                    self.advanced_prefs_controls(ui);
                    self.advanced_feedback_section(ui);
                    ui.separator();
                    ui.label(format!(
                        "Owned matches: guid={} key={} miss={}",
                        self.owned_match_stats.by_guid,
                        self.owned_match_stats.by_key,
                        self.owned_match_stats.misses
                    ));
                    ui.label(format!(
                        "Scheduled matches: guid={} title/time={} miss={}",
                        self.scheduled_match_stats.by_guid,
                        self.scheduled_match_stats.by_title_slot,
                        self.scheduled_match_stats.misses
                    ));
                });
            });

        self.show_advanced_popup = open;
    }

    fn advanced_prefetch_controls(&mut self, ui: &mut eg::Ui) {
        ui.label(eg::RichText::new("Prefetch workers").strong());
        let workers_resp =
            ui.add(eg::Slider::new(&mut self.worker_count_ui, 1..=32).text("Threads"));
        if workers_resp.changed() {
            self.mark_dirty();
        }
        workers_resp
            .on_hover_text("Parallel downloads. Typical 8-16. New value applies to next prefetch.");
    }

    fn advanced_poster_controls(&mut self, ui: &mut eg::Ui, ctx: &eg::Context) {
        ui.label(eg::RichText::new("Poster cache").strong());
        ui.label(
            eg::RichText::new("Posters older than 14 days are pruned automatically on startup.")
                .weak(),
        );
        // Finish active writes before removing the files and starting a fresh queue.
        let can_clear = self.in_flight() == 0;
        if ui
            .add_enabled(can_clear, eg::Button::new("Clear & rebuild poster cache"))
            .on_disabled_hover_text("Available when poster downloads finish")
            .clicked()
        {
            match crate::app::cache::clear_poster_cache() {
                Ok(removed) => {
                    self.restart_poster_pipeline(ctx);
                    self.advanced_feedback = Some(format!(
                        "Poster cache cleared (removed {removed} files) and prefetch restarting."
                    ));
                    self.set_status("Poster cache cleared; restarting prefetch.");
                }
                Err(err) => {
                    let msg = format!("Poster cache clear failed: {err}");
                    self.advanced_feedback = Some(msg.clone());
                    self.set_status(msg);
                }
            }
        }
    }

    fn advanced_owned_controls(&mut self, ui: &mut eg::Ui) {
        ui.label(eg::RichText::new("Owned library cache").strong());
        if ui
            .add_enabled(
                self.owned_scan_available && !self.owned_scan_in_progress,
                eg::Button::new("Refresh owned scan"),
            )
            .clicked()
        {
            self.refresh_owned_scan();
            self.advanced_feedback = Some("Owned scan refresh started.".into());
            self.set_status("Refreshing owned library.");
        }

        let owned_running = self.owned_scan_in_progress;
        let owned_messages: Vec<String> =
            self.owned_scan_messages.iter().take(6).cloned().collect();

        ui.add_space(4.0);
        if owned_running {
            ui.horizontal(|ui| {
                ui.add(eg::Spinner::new().size(14.0));
                ui.label("Owned scan in progress.");
            });
        } else {
            ui.label(
                eg::RichText::new(self.owned_notice.as_deref().unwrap_or("Owned scan ready."))
                    .weak(),
            );
        }
        for (idx, msg) in owned_messages.iter().enumerate() {
            let text = if idx == 0 {
                eg::RichText::new(msg).strong()
            } else {
                eg::RichText::new(msg).weak()
            };
            ui.label(text);
        }
    }

    fn advanced_prefs_controls(&mut self, ui: &mut eg::Ui) {
        ui.label(eg::RichText::new("Preferences").strong());
        if ui.button("Backup UI prefs").clicked() {
            match crate::app::prefs::backup_ui_prefs() {
                Ok(path) => {
                    self.advanced_feedback = Some(format!("Prefs backed up to {}", path.display()));
                }
                Err(err) => {
                    self.advanced_feedback = Some(format!("Prefs backup failed: {err}"));
                }
            }
        }
        if ui.button("Restore latest prefs backup").clicked() {
            match crate::app::prefs::restore_latest_ui_prefs_backup() {
                Ok(Some(path)) => {
                    self.load_prefs();
                    self.advanced_feedback =
                        Some(format!("Prefs restored from {}", path.display()));
                }
                Ok(None) => {
                    self.advanced_feedback = Some("No prefs backups found.".into());
                }
                Err(err) => {
                    self.advanced_feedback = Some(format!("Prefs restore failed: {err}"));
                }
            }
        }
    }

    fn advanced_feedback_section(&self, ui: &mut eg::Ui) {
        if let Some(msg) = &self.advanced_feedback {
            ui.separator();
            ui.label(eg::RichText::new(msg).italics());
        }
    }
}
