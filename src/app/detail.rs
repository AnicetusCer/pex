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

                // Poster preview (uses small texture if available)
                ui.add_space(4.0);
                let avail_w = ui.available_width().clamp(120.0, 520.0);
                let poster_size = eg::vec2(avail_w, avail_w * 1.5);

                if let Some(tex) = &row.tex {
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
                let title = match row.year {
                    Some(y) => format!("{} ({})", row.title, y),
                    None => row.title.clone(),
                };
                ui.horizontal(|ui| {
                    ui.heading(&title);
                    if ui
                        .small_button("📋")
                        .on_hover_text("Copy title to clipboard")
                        .clicked()
                    {
                        ctx.output_mut(|o| o.copied_text = row.title.clone());
                    }
                });

                // Channel + time line (humanized)
                if row.channel.is_some() || row.airing.is_some() {
                    let ch = row
                        .channel
                        .as_deref()
                        .map(crate::app::utils::humanize_channel)
                        .unwrap_or_else(|| "—".into());
                    let tm = row
                        .airing
                        .map(crate::app::utils::hhmm_utc)
                        .unwrap_or_else(|| "—".into());
                    ui.label(eg::RichText::new(format!("{ch}  •  {tm} UTC")).weak());
                }

                if row.critic_rating.is_some() || row.audience_rating.is_some() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if let Some(r) = row.critic_rating {
                            ui.label(
                                eg::RichText::new(format!("Critics: {r:.1}/10"))
                                    .color(eg::Color32::from_rgb(255, 208, 121)),
                            );
                        }
                        if let Some(r) = row.audience_rating {
                            ui.label(
                                eg::RichText::new(format!("Audience: {r:.1}/10"))
                                    .color(eg::Color32::from_rgb(160, 220, 160)),
                            );
                        }
                    });
                }

                ui.add_space(6.0);
                let rating_state = self.rating_state_for_key(&row.key);
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
                                eg::RichText::new("Set omdb_api_key in config.json to enable ratings.")
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
                    let tags_joined = if !row.genres.is_empty() {
                        Some(row.genres.join("|"))
                    } else {
                        None
                    };
                    let broadcast_hd = crate::app::utils::infer_broadcast_hd(
                        tags_joined.as_deref(),
                        row.channel.as_deref(),
                    );

                    let owned_key = Self::make_owned_key(&row.title, row.year);
                    let owned_is_hd = self
                        .owned_hd_keys
                        .as_ref()
                        .map_or(false, |set| set.contains(&owned_key));

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
                        if row.owned {
                            let (txt, col) = if owned_is_hd {
                                ("Owned HD", eg::Color32::from_rgb(130, 200, 130))
                            } else {
                                ("Owned SD", eg::Color32::from_gray(200))
                            };
                            ui.add(eg::Label::new(eg::RichText::new(txt).color(col)));
                        }
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Description
                ui.label(eg::RichText::new("Description").strong());
                if let Some(summary) = row.summary.as_deref() {
                    ui.add(eg::Label::new(eg::RichText::new(summary)).wrap());
                } else {
                    ui.label(eg::RichText::new("No description available.").italics().weak());
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // Genres
                if !row.genres.is_empty() {
                    ui.label(eg::RichText::new("Genres").strong());
                    ui.add(eg::Label::new(row.genres.join(", ")).wrap());
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
