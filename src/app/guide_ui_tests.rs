use super::test_fixtures::{database, TestUi};
use crate::config;
use rusqlite::{params, Connection};
use std::{env, fs, process::Command};

#[test]
fn browse_repeats_and_owned_details() {
    let dir = tempfile::tempdir().unwrap();
    let result = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "app::guide_ui_tests::portable_guide_details",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(dir.path())
        .env("PEX_GUIDE_DETAILS_FIXTURE", dir.path())
        .env("PEX_BASE_DIR", dir.path())
        .env("HOME", dir.path())
        .env("USERPROFILE", dir.path())
        .env("PEX_DISABLE_PREFETCH", "1")
        .env_remove("PEX_CONFIG")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "parent creates an isolated portable install"]
fn portable_guide_details() {
    let portable = env::var_os("PEX_GUIDE_DETAILS_FIXTURE").unwrap();
    assert_eq!(config::base_dir(), std::path::Path::new(&portable));
    assert_eq!(config::base_dir(), env::current_dir().unwrap());
    let start = chrono::Utc::now().timestamp() + 1800;
    let repeat = start + 7200;
    let source = config::base_dir().join("source");
    fs::create_dir(&source).unwrap();
    let guide = source.join("guide.db");
    let library = source.join("library.db");
    database(&guide);
    database(&library);
    {
        let db = Connection::open(&guide).unwrap();
        db.execute("UPDATE metadata_items SET user_thumb_url=NULL", [])
            .unwrap();
        db.execute(
            "UPDATE media_items SET begins_at=?1,extra_data=?2",
            params![start, r#"{"at:channelCallSign":"Channel One"}"#],
        )
        .unwrap();
        db.execute(
            "INSERT INTO media_items (id,metadata_item_id,begins_at,extra_data) VALUES(2,1,?1,?2)",
            params![repeat, r#"{"at:channelCallSign":"Channel Two HD"}"#],
        )
        .unwrap();
        let db = Connection::open(&library).unwrap();
        db.execute_batch(
            "CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
            CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
            CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);",
        )
        .unwrap();
        db.execute(
            "INSERT INTO media_grabs VALUES(0,?1)",
            [serde_json::json!({"mt:guid":"imdb://tt1234567","me:beginsAt":repeat}).to_string()],
        )
        .unwrap();
    }
    config::save_connection(&guide, Some(&library), false).unwrap();
    let mut ui = TestUi {
        screen: Some(eframe::egui::vec2(1360.0, 1400.0)),
        ..Default::default()
    };
    ui.frames_until("owned and scheduled guide", |app| {
        app.rows.len() == 2
            && app.rows.iter().any(|r| r.owned)
            && app.rows.iter().any(|r| r.scheduled)
    });
    ui.press("Other REC");
    ui.press("Other showings (1)");
    ui.label("Scheduled to record");
    ui.press("Owned Details");
    ui.label("Matched by the same Plex metadata identifier.");
    ui.label("/fixture/Fixture Movie (2020).mkv");

    // A completed path check stays on screen without re-probing the source
    // until requested, including when that source disappears between frames.
    let guide_label = format!("EPG source: {}", guide.display());
    ui.press("Advanced.");
    ui.label(&guide_label);
    fs::rename(&guide, source.join("offline.db")).unwrap();
    ui.label(&guide_label);
    ui.press("Refresh path status");
    ui.label(&format!("{guide_label} (unavailable)"));
}
