use super::PexApp;
use crate::config;
use eframe::egui as eg;
use std::{
    env, fs,
    process::Command,
    time::{Duration, Instant},
};

// Locate controls in the frame that egui actually painted, so this exercises
// the setup gate and its modal without tying clicks to a font or DPI setting.
#[derive(Default)]
struct SetupUi {
    app: PexApp,
    ctx: eg::Context,
}

impl SetupUi {
    fn frames_until(&mut self, goal: &str, condition: impl Fn(&PexApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition(&self.app) {
            self.frame(Vec::new());
            assert!(Instant::now() < deadline, "Timed out waiting for {goal}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn frame(&mut self, events: Vec<eg::Event>) -> eg::FullOutput {
        let input = eg::RawInput {
            screen_rect: Some(eg::Rect::from_min_size(
                eg::Pos2::ZERO,
                eg::vec2(1100.0, 850.0),
            )),
            events,
            ..Default::default()
        };
        self.ctx.run(input, |ctx| self.app.update_ui(ctx))
    }

    fn label(&mut self, label: &str) -> eg::Pos2 {
        let mut visible = Vec::new();
        // A new egui Window gets a sizing frame before its contents are painted.
        for _ in 0..3 {
            let frame = self.frame(Vec::new());
            visible.clear();
            for clipped in frame.shapes {
                if let eg::Shape::Text(text) = clipped.shape {
                    visible.push(text.galley.text().to_owned());
                    if text.galley.text() == label {
                        let center = text.visual_bounding_rect().center();
                        if clipped.clip_rect.contains(center) {
                            return center;
                        }
                    }
                }
            }
        }
        panic!(
            "Control was not visible: {label}. Editor open: {}. Labels: {visible:?}",
            self.app.rating_key_editor.is_some()
        );
    }

    fn click(&mut self, pos: eg::Pos2) {
        self.frame(vec![eg::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.frame(vec![eg::Event::PointerButton {
                pos,
                button: eg::PointerButton::Primary,
                pressed,
                modifiers: eg::Modifiers::NONE,
            }]);
        }
    }

    fn press(&mut self, label: &str) {
        let pos = self.label(label);
        self.click(pos);
    }

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
    let mut ui = SetupUi::default();
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
            let mut guide_ui = SetupUi::default();
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
                    app.rating_states.get(&app.rows[0].key),
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
