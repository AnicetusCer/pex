// src/app/detail.rs
use crate::app::types::RatingState;
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

                let Some(sel) = self.selected_idx else {
                    ui.label("Select a film from the grid to see details.");
                    return;
                };
                let Some(row) = self.rows.get(sel) else {
                    ui.label("Selection is out of range.");
                    return;
                };

                let broadcast_hd = Self::row_broadcast_hd(row);
                let owned_is_hd = self.row_owned_is_hd(row);

                // Snapshot values so we can release the immutable borrow on self.rows
                let poster_tex = row.tex.clone();
                let title_text = row.title.clone();
                let year = row.year;
                let channel_display = row.channel.clone();
                let channel_raw = row.channel_raw.clone();
                let channel_thumb = row.channel_thumb.clone();
                let airing = row.airing;
                let critic_rating = row.critic_rating;
                let audience_rating = row.audience_rating;
                let owned = row.owned;
                let owned_modified = row.owned_modified;
                let genres = row.genres.clone();
                let summary = row.summary.clone();
                let poster_key = row.key.clone();
                let scheduled = row.scheduled;

                // Poster preview (uses small texture if available)
                ui.add_space(4.0);
                let avail_w = ui.available_width().clamp(120.0, 520.0);
                let poster_size = eg::vec2(avail_w, avail_w * 1.5);

                if let Some(tex) = poster_tex {
                    ui.image((tex.id(), poster_size));
                } else {
                    // Placeholder if texture not ready
                    let (rect, _resp) = ui.allocate_exact_size(poster_size, eg::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 8.0, eg::Color32::from_gray(40));
                    ui.painter().text(
                        rect.center(),
                        eg::Align2::CENTER_CENTER,
                        "Poster loading…",
                        eg::FontId::proportional(14.0),
                        eg::Color32::WHITE,
                    );
                }

                ui.add_space(8.0);

                // Title (YYYY)
                let title_display = match year {
                    Some(y) => format!("{} ({})", title_text, y),
                    None => title_text.clone(),
                };
                let channel_icon = channel_thumb
                    .as_deref()
                    .and_then(|thumb| self.channel_icon_texture(ctx, thumb));

                ui.horizontal(|ui| {
                    if let Some(tex) = channel_icon {
                        let size = eg::vec2(64.0, 64.0);
                        ui.image((tex.id(), size));
                        ui.add_space(6.0);
                    }

                    let scroll_width = (ui.available_width() - 40.0).max(100.0);
                    eg::ScrollArea::horizontal()
                        .id_source(("detail_title_scroll", poster_key.clone()))
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
                if channel_display.is_some() || airing.is_some() {
                    let ch = channel_display
                        .clone()
                        .or_else(|| {
                            channel_raw
                                .as_ref()
                                .map(|raw| crate::app::utils::humanize_channel(raw))
                        })
                        .unwrap_or_else(|| "—".into());
                    let schedule = airing
                        .map(|ts| {
                            let bucket = crate::app::utils::day_bucket(ts);
                            let (_, _, day) = crate::app::utils::civil_from_days(bucket);
                            let weekday = crate::app::utils::weekday_full_from_bucket(bucket);
                            let suffix = crate::app::utils::ordinal_suffix(day);
                            let hhmm = crate::app::utils::hhmm_utc(ts);
                            format!("{weekday} {day}{suffix} {hhmm} UTC")
                        })
                        .unwrap_or_else(|| "— UTC".into());
                    ui.label(eg::RichText::new(format!("{ch}  •  {schedule}")).weak());
                }

                if scheduled {
                    ui.label(
                        eg::RichText::new("Scheduled to record")
                            .color(eg::Color32::from_rgb(220, 80, 80))
                            .strong(),
                    );
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
                let rating_state = self.rating_state_for_key(&poster_key);
                ui.horizontal(|ui| {
                    let fetch_enabled = !matches!(rating_state, RatingState::Pending);
                    if ui
                        .add_enabled(fetch_enabled, eg::Button::new("⭐ Rating"))
                        .on_hover_text("Fetch IMDb rating on demand")
                        .clicked()
                    {
                        trigger_rating_request = Some(sel);
                    }
                    ui.add_space(6.0);
                    match rating_state {
                        RatingState::Pending => {
                            ui.add(eg::Spinner::new().size(14.0));
                            ui.label("Fetching IMDb rating…");
                        }
                        RatingState::Success(ref txt) => {
                            ui.label(eg::RichText::new(txt).strong());
                        }
                        RatingState::NotFound => {
                            ui.label(eg::RichText::new("IMDb rating not found.").weak());
                        }
                        RatingState::Error(ref err) => {
                            ui.label(
                                eg::RichText::new(format!("Rating error: {err}"))
                                    .color(eg::Color32::LIGHT_RED),
                            );
                        }
                        RatingState::MissingApiKey => {
                            ui.label(
                                eg::RichText::new(
                                    "Set omdb_api_key in config.json to enable ratings.",
                                )
                                .weak(),
                            );
                        }
                        RatingState::Idle => {
                            ui.label(eg::RichText::new("No rating fetched yet.").weak());
                        }
                    }
                });

                // --- Owned tags (explicit SD/HD) + optional Airing status ---
                {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        // Airing chip (HD/SD)
                        ui.add(
                            eg::Label::new(
                                eg::RichText::new(if broadcast_hd {
                                    "Airing HD"
                                } else {
                                    "Airing SD"
                                })
                                .color(if broadcast_hd {
                                    eg::Color32::from_rgb(120, 180, 255)
                                } else {
                                    eg::Color32::GRAY
                                }),
                            )
                            .wrap(),
                        );

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

        // Persist the width so it sticks between runs
        let actual_w = panel.response.rect.width().clamp(min_w, max_w);
        if (actual_w - self.detail_panel_width).abs() > (step * 0.05).max(0.5) {
            self.detail_panel_width = actual_w;
            self.mark_dirty(); // let your prefs autosave pick this up
        }

        if let Some(idx) = trigger_rating_request {
            self.request_rating_for(idx);
        }
    }
}
