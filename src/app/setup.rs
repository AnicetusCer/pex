use super::PexApp;
use crate::config;
use eframe::egui as eg;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

type Discovery = Result<(Vec<PathBuf>, Option<PathBuf>), String>;
#[derive(Default)]
pub(super) struct ConnectionSetup {
    pub open: bool,
    folder: String,
    guides: Vec<PathBuf>,
    selected: usize,
    library: Option<PathBuf>,
    guide_only: bool,
    discovery: Option<Receiver<(String, Discovery)>>,
    checks: Option<Receiver<(Vec<String>, Vec<String>)>>,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn frames_until(app: &mut PexApp, ctx: &eg::Context, condition: impl Fn(&PexApp) -> bool) {
        let start = Instant::now();
        loop {
            let _ = ctx.run(eg::RawInput::default(), |ctx| app.update_ui(ctx));
            if condition(app) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "Startup timed out: {} {:?}",
                app.loading_message,
                app.setup_errors
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn portable_first_launch_scenarios() {
        // Every portable launch has isolated environment, OnceLocks, home and cwd.
        for scenario in [
            "missing",
            "bundled",
            "placeholder",
            "unreadable",
            "malformed",
            "guide_only",
            "busy_backup",
            "busy_no_backup",
            "legacy",
            "local_only",
        ] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "app::setup::tests::portable_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("PEX_FIXTURE_SCENARIO", scenario)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{scenario}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    #[ignore = "launched in isolated child processes by portable_first_launch_scenarios"]
    fn portable_fixture() {
        let scenario = std::env::var("PEX_FIXTURE_SCENARIO").unwrap();
        let root = tempfile::tempdir().unwrap();
        let portable = root.path().join("portable");
        let plex = root.path().join("Plex Databases");
        fs::create_dir_all(&portable).unwrap();
        fs::create_dir_all(&plex).unwrap();
        std::env::set_current_dir(&portable).unwrap();
        std::env::set_var("PEX_BASE_DIR", &portable);
        std::env::set_var("HOME", &portable);
        std::env::set_var("USERPROFILE", &portable);
        std::env::remove_var("PEX_CONFIG");
        std::env::set_var("PEX_DISABLE_PREFETCH", "1");
        let guide = plex.join("tv.plex.providers.epg.cloud-fixture.db");
        let library = plex.join("com.plexapp.plugins.library.db");
        super::super::test_fixtures::database(&guide);
        super::super::test_fixtures::database(&library);
        if scenario == "guide_only" {
            fs::create_dir_all(portable.join("db")).unwrap();
            fs::copy(&library, config::local_library_db_path()).unwrap();
            fs::create_dir_all(portable.join(".pex_cache")).unwrap();
            fs::write(
                portable.join(".pex_cache/owned_all_v2.txt"),
                "fixture movie|2020\n",
            )
            .unwrap();
        }
        let legacy = portable.join("config.json");
        match scenario.as_str() {
            "bundled" => fs::write(&legacy, include_str!("../../make_portable/config.json")).unwrap(),
            "placeholder" => fs::write(&legacy, r#"{"plex_epg_db_source":"REPLACE_ME_WITH_PATH/tv.plex.providers.epg.cloud-example.db","plex_library_db_source":"REPLACE_ME_WITH_PATH/com.plexapp.plugins.library.db"}"#).unwrap(),
            "unreadable" => fs::write(&legacy, r#"{"plex_epg_db_source":"missing.db"}"#).unwrap(),
            "malformed" => fs::write(&legacy, "{ invalid json").unwrap(),
            "legacy" => fs::write(&legacy, serde_json::to_vec(&serde_json::json!({"plex_db_source":guide,"plex_library_db_source":library,"custom_setting":42,"tmdb_api_key":"fixture-key"})).unwrap()).unwrap(),
            "local_only" => {
                fs::create_dir_all(portable.join("db")).unwrap();
                fs::copy(&guide, config::local_db_path()).unwrap();
            }
            _ => {}
        }
        if scenario == "busy_backup" || scenario == "busy_no_backup" {
            // A nonempty rollback journal is a deterministic busy-source fixture.
            fs::write(
                format!("{}-journal", library.display()),
                b"active fixture journal",
            )
            .unwrap();
            if scenario == "busy_backup" {
                super::super::test_fixtures::database(
                    &plex.join("com.plexapp.plugins.library.db-2026-01-02"),
                );
            }
        }
        let ctx = eg::Context::default();
        let mut app = PexApp::default();
        if scenario != "legacy" && scenario != "local_only" {
            frames_until(&mut app, &ctx, |app| app.setup_checked);
            assert!(!app.did_init);
            assert!(!app.setup_errors.is_empty());
            assert!(!config::local_db_path().exists());
            if scenario == "malformed" {
                assert!(app.setup_errors[0].contains("Cannot load"));
                assert!(config::save_connection(&guide, None, true).is_err());
                assert_eq!(fs::read_to_string(&legacy).unwrap(), "{ invalid json");
                return;
            }
            let (guides, found_library) = config::discover_databases(&plex).unwrap();
            app.connection.guides = guides;
            app.connection.library = found_library;
            app.connection.guide_only = !scenario.starts_with("busy_");
            app.save_selected_connection(&ctx);
        }
        frames_until(&mut app, &ctx, |app| {
            app.did_init
                && app.rows.len() == 1
                && !app.owned_scan_in_progress
                && app.prefetch_started
        });
        assert_eq!(app.rows[0].title, "Fixture Movie");
        assert!(app.should_show_grid());
        match scenario.as_str() {
            "busy_backup" => {
                assert!(app.rows[0].owned);
                assert!(app.owned_notice.as_ref().unwrap().contains("2026-01-02"));
                assert_eq!(config::load_config().plex_library_db_source, Some(library));
                assert!(!portable.join("db/plex_library.db.last_sync").exists());
            }
            "busy_no_backup" => {
                assert!(!app.rows[0].owned);
                assert!(app
                    .owned_notice
                    .as_ref()
                    .unwrap()
                    .contains("Retry owned scan"));
                assert!(!config::local_library_db_path().exists());
                assert!(!app.owned_scan_in_progress);
            }
            "legacy" => {
                assert!(app.rows[0].owned);
                config::save_connection(&guide, Some(&library), false).unwrap();
                assert_eq!(
                    config::discover_config_path(),
                    Some(portable.join("pex-config.json"))
                );
                let value: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(portable.join("pex-config.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(value["custom_setting"], 42);
                assert_eq!(value["tmdb_api_key"], "fixture-key");
                assert!(value.get("plex_db_source").is_none());
                assert!(fs::read_to_string(&legacy)
                    .unwrap()
                    .contains("plex_db_source"));
                // Re-extracting a bundle's empty legacy config must not lose setup.
                fs::write(&legacy, include_str!("../../make_portable/config.json")).unwrap();
                assert_eq!(
                    config::load_config().plex_epg_db_source,
                    Some(guide.clone())
                );
                let explicit = portable.join("custom-settings.json");
                std::env::set_var("PEX_CONFIG", &explicit);
                config::save_connection(&guide, Some(&library), false).unwrap();
                assert_eq!(config::discover_config_path(), Some(explicit));
                #[cfg(windows)]
                {
                    let unc = PathBuf::from(
                        r"\\fixture-nas\Plex share\Databases\tv.plex.providers.epg.cloud-test.db",
                    );
                    config::save_connection(&unc, None, true).unwrap();
                    assert_eq!(config::load_config().plex_epg_db_source, Some(unc));
                }
            }
            _ => assert!(!app.rows[0].owned),
        }
    }
}

impl PexApp {
    pub(super) fn run_setup_checks(&mut self) {
        if self.connection.checks.is_none() {
            let (tx, rx) = mpsc::channel();
            self.connection.checks = Some(rx);
            std::thread::spawn(move || {
                let cfg = config::load_config();
                let mut errors = Vec::new();
                let mut warnings = Vec::new();
                if let Some(problem) = config::config_problem() {
                    errors.push(problem);
                }
                let local_ok = config::readable_database(&config::local_db_path()).is_ok();
                match cfg.plex_epg_db_source.as_deref() {
                    Some(path) => if let Err(e) = config::readable_database(path) {
                        if local_ok { warnings.push(format!("{e} Using the local guide.")); }
                        else { errors.push(e); }
                    },
                    None if !local_ok => errors.push("Welcome to PEX. Choose your Plex Databases folder to load the programme guide. No config or SQLite installation is needed.".into()),
                    _ => {}
                }
                if !cfg.guide_only {
                    if let Some(path) = cfg.plex_library_db_source.as_deref() {
                        if let Err(e) = config::readable_database(path) {
                            warnings.push(e);
                        }
                    }
                }
                let _ = tx.send((errors, warnings));
            });
        }
        if let Some(Ok((errors, warnings))) =
            self.connection.checks.as_ref().map(Receiver::try_recv)
        {
            self.setup_errors = errors;
            self.setup_warnings = warnings;
            self.setup_checked = true;
            self.connection.checks = None;
        }
    }

    pub(super) fn render_setup_gate(&mut self, ctx: &eg::Context) {
        if let Some(Ok((folder, result))) =
            self.connection.discovery.as_ref().map(Receiver::try_recv)
        {
            self.connection.discovery = None;
            self.connection.folder = folder;
            match result {
                Ok((guides, library)) => {
                    self.connection.message = if guides.is_empty() {
                        "No guide databases found. Select the Databases folder containing tv.plex.providers.epg.cloud*.db. Plex DVR/EPG must be enabled.".into()
                    } else {
                        "Choose the guide below, then Save and load guide.".into()
                    };
                    self.connection.guides = guides;
                    self.connection.selected = 0;
                    self.connection.guide_only = library.is_none();
                    self.connection.library = library;
                }
                Err(e) => self.connection.message = e,
            }
        }
        eg::CentralPanel::default().show(ctx, |ui| { eg::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Connect to Plex");
            ui.label("Select Plex Media Server's Plug-in Support / Databases folder. PEX copies data locally and leaves Plex's files unchanged.");
            for e in &self.setup_errors { ui.colored_label(eg::Color32::LIGHT_RED, e); }
            for w in &self.setup_warnings { ui.label(w); }
            if !self.setup_checked && !self.connection.open { ui.spinner(); ui.label("Checking saved configuration..."); }
            ui.add_space(12.0);
            ui.label(r"Databases folder (local or network share, e.g. \\NAS\Plex\Databases):");
            if ui.text_edit_singleline(&mut self.connection.folder).changed() {
                self.connection.guides.clear();
                self.connection.library = None;
            }
            let busy = self.connection.discovery.is_some();
            ui.horizontal(|ui| {
                let browse = ui.add_enabled(!busy, eg::Button::new("Browse...")).clicked();
                let find = ui.add_enabled(!busy && !self.connection.folder.trim().is_empty(), eg::Button::new("Find databases")).clicked();
                if browse || find {
                    let folder = self.connection.folder.trim().to_string();
                    let (tx, rx) = mpsc::channel();
                    self.connection.discovery = Some(rx);
                    std::thread::spawn(move || {
                        let chosen = if browse { rfd::FileDialog::new().pick_folder() } else { Some(PathBuf::from(&folder)) };
                        if let Some(path) = chosen {
                            let result = config::discover_databases(&path);
                            let _ = tx.send((path.to_string_lossy().into_owned(), result));
                        } else { let _ = tx.send((folder, Err("Folder selection cancelled. You can also paste a network path.".into()))); }
                    });
                }
                if busy { ui.spinner(); ui.label("Looking for Plex databases..."); }
            });
            ui.label(&self.connection.message);
            for (i, guide) in self.connection.guides.iter().enumerate() {
                ui.radio_value(&mut self.connection.selected, i, guide.file_name().unwrap_or_default().to_string_lossy());
            }
            if let Some(library) = &self.connection.library { ui.label(format!("Library: {}", library.display())); }
            ui.checkbox(&mut self.connection.guide_only, "Guide only (skip owned library and recording badges)");
            if self.connection.library.is_none() && !self.connection.guides.is_empty() {
                ui.label("No live library found. Keep Guide only checked, or select a folder containing com.plexapp.plugins.library.db.");
            }
            if self.owned_scan_in_progress || self.in_flight() > 0 || self.prep_rx.is_some() {
                ui.label("A scan or artwork download is running. You can save a new connection when it finishes.");
            }
            let can_save = !busy && !self.connection.guides.is_empty() && (self.connection.guide_only || self.connection.library.is_some()) && !self.owned_scan_in_progress && self.in_flight() == 0 && self.prep_rx.is_none();
            if ui.add_enabled(can_save, eg::Button::new("Save and load guide")).clicked() {
                self.save_selected_connection(ctx);
            }
            if ui.add_enabled(!self.owned_scan_in_progress && self.prep_rx.is_none() && self.in_flight() == 0,
                eg::Button::new("Retry saved configuration")).clicked() {
                self.setup_checked = false;
                self.connection.checks = None;
                self.run_setup_checks();
                self.connection.open = false;
                if self.did_init { self.restart_poster_pipeline(ctx); self.start_owned_scan(true); }
            }
            if self.did_init && ui.button("Back to guide").clicked() { self.connection.open = false; }
            ui.label("No TMDb key is required. Choose a writable portable folder; your settings and local copies stay beside PEX.");
        }); });
    }
    fn save_selected_connection(&mut self, ctx: &eg::Context) {
        let guide = &self.connection.guides[self.connection.selected];
        match config::save_connection(
            guide,
            self.connection.library.as_deref(),
            self.connection.guide_only,
        ) {
            Ok(path) => {
                self.connection.message = format!("Saved {}", path.display());
                self.connection.open = false;
                self.setup_errors.clear();
                self.setup_checked = false;
                self.connection.checks = None;
                self.setup_warnings.clear();
                let _ = std::fs::remove_file(format!(
                    "{}.last_sync",
                    config::local_db_path().display()
                ));
                if self.did_init {
                    // A different server must not inherit the old server's owned flags.
                    self.owned_keys = None;
                    self.owned_hd_keys = None;
                    self.owned_guids = None;
                    self.scheduled_index = None;
                    self.restart_poster_pipeline(ctx);
                    self.start_owned_scan(true);
                }
            }
            Err(e) => self.connection.message = e,
        }
    }
}
