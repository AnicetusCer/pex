use super::PexApp;
use eframe::egui::{self as eg, TextureHandle};

use crate::app::cache::{
    cache_dir, download_and_store, find_any_by_key, load_rgba_raw_or_image, url_to_cache_key,
};

// ===== Helpers =====

fn film_thumb_key(film: &crate::app::Film) -> Option<String> {
    film.user_thumb_url.clone()
}

fn load_texture_from_path(
    ctx: &eg::Context,
    path_str: &str,
    cache_key_for_name: &str,
) -> Result<TextureHandle, String> {
    let (w, h, bytes) = load_rgba_raw_or_image(path_str)?;
    let img = eg::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &bytes);
    Ok(ctx.load_texture(cache_key_for_name.to_string(), img, eg::TextureOptions::LINEAR))
}

/// Process up to `budget` posters this frame. Returns (loaded_now, have_textures, need_textures).
fn prefetch_step(app: &mut PexApp, ctx: &eg::Context, budget: usize) -> (usize, usize, usize) {
    // Count how many films have poster URLs
    let mut need = 0usize;
    let mut have = 0usize;
    for f in &app.films {
        if let Some(url) = film_thumb_key(f) {
            need += 1;
            if app.images.contains_key(&url) {
                have += 1;
            }
        }
    }

    if need == 0 {
        app.loading_progress = 1.0;
        app.loading_message = "No posters to preload".to_string();
        return (0, 0, 0);
    }

    let mut loaded_now = 0usize;

    // Walk films and handle a few missing posters per frame
    for (idx, f) in app.films.iter().enumerate() {
        if loaded_now >= budget {
            break;
        }
        let Some(url) = film_thumb_key(f) else { continue; };

        // Already loaded into GPU?
        if app.images.contains_key(&url) {
            continue;
        }

        let key = url_to_cache_key(&url);

        // Ensure we have a cached file
        let path = if let Some(found) = find_any_by_key(&key) {
            found
        } else {
            match download_and_store(&url, &key) {
                Ok(p) => p,
                Err(e) => {
                    app.loading_message = format!("Fetch failed: {}", e);
                    continue;
                }
            }
        };

        // Turn file into a texture
        match load_texture_from_path(ctx, &path.to_string_lossy(), &key) {
            Ok(tex) => {
                app.images.insert(url.clone(), tex);
                loaded_now += 1;
                have += 1;
                app.loading_message = format!("Loaded poster {}/{} (film #{})", have, need, idx);
            }
            Err(e) => {
                app.loading_message = format!("Decode failed: {}", e);
            }
        }
    }

    app.loading_progress = (have as f32 / need as f32).clamp(0.0, 1.0);
    (loaded_now, have, need)
}

fn poster_title_line(title: &str, year: Option<i32>) -> String {
    if let Some(y) = year { format!("{title} ({y})") } else { title.to_string() }
}

// ===== UI =====

impl PexApp {
    pub fn render(&mut self, ctx: &eg::Context) {
        eg::CentralPanel::default().show(ctx, |ui| {
            // Splash + prefetch
            if self.loading_progress < 1.0 {
                let (_loaded_now, have, need) = prefetch_step(self, ctx, 2);

                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.heading("Plex EPG Explorer — UI v2");
                    ui.label("Preparing posters…");
                    ui.label(&self.loading_message);

                    let pct = if need == 0 { 1.0 } else { have as f32 / need as f32 };
                    ui.add(eg::ProgressBar::new(pct).show_percentage());
                    ui.label(format!("{have}/{need}"));
                    ui.separator();
                    ui.monospace(format!("Cache: {}", self.dbg_cache_dir));
                });

                return; // keep loading until done
            }

            // Grid
            let available = ui.available_width() - 8.0;
            let card_w: f32 = 140.0;
            let card_h: f32 = 140.0 * 1.5 + 36.0;
            let cols = (available / card_w.max(1.0)).floor().max(1.0) as usize;

            let indices: Vec<usize> = (0..self.films.len()).collect();

            eg::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    eg::Grid::new("grid")
                        .num_columns(cols)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (i, idx) in indices.iter().enumerate() {
                                let f = &self.films[*idx];
                                let url = film_thumb_key(f).unwrap_or_default();

                                // Card rect
                                let (rect, _resp) = ui.allocate_exact_size(
                                    eg::vec2(card_w, card_h),
                                    eg::Sense::click(),
                                );

                                // Poster & text rects
                                let poster_rect = eg::Rect::from_min_max(
                                    rect.min,
                                    eg::pos2(rect.min.x + card_w, rect.min.y + card_w * 1.5),
                                );
                                let text_rect = eg::Rect::from_min_max(
                                    eg::pos2(rect.min.x, poster_rect.max.y),
                                    rect.max,
                                );

                                // Poster
                                if !url.is_empty() {
                                    if let Some(tex) = self.images.get(&url) {
                                        ui.painter().image(
                                            tex.id(),
                                            poster_rect,
                                            eg::Rect::from_min_size(eg::pos2(0.0, 0.0), tex.size_vec2()),
                                            eg::Color32::WHITE,
                                        );
                                    } else {
                                        ui.painter().rect_filled(poster_rect, 6.0, eg::Color32::from_gray(40));
                                    }
                                } else {
                                    ui.painter().rect_filled(poster_rect, 6.0, eg::Color32::from_gray(40));
                                }

                                // Title line
                                let line = poster_title_line(&f.title, f.year);
                                ui.painter().text(
                                    text_rect.left_top(),
                                    eg::Align2::LEFT_TOP,
                                    line,
                                    eg::FontId::proportional(14.0),
                                    eg::Color32::WHITE,
                                );

                                // Selection
                                let resp = ui.interact(rect, ui.make_persistent_id(("card", i)), eg::Sense::click());
                                if resp.clicked() {
                                    self.selected_idx = Some(*idx);
                                }
                                if self.selected_idx == Some(*idx) {
                                    ui.painter().rect_stroke(
                                        rect.shrink(1.0),
                                        6.0,
                                        eg::Stroke::new(2.0, eg::Color32::YELLOW),
                                    );
                                }

                                if (i + 1) % cols == 0 { ui.end_row(); }
                            }
                            ui.end_row();
                        });
                });
        });
    }
}
