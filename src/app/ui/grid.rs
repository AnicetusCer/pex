// src/app/ui/grid.rs
use eframe::egui as eg;

pub const H_SPACING: f32 = 4.0;
pub const V_SPACING: f32 = 10.0;

fn draw_corner_badge(p: &eframe::egui::Painter, rect: eg::Rect, label: &str) {
    if label.is_empty() {
        return;
    }
    let pad = 6.0;
    let r = eg::Rect::from_min_size(
        eg::pos2(rect.left() + pad, rect.top() + pad),
        eg::vec2(48.0, 20.0),
    );

    let visuals = p.ctx().style().visuals.clone();
    let bg = visuals.extreme_bg_color.gamma_multiply(0.92);
    let fg = visuals.strong_text_color();

    p.rect_filled(r, eg::Rounding::same(6.0), bg);
    p.rect_stroke(r, eg::Rounding::same(6.0), eg::Stroke::new(1.0, fg));
    p.text(
        r.center(),
        eg::Align2::CENTER_CENTER,
        label,
        eg::FontId::monospace(12.0),
        fg,
    );
}

impl crate::app::PexApp {
    // src/app/ui/grid.rs
    pub(crate) fn ui_render_grouped_grid(&mut self, ui: &mut eg::Ui, ctx: &eg::Context) {
        let groups = self.build_grouped_indices();

        let card_w: f32 = self.poster_width_ui;
        let text_h: f32 = 56.0;
        let card_h: f32 = card_w * 1.5 + text_h;

        let mut uploads_left = super::super::MAX_UPLOADS_PER_FRAME;

        eg::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for (bucket, idxs) in groups {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.heading(crate::app::utils::format_day_label(bucket));
                    ui.add_space(4.0);

                    // Columns + centering (use local module constants directly)
                    let avail = ui.available_width();
                    let cols = ((avail + H_SPACING) / (card_w + H_SPACING))
                        .floor()
                        .max(1.0) as usize;

                    let used = cols as f32 * card_w + (cols.saturating_sub(1)) as f32 * H_SPACING;
                    let left_pad = ((avail - used) * 0.5).max(0.0);
                    if left_pad > 0.0 {
                        ui.add_space(left_pad);
                    }

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = eg::vec2(H_SPACING, V_SPACING);

                        for (col, &idx) in idxs.iter().enumerate() {
                            if col > 0 && col.is_multiple_of(cols) {
                                ui.end_row();
                            }

                            ui.allocate_ui_with_layout(
                                eg::vec2(card_w, card_h),
                                eg::Layout::top_down(eg::Align::Min),
                                |ui| {
                                    ui.set_min_size(eg::vec2(card_w, card_h));
                                    let rect = ui.max_rect();

                                    // selection
                                    let id = eg::Id::new(("card_sel", idx));
                                    if ui.interact(rect, id, eg::Sense::click()).clicked() {
                                        self.selected_idx = Some(idx);
                                    }

                                    // opportunistic upload
                                    if uploads_left > 0 && self.try_lazy_upload_row(ctx, idx) {
                                        uploads_left -= 1;
                                    }

                                    // rects
                                    let poster_rect = eg::Rect::from_min_max(
                                        rect.min,
                                        eg::pos2(rect.min.x + card_w, rect.min.y + card_w * 1.5),
                                    );
                                    let text_rect = eg::Rect::from_min_max(
                                        eg::pos2(rect.min.x, poster_rect.max.y),
                                        rect.max,
                                    );

                                    if let Some(row) = self.rows.get(idx) {
                                        // Poster
                                        if let Some(tex) = &row.tex {
                                            ui.painter().image(
                                                tex.id(),
                                                poster_rect,
                                                eg::Rect::from_min_max(
                                                    eg::pos2(0.0, 0.0),
                                                    eg::pos2(1.0, 1.0),
                                                ),
                                                eg::Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter().rect_filled(
                                                poster_rect,
                                                6.0,
                                                eg::Color32::from_gray(40),
                                            );
                                        }

                                        // --- Compute statuses (needed for badges & dimming) ---
                                        let broadcast_hd = Self::row_broadcast_hd(row);
                                        let owned_is_hd = self.row_owned_is_hd(row);
                                        let better_hd_available =
                                            row.owned && !owned_is_hd && broadcast_hd;

                                        // Corner badge: show only for HD airings; SD gets no symbol
                                        if better_hd_available {
                                            draw_corner_badge(ui.painter(), poster_rect, "HD ↑");
                                        } else if broadcast_hd {
                                            draw_corner_badge(ui.painter(), poster_rect, "HD");
                                        }

                                        // Dim overlay: do NOT dim if there's an HD upgrade airing
                                        let should_dim =
                                            row.owned && self.dim_owned && !better_hd_available;
                                        if should_dim {
                                            let a = (self.dim_strength_ui.clamp(0.10, 0.90) * 255.0)
                                                as u8;
                                            let overlay_rect = poster_rect.expand(0.5);
                                            ui.painter().rect_filled(
                                                overlay_rect,
                                                eg::Rounding::ZERO,
                                                eg::Color32::from_black_alpha(a),
                                            );
                                        }

                                        // Label
                                        let title_line = match row.year {
                                            Some(y) => format!("{} ({})", row.title, y),
                                            None => row.title.clone(),
                                        };
                                        let ch = row
                                            .channel
                                            .as_deref()
                                            .map(crate::app::utils::humanize_channel)
                                            .unwrap_or_else(|| "—".into());
                                        let line2 = if broadcast_hd {
                                            format!("{ch} • HD")
                                        } else {
                                            ch
                                        };
                                        let tm = row
                                            .airing
                                            .map(crate::app::utils::hhmm_utc)
                                            .unwrap_or_else(|| "—".into());
                                        let line3 = tm + " UTC";

                                        let label_text = format!(
                                            "{title}\n{line2}\n{line3}",
                                            title = title_line
                                        );

                                        ui.allocate_ui_at_rect(text_rect, |ui| {
                                            ui.add(
                                                eg::Label::new(
                                                    eg::RichText::new(label_text).size(14.0),
                                                )
                                                .wrap(),
                                            );
                                        });

                                        // Selection stroke
                                        if self.selected_idx == Some(idx) {
                                            ui.painter().rect_stroke(
                                                rect.shrink(1.0),
                                                6.0,
                                                eg::Stroke::new(2.0, eg::Color32::YELLOW),
                                            );
                                        }
                                    }
                                },
                            );
                        }

                        ui.end_row();
                    });
                }
            });
    }
}
