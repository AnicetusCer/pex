// src/app/ui/topbar.rs
use super::super::{DayRange, SortKey};
use eframe::egui as eg;
use std::path::Path;

impl crate::app::PexApp {
    // ---------- TOP BAR ----------
    pub(crate) fn ui_render_topbar(&mut self, ui: &mut eg::Ui) {
        ui.horizontal(|ui| {
            // Day range
            let mut changed_day = false;
            eg::ComboBox::from_id_source("day_window_combo")
                .selected_text(match self.current_range {
                    DayRange::Two => "2 days",
                    DayRange::Four => "4 days",
                    DayRange::Five => "5 days",
                    DayRange::Seven => "7 days",
                    DayRange::Fourteen => "14 days",
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut self.current_range, DayRange::Two, "2 days")
                        .clicked()
                    {
                        changed_day = true;
                    }
                    if ui
                        .selectable_value(&mut self.current_range, DayRange::Four, "4 days")
                        .clicked()
                    {
                        changed_day = true;
                    }
                    if ui
                        .selectable_value(&mut self.current_range, DayRange::Five, "5 days")
                        .clicked()
                    {
                        changed_day = true;
                    }
                    if ui
                        .selectable_value(&mut self.current_range, DayRange::Seven, "7 days")
                        .clicked()
                    {
                        changed_day = true;
                    }
                    if ui
                        .selectable_value(&mut self.current_range, DayRange::Fourteen, "14 days")
                        .clicked()
                    {
                        changed_day = true;
                    }
                });
            if changed_day {
                self.mark_dirty();
            }

            ui.separator();

            // Search
            let resp = ui.add(
                eg::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Title…")
                    .desired_width(160.0),
            );
            if resp.changed() {
                self.mark_dirty();
            }

            if ui
                .toggle_value(&mut self.filter_hd_only, "HD only")
                .on_hover_text("Show only broadcast HD airings")
                .changed()
            {
                self.mark_dirty();
            }

            ui.separator();

            // Channel filter popup trigger
            if ui.button("Channel filter…").clicked() {
                self.show_channel_filter_popup = true;
            }

            if ui.button("Genre filter…").clicked() {
                self.show_genre_filter_popup = true;
            }

            // Clear active channel filter (only when something is selected)
            if !self.selected_channels.is_empty() {
                if ui
                    .small_button("Clear channels")
                    .on_hover_text("Clear the channel include-only filter")
                    .clicked()
                {
                    self.selected_channels.clear();
                    self.mark_dirty();
                }
            }

            ui.separator();

            if ui.button("Advanced…").clicked() {
                self.show_advanced_popup = true;
            }

            ui.separator();

            // Sort
            let mut changed_sort = false;
            eg::ComboBox::from_id_source("sort_by_combo")
                .selected_text(match self.sort_key {
                    SortKey::Time => "Sort: Time",
                    SortKey::Title => "Sort: Title",
                    SortKey::Channel => "Sort: Channel",
                    SortKey::Genre => "Sort: Genre",
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut self.sort_key, SortKey::Time, "Time")
                        .clicked()
                    {
                        changed_sort = true;
                    }
                    if ui
                        .selectable_value(&mut self.sort_key, SortKey::Title, "Title")
                        .clicked()
                    {
                        changed_sort = true;
                    }
                    if ui
                        .selectable_value(&mut self.sort_key, SortKey::Channel, "Channel")
                        .clicked()
                    {
                        changed_sort = true;
                    }
                    if ui
                        .selectable_value(&mut self.sort_key, SortKey::Genre, "Genre")
                        .clicked()
                    {
                        changed_sort = true;
                    }
                });
            if changed_sort {
                self.mark_dirty();
            }
            if ui.checkbox(&mut self.sort_desc, "Desc").changed() {
                self.mark_dirty();
            }

            ui.separator();

            // Poster size
            ui.label("Poster:");
            if ui
                .add(eg::Slider::new(&mut self.poster_width_ui, 120.0..=220.0).suffix(" px"))
                .changed()
            {
                self.mark_dirty();
            }

            ui.separator();

            // Owned controls
            if ui.checkbox(&mut self.hide_owned, "Hide owned").changed() {
                self.mark_dirty();
            }
            let mut dim_changed = ui.checkbox(&mut self.dim_owned, "Dim owned").changed();
            if self.dim_owned {
                if ui
                    .add(eg::Slider::new(&mut self.dim_strength_ui, 0.10..=0.90).text("Darken %"))
                    .changed()
                {
                    dim_changed = true;
                }
            }
            if dim_changed {
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
        let mut channels: Vec<String> =
            self.rows.iter().filter_map(|r| r.channel.clone()).collect();
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
                        let label = crate::app::utils::humanize_channel(ch);
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

        let mut genres: Vec<String> = self
            .rows
            .iter()
            .flat_map(|r| r.genres.clone())
            .collect();
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
        let ctx_clone = ctx.clone();
        let cfg = crate::config::load_config();
        let db_path_str = cfg
            .plex_db_local
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("plex_epg.db");
        let db_exists = Path::new(db_path_str).exists();
        let using_demo_omdb = cfg.omdb_api_key.is_none();

        eg::Window::new("Advanced controls")
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        eg::RichText::new(format!(
                            "Plex DB: {}",
                            db_path_str
                        ))
                        .color(if db_exists {
                            eg::Color32::LIGHT_GREEN
                        } else {
                            eg::Color32::LIGHT_RED
                        }),
                    );
                    if !db_exists {
                        ui.label(
                            eg::RichText::new("Database file missing at the configured path.")
                                .color(eg::Color32::LIGHT_RED),
                        );
                    }
                    if using_demo_omdb {
                        ui.label(
                            eg::RichText::new(
                                "Using demo OMDb key (config omdb_api_key not set).",
                            )
                            .weak(),
                        );
                    }
                    ui.separator();

                    ui.label(eg::RichText::new("Prefetch workers").strong());
                    let workers_resp =
                        ui.add(eg::Slider::new(&mut self.worker_count_ui, 1..=32).text("Threads"));
                    if workers_resp.changed() {
                        self.mark_dirty();
                    }
                    workers_resp.on_hover_text(
                        "Parallel downloads. Typical 8–16. New value applies to next prefetch.",
                    );

                    ui.separator();
                    ui.label(eg::RichText::new("Poster cache").strong());
                    if let Some(limit) = crate::app::cache::poster_cache_limit() {
                        ui.label(
                            eg::RichText::new(format!(
                                "Current limit: {} files",
                                limit
                            ))
                            .weak(),
                        );
                    } else {
                        ui.label(
                            eg::RichText::new("Current limit: unlimited (set poster_cache_max_files in config.json)")
                                .weak(),
                        );
                    }
                    if ui.button("Clear poster cache").clicked() {
                        match self.clear_poster_cache_files() {
                            Ok(removed) => {
                                self.restart_poster_pipeline(&ctx_clone);
                                self.advanced_feedback = Some(format!(
                                    "Poster cache cleared (removed {removed} files). Prefetch restarting…"
                                ));
                                self.set_status("Poster cache cleared; restarting prefetch…");
                            }
                            Err(err) => {
                                let msg = format!("Poster cache clear failed: {err}");
                                self.advanced_feedback = Some(msg.clone());
                                self.set_status(msg);
                            }
                        }
                    }
                    if ui.button("Prune over limit").clicked() {
                        match crate::app::cache::prune_poster_cache_now() {
                            Ok(removed) => {
                                self.advanced_feedback = Some(format!(
                                    "Pruned {removed} poster file(s) beyond limit."
                                ));
                            }
                            Err(err) => {
                                let msg = format!("Poster prune failed: {err}");
                                self.advanced_feedback = Some(msg.clone());
                                self.set_status(msg);
                            }
                        }
                    }
                    if ui.button("Refresh poster cache").clicked() {
                        match self.refresh_poster_cache_light() {
                            Ok(removed) => {
                                self.advanced_feedback = Some(format!(
                                    "Poster cache refresh removed {removed} stale file(s)."
                                ));
                                self.set_status("Poster cache refreshed.");
                            }
                            Err(err) => {
                                self.advanced_feedback = Some(format!(
                                    "Poster cache refresh failed: {err}"
                                ));
                                self.set_status("Poster cache refresh failed.");
                            }
                        }
                    }
                    if ui.button("Restart poster prep").clicked() {
                        self.restart_poster_pipeline(&ctx_clone);
                        self.advanced_feedback =
                            Some("Poster prep restarted without clearing cache.".into());
                        self.set_status("Poster prep restarting…");
                    }

                    ui.separator();
                    ui.label(eg::RichText::new("Owned library cache").strong());
                    if ui.button("Clear owned cache").clicked() {
                        match self.clear_owned_cache() {
                            Ok(removed) => {
                                self.record_owned_message(format!(
                                    "Owned cache cleared manually (removed {removed} file{}).",
                                    if removed == 1 { "" } else { "s" }
                                ));
                                self.advanced_feedback = Some(format!(
                                    "Owned cache cleared (removed {removed} files). Rescanning library…"
                                ));
                                self.set_status("Owned cache cleared; rescanning library…");
                            }
                            Err(err) => {
                                let msg = format!("Owned cache clear failed: {err}");
                                self.advanced_feedback = Some(msg.clone());
                                self.set_status(msg.clone());
                                self.record_owned_message(msg);
                            }
                        }
                    }
                    if ui.button("Refresh owned scan").clicked() {
                        self.refresh_owned_scan();
                        self.advanced_feedback =
                            Some("Owned scan refresh started (incremental).".into());
                        self.set_status("Refreshing owned library…");
                    }

                    let owned_running = self.owned_scan_in_progress;
                    let owned_messages: Vec<String> = self
                        .owned_scan_messages
                        .iter()
                        .take(6)
                        .cloned()
                        .collect();

                    ui.add_space(4.0);
                    if owned_running {
                        ui.horizontal(|ui| {
                            ui.add(eg::Spinner::new().size(14.0));
                            ui.label("Owned scan in progress…");
                        });
                    } else {
                        ui.label(eg::RichText::new("Owned scan idle.").weak());
                    }
                    for (idx, msg) in owned_messages.iter().enumerate() {
                        let text = if idx == 0 {
                            eg::RichText::new(msg).strong()
                        } else {
                            eg::RichText::new(msg).weak()
                        };
                        ui.label(text);
                    }

                    ui.separator();
                    ui.label(eg::RichText::new("ffprobe cache").strong());
                    if ui.button("Refresh ffprobe cache").clicked() {
                        match self.refresh_ffprobe_cache() {
                            Ok(removed) => {
                                self.advanced_feedback = Some(format!(
                                    "ffprobe cache refresh removed {removed} stale entry(s)."
                                ));
                                self.set_status("ffprobe cache refreshed.");
                            }
                            Err(err) => {
                                self.advanced_feedback = Some(format!(
                                    "ffprobe cache refresh failed: {err}"
                                ));
                                self.set_status("ffprobe cache refresh failed.");
                            }
                        }
                    }
                    if ui.button("Clear ffprobe cache").clicked() {
                        match self.clear_ffprobe_cache() {
                            Ok(removed) => {
                                if removed {
                                    self.advanced_feedback =
                                        Some("ffprobe cache cleared.".into());
                                } else {
                                    self.advanced_feedback =
                                        Some("ffprobe cache already clear.".into());
                                }
                                self.set_status("ffprobe cache reset.");
                            }
                            Err(err) => {
                                let msg = format!("ffprobe cache clear failed: {err}");
                                self.advanced_feedback = Some(msg.clone());
                                self.set_status(msg);
                            }
                        }
                    }

                    ui.separator();
                    ui.label(eg::RichText::new("Preferences").strong());
                    if ui.button("Backup UI prefs").clicked() {
                        match crate::app::prefs::backup_ui_prefs() {
                            Ok(path) => {
                                self.advanced_feedback =
                                    Some(format!("Prefs backed up to {}", path.display()));
                            }
                            Err(err) => {
                                self.advanced_feedback =
                                    Some(format!("Prefs backup failed: {err}"));
                            }
                        }
                    }
                    if ui.button("Restore latest prefs backup").clicked() {
                        match crate::app::prefs::restore_latest_ui_prefs_backup() {
                            Ok(Some(path)) => {
                                self.load_prefs();
                                self.advanced_feedback = Some(format!(
                                    "Prefs restored from {}",
                                    path.display()
                                ));
                            }
                            Ok(None) => {
                                self.advanced_feedback =
                                    Some("No prefs backups found.".into());
                            }
                            Err(err) => {
                                self.advanced_feedback =
                                    Some(format!("Prefs restore failed: {err}"));
                            }
                        }
                    }

                    if let Some(msg) = &self.advanced_feedback {
                        ui.separator();
                        ui.label(eg::RichText::new(msg).italics());
                    }
                });
            });

        self.show_advanced_popup = open;
    }
}
