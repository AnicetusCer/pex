// src/app/ui/topbar.rs
use super::super::{DayRange, SortKey};
use eframe::egui as eg;

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

            ui.separator();

            // Channel filter popup trigger
            if ui.button("Channel filter…").clicked() {
                self.show_channel_filter_popup = true;
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

            // Workers
            ui.label("Workers:");
            let workers_resp = ui.add(eg::Slider::new(&mut self.worker_count_ui, 1..=32));
            if workers_resp.changed() {
                self.mark_dirty();
            }
            workers_resp.on_hover_text(
                "Parallel downloads. Typical 8–16. New value applies to next prefetch.",
            );
            if self.prefetch_started && self.loading_progress < 1.0 {
                ui.add_space(6.0);
                ui.label(
                    eg::RichText::new("(new value applies to next prefetch)")
                        .italics()
                        .weak(),
                );
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

                ui.separator();
                if ui.button("Close").clicked() {
                    // Window will close when `open` is set to false afterward
                }
            });

        // Apply result (avoid E0499 by setting after .show)
        self.show_channel_filter_popup = open;
    }
}
