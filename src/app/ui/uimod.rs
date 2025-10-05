// src/app/ui/mod.rs
pub mod grid;
pub mod topbar;

use eframe::egui as eg;

impl crate::app::PexApp {
    // Keep splash here; it's tiny and used early.
    pub(crate) fn ui_render_splash(&mut self, ui: &mut eg::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("Poster preparation…");
            ui.add(eg::Spinner::new().size(16.0));
            ui.separator();
            if !self.loading_message.is_empty() {
                ui.label(&self.loading_message);
            }
            ui.monospace(format!(
                "Cache: {}",
                crate::app::cache::cache_dir().display()
            ));
        });
    }
}
