use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result<()> {
    // Initialize logging with RUST_LOG if provided (e.g., RUST_LOG=info)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Plex EPG Explorer",
        options,
        Box::new(|_cc| {
            // PexApp must be in crate::app and implement Default + eframe::App
            Ok(Box::new(pex::app::PexApp::default()))
        }),
    )
}
