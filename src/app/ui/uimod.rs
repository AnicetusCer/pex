// src/app/ui/mod.rs
pub mod grid;
pub mod topbar;

use eframe::egui as eg;

impl crate::app::PexApp {
    // Keep splash here; it's tiny and used early.
    pub(crate) fn ui_render_splash(&self, ui: &mut eg::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(28.0);
            ui.heading("Initialising Plex EPG Explorer");
            ui.add(eg::Spinner::new().size(18.0));
            ui.separator();
            if !self.loading_message.is_empty() {
                ui.label(&self.loading_message);
            }
            ui.add_space(12.0);
            ui.label("Startup stages:");
            ui.label("1) Setup & cache validation (checks config, DB paths, tools).");
            ui.label("2) Prepare guide data (scans Plex EPG for posters).");
            ui.label("3) Scan owned library (enables Owned/HD markers).");
            ui.label("4) Prefetch artwork (caches posters for smooth browsing).");
            ui.add_space(8.0);
            ui.monospace(format!(
                "Cache: {}", 
                crate::app::cache::cache_dir().display()
            ));
            ui.label(
                "Tip: first runs may take a while on large libraries; later launches reuse cached data.",
            );
        });
    }
}
