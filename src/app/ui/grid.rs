// src/app/ui/grid.rs
use eframe::egui as eg;

pub const H_SPACING: f32 = 4.0;
pub const V_SPACING: f32 = 10.0;

fn draw_corner_badge(p: &eframe::egui::Painter, rect: eg::Rect, label: &str) {
    if label.is_empty() {
        return;
    }
    let pad = 6.0;
    let size = eg::vec2(48.0, 20.0);
    let r = eg::Rect::from_min_max(
        eg::pos2(rect.right() - pad - size.x, rect.top() + pad),
        eg::pos2(rect.right() - pad, rect.top() + pad + size.y),
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
    pub(crate) fn ui_render_grouped_grid(&mut self, ui: &mut eg::Ui, ctx: &eg::Context) {
        self.handle_keyboard_navigation(ctx);

        let groups = self.build_grouped_indices();
        self.sync_selection_with_groups(&groups);
        self.grid_rows.clear();

        let card_w: f32 = self.poster_width_ui;
        let text_h: f32 = 56.0;
        let card_h: f32 = card_w.mul_add(1.5, text_h);

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

                    let used =
                        (cols as f32).mul_add(card_w, (cols.saturating_sub(1)) as f32 * H_SPACING);
                    let left_pad = ((avail - used) * 0.5).max(0.0);
                    if left_pad > 0.0 {
                        ui.add_space(left_pad);
                    }

                    let mut row_buffer: Vec<usize> = Vec::new();
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = eg::vec2(H_SPACING, V_SPACING);

                        for (col, &idx) in idxs.iter().enumerate() {
                            if col > 0 && col % cols == 0 {
                                ui.end_row();
                                if !row_buffer.is_empty() {
                                    self.grid_rows.push(std::mem::take(&mut row_buffer));
                                }
                            }

                            if row_buffer.is_empty() {
                                row_buffer.reserve(cols);
                            }
                            row_buffer.push(idx);

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
                                        eg::pos2(
                                            rect.min.x + card_w,
                                            card_w.mul_add(1.5, rect.min.y),
                                        ),
                                    );
                                    let text_rect = eg::Rect::from_min_max(
                                        eg::pos2(rect.min.x, poster_rect.max.y),
                                        rect.max,
                                    );

                                    if self.scroll_to_idx == Some(idx) {
                                        ui.scroll_to_rect(rect, Some(eg::Align::Center));
                                        self.scroll_to_idx = None;
                                    }

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

                                        if row.scheduled {
                                            let pad = 6.0;
                                            let size = eg::vec2(56.0, 22.0);
                                            let rec_rect = eg::Rect::from_min_size(
                                                eg::pos2(
                                                    poster_rect.left() + pad,
                                                    poster_rect.top() + pad,
                                                ),
                                                size,
                                            );
                                            let fill = eg::Color32::from_rgb(200, 40, 40);
                                            let stroke = eg::Color32::from_rgb(140, 16, 16);
                                            ui.painter().rect_filled(
                                                rec_rect,
                                                eg::Rounding::same(6.0),
                                                fill,
                                            );
                                            ui.painter().rect_stroke(
                                                rec_rect,
                                                eg::Rounding::same(6.0),
                                                eg::Stroke::new(1.0, stroke),
                                            );
                                            ui.painter().text(
                                                rec_rect.center(),
                                                eg::Align2::CENTER_CENTER,
                                                "REC",
                                                eg::FontId::monospace(13.0),
                                                eg::Color32::WHITE,
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
                                        let title_line = row.year.map_or_else(
                                            || row.title.clone(),
                                            |y| format!("{} ({})", row.title, y),
                                        );
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
                                            let highlight = poster_rect.expand(2.0);
                                            ui.painter().rect_stroke(
                                                highlight,
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
                    if !row_buffer.is_empty() {
                        self.grid_rows.push(std::mem::take(&mut row_buffer));
                    }
                }
            });
    }
}
