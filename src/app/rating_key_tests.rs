use super::test_fixtures::TestUi;
use crate::config;
use eframe::egui as eg;
use std::{env, fs, process::Command};

impl TestUi {
    fn open_editor(&mut self) {
        self.press("Set up ratings…");
        self.label("TMDb ratings key");
    }

    fn enter_key(&mut self, key: &str) {
        let id = eg::Id::new("tmdb_rating_key_input");
        let rect = self
            .ctx
            .read_response(id)
            .expect("key input is visible")
            .rect;
        self.click(rect.center());
        assert!(
            self.ctx.memory(|m| m.has_focus(id)),
            "key input receives focus"
        );
        self.frame(vec![
            eg::Event::Key {
                key: eg::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: eg::Modifiers::COMMAND,
            },
            eg::Event::Paste(key.to_owned()),
        ]);
    }
}

#[test]
fn ratings_key_through_setup_and_restart() {
    let portable = tempfile::tempdir().unwrap();
    for phase in ["save", "manage", "removed"] {
        let output = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "app::rating_key_tests::isolated_setup_editor",
                "--ignored",
                "--nocapture",
            ])
            .env("PEX_RATINGS_UI_FIXTURE", phase)
            .env("PEX_BASE_DIR", portable.path())
            .env("HOME", portable.path())
            .env("USERPROFILE", portable.path())
            .env("PEX_DISABLE_PREFETCH", "1")
            .env_remove("PEX_CONFIG")
            .current_dir(portable.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Setup UI {phase} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
#[ignore = "runs in the test's temporary portable directory in three separate processes"]
fn isolated_setup_editor() {
    // Require the fixture marker before the app or config loader is constructed.
    let phase = env::var("PEX_RATINGS_UI_FIXTURE").unwrap();
    assert_eq!(env::current_dir().unwrap(), config::base_dir());
    let mut ui = TestUi::default();
    // Wait for the real asynchronous connection check before observing button
    // positions: the welcome message changes the layout when the check finishes.
    ui.frames_until("connection check", |app| app.setup_checked);
    let saved_key = || config::load_config().tmdb_api_key;
    let first = "ratings-fixture-first";
    let replacement = "ratings-fixture-replacement";

    match phase.as_str() {
        "save" => {
            assert_eq!(saved_key(), None);
            ui.open_editor();
            ui.enter_key(first);
            ui.press("Save key");
            assert_eq!(saved_key().as_deref(), Some(first));
        }
        "manage" => {
            assert_eq!(saved_key().as_deref(), Some(first));
            ui.open_editor();
            ui.enter_key("discard-this-fixture");
            ui.press("Cancel");
            assert_eq!(saved_key().as_deref(), Some(first));

            ui.open_editor();
            ui.enter_key(replacement);
            ui.press("Save key");
            assert_eq!(saved_key().as_deref(), Some(replacement));

            ui.open_editor();
            ui.press("Remove key");
            assert_eq!(saved_key(), None);
        }
        "removed" => {
            assert_eq!(saved_key(), None);
            ui.open_editor();
            ui.press("Cancel");
            // Boot the fixture guide and select its only film. A rating lookup
            // with no title or IMDb ID completes locally.
            let guide = config::local_db_path();
            fs::create_dir_all(guide.parent().unwrap()).unwrap();
            super::test_fixtures::database(&guide);
            let mut guide_ui = TestUi::default();
            guide_ui.frames_until("fixture guide", |app| {
                app.boot_phase == super::types::BootPhase::Ready && app.rows.len() == 1
            });
            guide_ui.app.rows[0].title.clear();
            guide_ui.app.rows[0].guid = None;
            guide_ui.app.selected_idx = Some(0);
            guide_ui.press("⭐ Rating");
            guide_ui.label("TMDb ratings key");
            guide_ui.enter_key("ratings-fixture-detail");
            guide_ui.press("Save key");
            guide_ui.frames_until("resumed film rating", |app| {
                matches!(
                    app.rating_states.get(&app.rows[0].film_key),
                    Some(super::types::RatingState::NotFound)
                )
            });
            guide_ui.label("TMDb rating not found.");
            guide_ui.press("Advanced.");
            guide_ui.press("Ratings key…");
            guide_ui.label("TMDb ratings key");
            guide_ui.press("Remove key");
            assert_eq!(saved_key(), None);
        }
        _ => panic!("Unknown ratings UI fixture phase"),
    }
}
