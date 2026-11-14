// src/app/detail.rs
use crate::app::types::{AiringQuality, RatingState};
use eframe::egui as eg;

impl crate::app::PexApp {
    pub(crate) fn ui_render_detail_panel(&mut self, ctx: &eg::Context) {
        // Use poster/card sizing to keep the panel width within a sane range
        let screen_w: f32 = ctx.input(|i| i.screen_rect().width());
        let step: f32 = (self.poster_width_ui + crate::app::ui::grid::H_SPACING).max(1.0); // poster + gutter
        let max_w: f32 = (screen_w * 0.45).clamp(360.0, 520.0);
        let min_w: f32 = 260.0;
        let default_width = self.detail_panel_width.clamp(min_w, max_w);

        let mut trigger_rating_request: Option<usize> = None;
        let mut replace_key_for = None;

        let panel = eg::SidePanel::right("detail_panel")
            .resizable(true)
            .default_width(default_width)
            .min_width(min_w)
            .max_width(max_w)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading("Details");
                    ui.with_layout(eg::Layout::right_to_left(eg::Align::Center), |ui| {
                        if ui.button("Clear").clicked() {
                            self.selected_idx = None;
                        }
                    });
                });
                ui.separator();

                eg::ScrollArea::vertical()
                    .id_source("detail_content")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let Some(sel) = self.selected_idx else {
                            ui.label("Select a film from the grid to see details.");
                            return;
                        };
                        let Some(row) = self.rows.get(sel) else {
                            ui.label("Selection is out of range.");
                            return;
                        };

                        let airing_quality = row.airing_quality;
                        let owned_is_hd = row.owned_hd;

                        // Snapshot values so we can release the immutable borrow on self.rows
                        let poster_tex = row.tex.clone();
                        let title_text = row.title.clone();
                        let year = row.year;
                        let channel_display = channel_name(row);
                        let channel_thumb = row.channel_thumb.clone();
                        let airing = row.airing;
                        let critic_rating = row.critic_rating;
                        let audience_rating = row.audience_rating;
                        let owned = row.owned;
                        let owned_modified = row.owned_modified;
                        let genres = row.genres.clone();
                        let summary = row.summary.clone();
                        let film_key = row.film_key.clone();
                        let scheduled = row.scheduled;

                        // Poster preview (uses small texture if available)
                        ui.add_space(4.0);
                        let avail_w = ui.available_width().clamp(120.0, 520.0);
                        let poster_size = eg::vec2(avail_w, avail_w * 1.5);

                        if let Some(tex) = poster_tex {
                            ui.image((tex.id(), poster_size));
                        } else {
                            // Placeholder if texture not ready
                            let (rect, _resp) =
                                ui.allocate_exact_size(poster_size, eg::Sense::hover());
                            ui.painter()
                                .rect_filled(rect, 8.0, eg::Color32::from_gray(40));
                            ui.painter().text(
                                rect.center(),
                                eg::Align2::CENTER_CENTER,
                                if row.state == super::PosterState::Cached
                                    || (row.state == super::PosterState::Pending
                                        && !row.url.is_empty()
                                        && self.in_flight() > 0)
                                {
                                    "Poster loading…"
                                } else {
                                    "No poster available"
                                },
                                eg::FontId::proportional(14.0),
                                eg::Color32::WHITE,
                            );
                        }

                        ui.add_space(8.0);

                        // Title (YYYY)
                        let title_display = year.map_or_else(
                            || title_text.clone(),
                            |y| format!("{} ({})", title_text, y),
                        );
                        let channel_icon = channel_thumb
                            .as_deref()
                            .and_then(|thumb| self.channel_icon_texture(ctx, thumb));
                        ui.horizontal(|ui| {
                            if let Some(tex) = channel_icon {
                                let tex_size = tex.size();
                                let native_w = tex_size[0].max(1) as f32;
                                let native_h = tex_size[1].max(1) as f32;
                                let target_h = 64.0;
                                let target_w = (native_w / native_h * target_h).clamp(48.0, 120.0);
                                ui.image((tex.id(), eg::vec2(target_w, target_h)));
                                ui.add_space(8.0);
                            }

                            let scroll_width = (ui.available_width() - 40.0).max(100.0);
                            eg::ScrollArea::horizontal()
                                .id_source(("detail_title_scroll", &film_key))
                                .max_width(scroll_width)
                                .show(ui, |ui| {
                                    ui.heading(&title_display);
                                });

                            if ui
                                .small_button("📋")
                                .on_hover_text("Copy title to clipboard")
                                .clicked()
                            {
                                ctx.output_mut(|o| o.copied_text = title_text.clone());
                            }
                        });

                        // Channel + time line (humanized)
                        if let Some(airing) = airing {
                            ui.weak(format!("{channel_display}  •  {}", format_airtime(airing)));
                        }

                        if let Some(label) = scheduled.detail() {
                            ui.label(
                                eg::RichText::new(label)
                                    .color(eg::Color32::from_rgb(220, 80, 80))
                                    .strong(),
                            );
                        }

                        let showings = self.filtered_showings_for(sel);
                        if showings.len() > 1 {
                            eg::CollapsingHeader::new(format!(
                                "Other showings ({})",
                                showings.len() - 1
                            ))
                            .id_source(("other_showings", &film_key))
                            .show(ui, |ui| {
                                for idx in showings.into_iter().filter(|&idx| idx != sel) {
                                    let other = &self.rows[idx];
                                    if let Some(time) = other.airing {
                                        ui.label(format!(
                                            "{}  •  {}",
                                            format_airtime(time),
                                            channel_name(other)
                                        ));
                                        let quality = other.airing_quality.label();
                                        let recording = other.scheduled.detail();
                                        if quality.is_some() || recording.is_some() {
                                            ui.horizontal_wrapped(|ui| {
                                                if let Some(label) = quality {
                                                    ui.weak(label);
                                                }
                                                if let Some(label) = recording {
                                                    ui.strong(label);
                                                }
                                            });
                                        }
                                    }
                                }
                            });
                        }

                        if critic_rating.is_some() || audience_rating.is_some() {
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                if let Some(r) = critic_rating {
                                    ui.label(
                                        eg::RichText::new(format!("Critics: {r:.1}/10"))
                                            .color(eg::Color32::from_rgb(255, 208, 121)),
                                    );
                                }
                                if let Some(r) = audience_rating {
                                    ui.label(
                                        eg::RichText::new(format!("Audience: {r:.1}/10"))
                                            .color(eg::Color32::from_rgb(160, 220, 160)),
                                    );
                                }
                            });
                        }

                        ui.add_space(6.0);
                        let rating_state = self.rating_state_for_key(&film_key);
                        ui.horizontal_wrapped(|ui| {
                            let fetch_enabled = !matches!(rating_state, RatingState::Pending);
                            if ui
                                .add_enabled(fetch_enabled, eg::Button::new("⭐ Rating"))
                                .on_hover_text("Fetch TMDb rating on demand")
                                .clicked()
                            {
                                trigger_rating_request = Some(sel);
                            }
                            ui.add_space(6.0);
                            match rating_state {
                                RatingState::Pending => {
                                    ui.add(eg::Spinner::new().size(14.0));
                                    ui.label("Fetching TMDb rating…");
                                }
                                RatingState::Success(ref txt) => {
                                    ui.label(eg::RichText::new(txt).strong());
                                }
                                RatingState::NotFound => {
                                    ui.label(eg::RichText::new("TMDb rating not found.").weak());
                                }
                                RatingState::KeyRejected => {
                                    ui.label("TMDb rejected this API key.");
                                    if ui.button("Change ratings key…").clicked() {
                                        replace_key_for =
                                            self.rows.get(sel).map(super::RatingLookup::from);
                                    }
                                }
                                RatingState::Error(ref err) => {
                                    ui.label(
                                        eg::RichText::new(format!("Rating error: {err}"))
                                            .color(eg::Color32::LIGHT_RED),
                                    );
                                }
                                RatingState::Idle => {
                                    ui.label(eg::RichText::new("No rating fetched yet.").weak());
                                }
                            }
                        });

                        let quality = airing_quality.label();
                        if quality.is_some() || owned {
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                if let Some(label) = quality {
                                    let color = if airing_quality == AiringQuality::Hd {
                                        eg::Color32::from_rgb(120, 180, 255)
                                    } else {
                                        eg::Color32::GRAY
                                    };
                                    ui.add(
                                        eg::Label::new(eg::RichText::new(label).color(color))
                                            .wrap(),
                                    );
                                }

                                // Owned chip (Owned HD / Owned SD)
                                if owned {
                                    let (txt, col) = if owned_is_hd {
                                        ("Owned HD", eg::Color32::from_rgb(130, 200, 130))
                                    } else {
                                        ("Owned SD", eg::Color32::from_gray(200))
                                    };
                                    ui.add(eg::Label::new(eg::RichText::new(txt).color(col)));

                                    if let Some(ts) = owned_modified {
                                        if let Some(date_str) =
                                            crate::app::utils::format_owned_timestamp(ts)
                                        {
                                            ui.add_space(6.0);
                                            ui.label(
                                                eg::RichText::new(format!(
                                                    "Owned file recorded: {}",
                                                    date_str
                                                ))
                                                .weak(),
                                            );
                                        }
                                    }
                                }
                            });
                            if owned {
                                ui.collapsing("Owned Details", |ui| {
                                    self.ui_owned_details(ui, &self.rows[sel]);
                                });
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        // Description
                        ui.label(eg::RichText::new("Description").strong());
                        if let Some(summary) = summary.as_deref() {
                            ui.add(eg::Label::new(eg::RichText::new(summary)).wrap());
                        } else {
                            ui.label(
                                eg::RichText::new("No description available.")
                                    .italics()
                                    .weak(),
                            );
                        }

                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);

                        // Genres
                        if !genres.is_empty() {
                            ui.label(eg::RichText::new("Genres").strong());
                            ui.add(eg::Label::new(genres.join(", ")).wrap());
                        } else {
                            ui.label(eg::RichText::new("Genres").weak());
                            ui.label("—");
                        }
                    });
            });

        // Persist the width so it sticks between runs
        let actual_w = panel.response.rect.width().clamp(min_w, max_w);
        if (actual_w - self.detail_panel_width).abs() > (step * 0.05).max(0.5) {
            self.detail_panel_width = actual_w;
            self.mark_dirty(); // let your prefs autosave pick this up
        }

        if let Some(idx) = trigger_rating_request {
            self.request_rating_for(idx);
        }
        if let Some(film) = replace_key_for {
            self.open_rating_key_editor(Some(film));
        }
    }
}

fn channel_name(row: &super::PosterRow) -> String {
    row.channel
        .clone()
        .or_else(|| {
            row.channel_raw
                .as_deref()
                .map(super::utils::humanize_channel)
        })
        .unwrap_or_else(|| "Unknown channel".into())
}

fn format_airtime(time: std::time::SystemTime) -> String {
    let bucket = super::utils::day_bucket(time);
    let (_, _, day) = super::utils::civil_from_days(bucket);
    let weekday = super::utils::weekday_full_from_bucket(bucket);
    let suffix = super::utils::ordinal_suffix(day);
    let hhmm = super::utils::hhmm_utc(time);
    format!("{weekday} {day}{suffix} {hhmm} UTC")
}
