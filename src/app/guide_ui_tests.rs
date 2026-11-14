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
        for (channel, width, height) in [("Channel Two", 720, 576), ("Channel Three", 1920, 1080)] {
            db.execute(
                "INSERT INTO media_items (metadata_item_id,begins_at,width,height,extra_data)
                 VALUES(1,?1,?2,?3,?4)",
                params![
                    repeat,
                    width,
                    height,
                    serde_json::json!({"at:channelCallSign":channel, "at:channelTitle":channel})
                        .to_string()
                ],
            )
            .unwrap();
        }
        db.execute_batch(
            "INSERT INTO metadata_items(id,title,year,metadata_type) VALUES(2,'Neighbour Fixture',2025,1);",
        )
        .unwrap();
        db.execute(
            "INSERT INTO media_items(metadata_item_id,begins_at) VALUES(2,?1)",
            [repeat + 60],
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
            [serde_json::json!({"mt:guid":"imdb://tt1234567","me:beginsAt":repeat,"me:channelTitle":"Channel Three"}).to_string()],
        )
        .unwrap();
        for (id, year) in [(2, 2024), (3, 2026)] {
            db.execute(
                "INSERT INTO metadata_items(id,title,year,metadata_type) VALUES(?1,'Neighbour Fixture',?2,1)",
                params![id, year],
            )
            .unwrap();
            db.execute(
                "INSERT INTO media_items(id,metadata_item_id) VALUES(?1,?1)",
                [id],
            )
            .unwrap();
            db.execute(
                "INSERT INTO media_parts(media_item_id,file) VALUES(?1,?2)",
                params![id, format!("/fixture/Neighbour ({year}).mkv")],
            )
            .unwrap();
        }
    }
    config::save_connection(&guide, Some(&library), false).unwrap();
    let mut ui = TestUi {
        screen: Some(eframe::egui::vec2(1360.0, 1400.0)),
        ..Default::default()
    };
    ui.frames_until("owned and scheduled guide", |app| {
        app.rows.len() == 4
            && app.rows.iter().all(|r| r.owned)
            && app
                .rows
                .iter()
                .any(|r| r.scheduled == super::scheduled::RecordingStatus::Exact)
    });
    ui.press("Other REC");
    ui.press("Other showings (2)");
    ui.label("Scheduled to record");
    ui.label("Airing SD");
    let booked_channels: Vec<_> = ui
        .app
        .rows
        .iter()
        .filter(|row| row.scheduled == super::scheduled::RecordingStatus::Exact)
        .map(|row| row.channel_raw.as_deref())
        .collect();
    assert_eq!(booked_channels, [Some("Channel Three")]);
    let frame = ui.frame(Vec::new());
    let labels: Vec<_> = frame
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            eframe::egui::Shape::Text(t) => Some(t.galley.text()),
            _ => None,
        })
        .collect();
    let count_label = |label: &str| labels.iter().filter(|&&text| text == label).count();
    assert_eq!(count_label("Scheduled to record"), 1);
    assert_eq!(count_label("Airing HD"), 2);
    assert_eq!(count_label("Airing SD"), 1);
    ui.press("Owned Details");
    ui.label("Matched by the same Plex metadata identifier.");
    ui.label("/fixture/Fixture Movie (2020).mkv");
    ui.press("Filters");
    ui.press("HD only");
    ui.label("Other showings (1)");
    let selected = ui.app.selected_idx.unwrap();
    let channels: Vec<_> = ui
        .app
        .filtered_showings_for(selected)
        .into_iter()
        .map(|i| ui.app.rows[i].channel_raw.as_deref().unwrap())
        .collect();
    assert_eq!(channels, ["Channel One", "Channel Three"]);
    ui.press("HD only");
    ui.press("Filters");
    let idx = ui
        .app
        .rows
        .iter()
        .position(|r| r.title == "Neighbour Fixture")
        .unwrap();
    let card = ui
        .ctx
        .read_response(eframe::egui::Id::new(("card_sel", idx)))
        .unwrap();
    ui.click(card.rect.center());
    ui.label("Airing quality unknown");
    ui.label("2 Plex movies match this airing.");
    ui.label("/fixture/Neighbour (2024).mkv");
    ui.label("/fixture/Neighbour (2026).mkv");

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
