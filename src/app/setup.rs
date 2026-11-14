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
    use_snapshots: bool,
    snapshot_folder: String,
    snapshot_picker: Option<Receiver<Option<PathBuf>>>,
    discovery: Option<Receiver<(String, Discovery)>>,
    checks: Option<Receiver<(Vec<String>, Vec<String>)>>,
    message: String,
}

impl ConnectionSetup {
    pub(super) fn checking(&self) -> bool {
        self.checks.is_some() || self.discovery.is_some() || self.snapshot_picker.is_some()
    }

    pub(super) fn from_config(cfg: config::AppConfig) -> Self {
        let guides: Vec<_> = cfg.plex_epg_db_source.into_iter().collect();
        let folder = guides
            .first()
            .and_then(|guide| guide.parent())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let snapshot_folder = cfg
            .library_snapshot_dir
            .as_ref()
            .filter(|p| Some(p.as_path()) != guides.first().and_then(|guide| guide.parent()))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            folder,
            guides,
            library: cfg.plex_library_db_source,
            guide_only: cfg.guide_only,
            use_snapshots: cfg.library_snapshot_dir.is_some(),
            snapshot_folder,
            ..Self::default()
        }
    }

    fn apply_discovery(&mut self, folder: String, guides: Vec<PathBuf>, library: Option<PathBuf>) {
        let previous_guide = self.guides.get(self.selected);
        let same_folder =
            previous_guide.and_then(|path| path.parent()) == Some(std::path::Path::new(&folder));
        self.selected = guides
            .iter()
            .position(|path| Some(path) == previous_guide)
            .unwrap_or(0);
        if same_folder {
            // Keep an explicitly configured library which may be outside the guide folder.
            self.library = self.library.take().or(library);
        } else {
            self.guide_only = library.is_none() && !self.use_snapshots;
            self.library = library;
            self.snapshot_folder.clear();
        }
        self.message = if guides.is_empty() {
            "No guide databases found. Select the Databases folder containing tv.plex.providers.epg.cloud*.db. Plex DVR/EPG must be enabled.".into()
        } else {
            "Choose the guide below, then Save and load guide.".into()
        };
        self.guides = guides;
        self.folder = folder;
    }

    fn selected_snapshot_dir(&self) -> Option<PathBuf> {
        if !self.use_snapshots {
            return None;
        }
        if self.snapshot_folder.trim().is_empty() {
            self.guides.get(self.selected)?.parent().map(PathBuf::from)
        } else {
            Some(PathBuf::from(self.snapshot_folder.trim()))
        }
    }
}

impl PexApp {
    pub(super) fn open_connection_setup(&mut self) {
        self.connection = ConnectionSetup::from_config(config::load_config());
        self.connection.open = true;
    }

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
                let local = config::local_db_path();
                let local_ok = config::readable_database(&local).is_ok();
                match cfg.plex_epg_db_source.as_deref() {
                    Some(path) => if let Err(e) = config::readable_database(path) {
                        if local_ok && super::prep::guide_mirror_matches(path, &local) {
                            warnings.push(format!("{e} Using the local guide."));
                        }
                        else { errors.push(e); }
                    },
                    None if !local_ok => errors.push("Welcome to PEX. Choose your Plex Databases folder to load the programme guide. No config or SQLite installation is needed.".into()),
                    _ => {}
                }
                if !cfg.guide_only && cfg.library_snapshot_dir.is_none() {
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
            match result {
                Ok((guides, library)) => self.connection.apply_discovery(folder, guides, library),
                Err(e) => self.connection.message = e,
            }
        }
        if let Some(Ok(picked)) = self
            .connection
            .snapshot_picker
            .as_ref()
            .map(Receiver::try_recv)
        {
            self.connection.snapshot_picker = None;
            if let Some(path) = picked {
                self.connection.snapshot_folder = path.to_string_lossy().into_owned();
            }
        }
        eg::CentralPanel::default().show(ctx, |ui| { eg::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Connect to Plex");
            ui.label("Select Plex Media Server's Plug-in Support / Databases folder. PEX copies data locally and leaves Plex's files unchanged.");
            for e in &self.setup_errors { ui.colored_label(eg::Color32::LIGHT_RED, e); }
            for w in &self.setup_warnings { ui.label(w); }
            if !self.setup_checked && !self.connection.open { ui.spinner(); ui.label("Checking saved configuration..."); }
            ui.add_space(12.0);
            ui.label("Databases folder (local path, mounted folder or network share)");
            if ui.text_edit_singleline(&mut self.connection.folder).changed() {
                self.connection.guides.clear();
                self.connection.library = None;
            }
            let busy = self.connection.discovery.is_some() || self.connection.snapshot_picker.is_some();
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
                if self.connection.discovery.is_some() { ui.spinner(); ui.label("Looking for Plex databases..."); }
            });
            ui.label(&self.connection.message);
            for (i, guide) in self.connection.guides.iter().enumerate() {
                ui.radio_value(&mut self.connection.selected, i, guide.file_name().unwrap_or_default().to_string_lossy());
            }
            ui.checkbox(&mut self.connection.guide_only, "Guide only (skip owned library and recording badges)");
            if self.connection.library.is_none() && self.connection.guide_only && !self.connection.guides.is_empty() {
                ui.label("To use library backups, uncheck Guide only and choose Completed snapshots.");
            }
            if !self.connection.guide_only && !self.connection.guides.is_empty() {
                ui.label("Owned library source");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.connection.use_snapshots, false, "Live library");
                    ui.radio_value(&mut self.connection.use_snapshots, true, "Completed snapshots");
                });
                if self.connection.use_snapshots {
                    ui.label("Snapshot folder");
                    ui.horizontal(|ui| {
                        ui.add(eg::TextEdit::singleline(&mut self.connection.snapshot_folder)
                            .hint_text("Use the selected guide’s Databases folder"));
                        if ui.add_enabled(!busy, eg::Button::new("Browse snapshot folder...")).clicked() {
                            let (tx, rx) = mpsc::channel();
                            self.connection.snapshot_picker = Some(rx);
                            std::thread::spawn(move || { let _ = tx.send(rfd::FileDialog::new().pick_folder()); });
                        }
                    });
                    ui.label("Refresh uses the newest completed snapshot in this folder. Badges reflect its date.");
                } else if let Some(library) = &self.connection.library {
                    ui.label(format!("Library: {}", library.display()));
                } else {
                    ui.label("No live library found. Choose Completed snapshots or Guide only, or select a folder containing com.plexapp.plugins.library.db.");
                }
            }
            if self.owned_scan_in_progress || self.in_flight() > 0 || self.prep_rx.is_some() {
                ui.label("A scan or artwork download is running. You can save a new connection when it finishes.");
            }
            let can_save = !busy && !self.connection.guides.is_empty() && (self.connection.guide_only || self.connection.use_snapshots || self.connection.library.is_some()) && !self.owned_scan_in_progress && self.in_flight() == 0 && self.prep_rx.is_none();
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
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("Set up ratings…").clicked() {
                    self.open_rating_key_editor(None);
                }
                ui.label(self.rating_key_feedback.unwrap_or("Optional TMDb film ratings"));
            });
        }); });
    }
    fn save_selected_connection(&mut self, ctx: &eg::Context) {
        let guide = &self.connection.guides[self.connection.selected];
        let reload_guide = config::load_config().plex_epg_db_source.as_deref()
            != Some(guide.as_path())
            || !self.setup_errors.is_empty();
        match config::save_connection_with_snapshots(
            guide,
            self.connection.library.as_deref(),
            self.connection.selected_snapshot_dir().as_deref(),
            self.connection.guide_only,
        ) {
            Ok(path) => {
                self.advanced_paths = Default::default();
                self.connection.message = format!("Saved {}", path.display());
                self.connection.open = false;
                self.setup_errors.clear();
                self.setup_checked = false;
                self.connection.checks = None;
                self.setup_warnings.clear();
                if reload_guide {
                    let _ = std::fs::remove_file(format!(
                        "{}.last_sync",
                        config::local_db_path().display()
                    ));
                }
                if self.did_init {
                    if reload_guide {
                        self.restart_poster_pipeline(ctx);
                    }
                    self.start_owned_scan(true);
                }
            }
            Err(e) => self.connection.message = e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    #[test]
    fn owned_badges_follow_the_connection_across_restarts() {
        let root = tempfile::tempdir().unwrap();
        for phase in [
            "snapshot",
            "offline-live",
            "manual",
            "guide-only",
            "live",
            "failed-live",
        ] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "app::setup::tests::owned_source_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .current_dir(root.path())
                .env("PEX_OWNED_SOURCE_FIXTURE", phase)
                .env("PEX_BASE_DIR", root.path())
                .env("HOME", root.path())
                .env("USERPROFILE", root.path())
                .env("APPDATA", root.path().join("roaming"))
                .env("LOCALAPPDATA", root.path().join("local"))
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_CACHE_HOME", root.path().join("cache"))
                .env_remove("PEX_CONFIG")
                .env("PEX_DISABLE_PREFETCH", "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{phase}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    #[ignore = "parent supplies a clean portable directory across launches"]
    fn owned_source_fixture() {
        use crate::app::test_fixtures::{database, TestUi};
        let phase = std::env::var("PEX_OWNED_SOURCE_FIXTURE").unwrap();
        let root = std::env::current_dir().unwrap();
        assert_eq!(config::base_dir(), root);
        let guide = root.join("guide.db");
        let folder = root.join("snapshot-A");
        let offline = root
            .join("offline-B")
            .join("com.plexapp.plugins.library.db");
        if phase == "snapshot" {
            fs::create_dir(&folder).unwrap();
            database(&guide);
            database(&folder.join("com.plexapp.plugins.library.db-2020-01-01"));
            config::save_connection_with_snapshots(&guide, Some(&offline), Some(&folder), false)
                .unwrap();
        }
        if phase == "live" {
            fs::create_dir(offline.parent().unwrap()).unwrap();
            database(&offline);
            config::save_connection_with_snapshots(&guide, Some(&offline), None, false).unwrap();
        }
        let mut ui = TestUi::default();
        // The last launch has a title cache for the selected live source, but
        // both that source and its mirror have gone away since the scan.
        assert_eq!(ui.app.owned_preload.is_some(), phase == "failed-live");
        if phase == "snapshot" || phase == "live" {
            ui.frames_until("verified library", |a| {
                a.rows.first().is_some_and(|r| {
                    r.owned && r.owned_quality == crate::app::types::OwnedQuality::Hd
                }) && a.owned_library.is_some()
                    && !a.owned_scan_in_progress
            });
            let saved: serde_json::Value = serde_json::from_slice(
                &fs::read(root.join(".pex_cache/owned_titles_v5.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                saved["titles"].as_object().unwrap().is_empty(),
                phase == "snapshot"
            );
            if phase == "live" {
                fs::remove_file(&offline).unwrap();
                fs::remove_file(config::local_library_db_path()).unwrap();
                ui.app.start_owned_scan(true);
                ui.frames_until("failed refresh of the same library", |a| {
                    !a.owned_scan_in_progress
                        && a.owned_notice
                            .as_deref()
                            .is_some_and(|s| s.contains("Owned scan error"))
                });
                assert!(ui.app.owned_library.is_some());
                assert!(ui.app.rows[0].owned);
                assert_eq!(
                    ui.app.rows[0].owned_quality,
                    crate::app::types::OwnedQuality::Hd
                );
                return;
            }
            ui.app.open_connection_setup();
            ui.app.connection.use_snapshots = false;
            ui.app.save_selected_connection(&ui.ctx);
        }
        ui.frames_until("library scan or guide-only mode", |a| {
            !a.rows.is_empty() && !a.owned_scan_in_progress && a.owned_notice.is_some()
        });
        assert!(
            !ui.app.rows[0].owned
                && ui.app.rows[0].owned_quality == crate::app::types::OwnedQuality::Unknown
        );
        assert!(ui.app.owned_library.is_none());
        ui.app.hide_owned = true;
        assert!(!ui.app.filtered_guide().days.is_empty());
        if phase == "guide-only" {
            assert!(!ui.app.owned_scan_available);
        } else {
            assert!(ui
                .app
                .owned_notice
                .as_deref()
                .unwrap()
                .contains("Owned scan error"));
        }
        match phase.as_str() {
            "offline-live" => {
                config::save_connection_with_snapshots(&guide, None, None, false).unwrap();
            }
            "manual" => {
                assert!(ui
                    .app
                    .owned_notice
                    .as_deref()
                    .unwrap()
                    .contains("previous connection"));
                config::save_connection_with_snapshots(&guide, Some(&offline), None, true).unwrap();
            }
            "snapshot" | "guide-only" | "failed-live" => {}
            _ => panic!("unexpected fixture phase"),
        }
    }

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
            "snapshots_only",
            "recording_schedule",
            "grouped_airings",
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
        let backups = root.path().join("Plex backups");
        if scenario == "snapshots_only" {
            fs::create_dir(&backups).unwrap();
            super::super::test_fixtures::database(
                &backups.join("com.plexapp.plugins.library.db-2026-01-02"),
            );
        } else {
            super::super::test_fixtures::database(&library);
        }
        if scenario == "grouped_airings" {
            let db = rusqlite::Connection::open(&guide).unwrap();
            db.execute_batch(r#"
                ALTER TABLE media_items ADD COLUMN media_analysis_version INTEGER DEFAULT 8;
                UPDATE metadata_items SET user_thumb_url='plex://unusable', thumb_url='https://fixture.invalid/shared.jpg';
                UPDATE media_items SET begins_at=unixepoch('now')-172800;
                INSERT INTO media_items (id,metadata_item_id,begins_at,extra_data) VALUES
                    (2,1,unixepoch('now')+3600,'{"at:channelCallSign":"A"}'),
                    (3,1,unixepoch('now')+7200,'{"at:channelCallSign":"B"}');
                INSERT INTO metadata_items (id,metadata_type,title,year,guid,user_thumb_url) VALUES
                    (2,1,'Fixture Movie',1980,'imdb://tt7654321','https://fixture.invalid/shared.jpg'),
                    (3,1,'Film without art',2000,NULL,NULL);
                INSERT INTO media_items (id,metadata_item_id,begins_at,extra_data) VALUES
                    (4,2,unixepoch('now')+10800,'{"at:channelCallSign":"A"}'),
                    (5,3,unixepoch('now')+14400,'{"at:channelCallSign":"B"}');
                UPDATE media_items SET width=720,height=576 WHERE id=2;
                UPDATE media_items SET width=1920,height=1080 WHERE id=3;
            "#).unwrap();
        }
        if scenario == "recording_schedule" {
            let guide_db = rusqlite::Connection::open(&guide).unwrap();
            let airing: i64 = guide_db
                .query_row("SELECT begins_at FROM media_items", [], |r| r.get(0))
                .unwrap();
            let db = rusqlite::Connection::open(&library).unwrap();
            db.execute_batch(
                "CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
                CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
                CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);",
            )
            .unwrap();
            db.execute("INSERT INTO media_grabs VALUES (0, json_object('mt:title', 'Fixture Movie', 'mt:year', 2020, 'me:beginsAt', ?1))", [airing]).unwrap();
        }
        if scenario == "guide_only" {
            fs::create_dir_all(portable.join("db")).unwrap();
            fs::copy(&library, config::local_library_db_path()).unwrap();
            fs::create_dir_all(portable.join(".pex_cache")).unwrap();
            fs::write(
                portable.join(".pex_cache").join("owned_all_v3.txt"),
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
            app.connection
                .apply_discovery(plex.display().to_string(), guides, found_library);
            app.connection.guide_only = !(scenario.starts_with("busy_")
                || scenario == "snapshots_only"
                || scenario == "recording_schedule");
            if scenario == "snapshots_only" {
                app.connection.use_snapshots = true;
                assert_eq!(app.connection.selected_snapshot_dir(), Some(plex.clone()));
                app.connection.snapshot_folder = backups.to_string_lossy().into_owned();
            }
            app.save_selected_connection(&ctx);
        }
        let expected = if scenario == "grouped_airings" { 5 } else { 1 };
        frames_until(&mut app, &ctx, |app| {
            app.did_init
                && app.rows.len() == expected
                && !app.owned_scan_in_progress
                && app.prefetch_started
        });
        assert_eq!(app.rows[0].title, "Fixture Movie");
        assert!(app.should_show_grid());
        match scenario.as_str() {
            "grouped_airings" => {
                // The first showing is in the past. The next two are distinct
                // showings of the same film, the fourth a remake sharing art.
                app.rows[2].scheduled = crate::app::scheduled::RecordingStatus::Exact;
                assert_eq!(app.rows[1].url, "https://fixture.invalid/shared.jpg");
                assert_eq!(app.rows[1].key, app.rows[3].key);
                assert_ne!(app.rows[1].film_key, app.rows[3].film_key);
                assert!(app.rows[4].url.is_empty() && app.rows[4].key.is_empty());
                let guide = app.filtered_guide();
                assert_eq!(
                    guide
                        .days
                        .into_iter()
                        .flat_map(|(_, v)| v)
                        .collect::<Vec<_>>(),
                    vec![1, 3, 4]
                );
                assert!(guide.scheduled_elsewhere.contains_key(&1));
                assert_eq!(app.filtered_showings_for(1), vec![1, 2]);
                app.selected_channels.insert("B".into());
                let guide = app.filtered_guide();
                assert_eq!(
                    guide
                        .days
                        .into_iter()
                        .flat_map(|(_, v)| v)
                        .collect::<Vec<_>>(),
                    vec![2, 4]
                );
                assert!(guide.scheduled_elsewhere.is_empty());
                assert_eq!(app.filtered_showings_for(2), vec![2]);
                app.selected_channels.clear();
                app.filter_hd_only = true;
                assert_eq!(app.filtered_showings_for(1), vec![2]);
                app.filter_hd_only = false;
                app.hide_owned = true;
                for row in &mut app.rows {
                    row.owned = true;
                    row.owned_quality = crate::app::types::OwnedQuality::Sd;
                }
                let upgrades: Vec<_> = app
                    .filtered_guide()
                    .days
                    .into_iter()
                    .flat_map(|(_, v)| v)
                    .collect();
                assert_eq!(upgrades, vec![2]);
            }
            "recording_schedule" => {
                assert_eq!(
                    app.rows[0].scheduled,
                    crate::app::scheduled::RecordingStatus::Exact
                );
                rusqlite::Connection::open(&library)
                    .unwrap()
                    .execute("DROP TABLE media_grabs", [])
                    .unwrap();
                app.start_owned_scan(true);
                frames_until(&mut app, &ctx, |app| !app.owned_scan_in_progress);
                assert_eq!(
                    app.rows[0].scheduled,
                    crate::app::scheduled::RecordingStatus::None
                );
                assert!(app.rows[0].owned);
            }
            "snapshots_only" => {
                assert!(app.rows[0].owned);
                assert!(app.owned_notice.as_deref().unwrap().contains("2026-01-02"));
                let stored = config::load_config();
                assert_eq!(stored.library_snapshot_dir, Some(backups.clone()));
                assert!(stored.plex_library_db_source.is_none());
                drop(app);
                let mut reopened = PexApp::default();
                frames_until(&mut reopened, &ctx, |app| {
                    app.did_init && app.rows.len() == 1 && !app.owned_scan_in_progress
                });
                assert!(reopened.rows[0].owned);
                assert!(reopened.setup_warnings.is_empty());
                reopened.open_connection_setup();
                assert!(reopened.connection.use_snapshots);
                assert_eq!(reopened.connection.selected_snapshot_dir(), Some(backups));
                assert_eq!(
                    reopened.connection.guides[reopened.connection.selected],
                    guide
                );
            }
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
                app.open_connection_setup();
                assert_eq!(app.connection.library.as_ref(), Some(&library));
                assert_eq!(app.connection.guides[app.connection.selected], guide);
                app.connection.use_snapshots = true;
                config::save_connection_with_snapshots(
                    &guide,
                    app.connection.library.as_deref(),
                    app.connection.selected_snapshot_dir().as_deref(),
                    false,
                )
                .unwrap();
                let saved = config::load_config();
                assert_eq!(saved.plex_library_db_source.as_ref(), Some(&library));
                assert_eq!(saved.library_snapshot_dir, Some(plex.clone()));
                config::save_connection_with_snapshots(&guide, Some(&library), None, false)
                    .unwrap();
                assert!(config::load_config().library_snapshot_dir.is_none());
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
