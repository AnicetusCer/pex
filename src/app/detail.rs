// src/app/detail.rs
use eframe::egui as eg;

impl crate::app::PexApp {
    pub(crate) fn ui_render_detail_panel(&mut self, ctx: &eg::Context) {
        // Read screen width and snap panel width to poster "column steps"
        let screen_w: f32 = ctx.input(|i| i.screen_rect().width());
        let step: f32 = (self.poster_width_ui + crate::app::ui::grid::H_SPACING).max(1.0); // poster + gutter
        let max_w: f32 = (screen_w * 0.45).clamp(360.0, 520.0);
        let snapped_max: f32 = ((max_w / step).floor() * step).max(260.0);

        // Start from the last saved width, but snap it into a valid step and clamp to min..max
        let snapped_default: f32 =
            ((self.detail_panel_width / step).round() * step).clamp(260.0, snapped_max);

        let panel = eg::SidePanel::right("detail_panel")
            .resizable(true)
            .default_width(snapped_default)
            .min_width(260.0)
            .max_width(snapped_max)
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
                ui.heading(title);

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

                // Genres
                if !row.genres.is_empty() {
                    ui.label(eg::RichText::new("Genres").strong());
                    ui.label(row.genres.join(", "));
                    ui.add_space(6.0);
                } else {
                    ui.label(eg::RichText::new("Genres").weak());
                    ui.label("—");
                    ui.add_space(6.0);
                }

                // Future: Description + IMDb review hook
                ui.separator();
                ui.label(eg::RichText::new("Review").strong().weak());
                ui.label("IMDb review integration (planned).");
            });

        // Persist the (snapped) width so it sticks between runs
        let actual_w = panel.response.rect.width();
        let snapped_new = ((actual_w / step).round() * step).clamp(260.0, snapped_max);
        if (snapped_new - self.detail_panel_width).abs() > 0.5 {
            self.detail_panel_width = snapped_new;
            self.mark_dirty(); // let your prefs autosave pick this up
        }
    }
}
