use super::PexApp;
use eframe::egui as eg;
use rusqlite::Connection;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn database(path: &Path) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(include_str!("test_fixtures.sql")).unwrap();
    db.execute(
        "UPDATE media_items SET begins_at=?1",
        [chrono::Utc::now().timestamp() + 1800],
    )
    .unwrap();
}

// Locate controls in the frame that egui actually painted, so this exercises
// the same controls as a user without tying clicks to a font or DPI setting.
#[derive(Default)]
pub(super) struct TestUi {
    pub(super) app: PexApp,
    pub(super) ctx: eg::Context,
    pub(super) screen: Option<eg::Vec2>,
}

impl TestUi {
    pub(super) fn frames_until(&mut self, goal: &str, condition: impl Fn(&PexApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition(&self.app) {
            self.frame(Vec::new());
            assert!(Instant::now() < deadline, "Timed out waiting for {goal}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(super) fn frame(&mut self, events: Vec<eg::Event>) -> eg::FullOutput {
        let input = eg::RawInput {
            screen_rect: Some(eg::Rect::from_min_size(
                eg::Pos2::ZERO,
                self.screen.unwrap_or(eg::vec2(1100.0, 850.0)),
            )),
            events,
            ..Default::default()
        };
        self.ctx.run(input, |ctx| self.app.update_ui(ctx))
    }

    pub(super) fn label(&mut self, label: &str) -> eg::Pos2 {
        let mut visible = Vec::new();
        // Windows need a sizing frame, and background path checks can finish between frames.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
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
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("Control was not visible: {label}. Labels: {visible:?}");
    }

    pub(super) fn click(&mut self, pos: eg::Pos2) {
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

    pub(super) fn press(&mut self, label: &str) {
        let pos = self.label(label);
        self.click(pos);
    }
}
