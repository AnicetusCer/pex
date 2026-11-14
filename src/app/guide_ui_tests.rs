use super::test_fixtures::{database, TestUi};
use crate::config;
use rusqlite::{params, Connection};
use std::{env, fs, process::Command};

#[test]
fn browse_repeats_and_owned_details() {
    run_portable_test("app::guide_ui_tests::portable_guide_details");
}

#[test]
fn distinguish_booked_and_unresolved_simulcasts() {
    run_portable_test("app::guide_ui_tests::portable_simulcasts");
}

#[test]
fn hd_upgrade_requires_evidence_for_the_airing() {
    run_portable_test("app::guide_ui_tests::portable_sky_mix_quality");
}

fn run_portable_test(test: &str) {
    let dir = tempfile::tempdir().unwrap();
    let result = Command::new(env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture"])
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

fn painted_airing_labels(ui: &mut TestUi) -> Vec<String> {
    ui.frame(Vec::new())
        .shapes
        .into_iter()
        .filter_map(|s| match s.shape {
            eframe::egui::Shape::Text(t) if t.galley.text().starts_with("Airing ") => {
                Some(t.galley.text().to_owned())
            }
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "parent creates an isolated portable install"]
fn portable_sky_mix_quality() {
    use super::types::AiringQuality;

    let portable = std::path::PathBuf::from(env::var_os("PEX_GUIDE_DETAILS_FIXTURE").unwrap());
    let source = portable.join("source");
    fs::create_dir(&source).unwrap();
    let guide = source.join("guide.db");
    let library = source.join("library.db");
    let start = chrono::Utc::now().timestamp() + 1800;
    let repeat = start + 1800;
    for path in [&guide, &library] {
        database(path);
        Connection::open(path).unwrap().execute_batch(
            "UPDATE metadata_items SET title='Sky Mix Fixture', original_title='Sky Mix Fixture',
                year=1997, user_thumb_url=NULL;"
        ).unwrap();
    }
    {
        let db = Connection::open(&guide).unwrap();
        db.execute_batch(
            "ALTER TABLE media_items ADD COLUMN media_analysis_version INTEGER;
             DELETE FROM media_items;",
        )
        .unwrap();
        for (time, call_sign, label, number) in [
            (start, "SKYMIX", "011 SKYMIX (Sky Mix)", "011"),
            (repeat, "SKYMIX", "011 SKYMIX (Sky Mix)", "011"),
            (start + 3600, "SKYMIXHD", "141 SKYMIXHD (Sky Mix HD)", "141"),
        ] {
            let channel = serde_json::json!({
                "at:channelVcn":number, "at:channelCallSign":call_sign, "at:channelTitle":label,
            });
            db.execute(
                "INSERT INTO media_items(metadata_item_id,begins_at,width,height,media_analysis_version,extra_data)
                 VALUES(1,?1,1280,720,0,?2)", params![time,channel.to_string()]
            ).unwrap();
        }
        let db = Connection::open(&library).unwrap();
        db.execute_batch(
            "UPDATE media_items SET width=718,height=432;
             UPDATE media_parts SET file='/fixture/Sky Mix Fixture (1997).mkv';
             CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
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
    let mut ui = TestUi::default();
    ui.frames_until("owned SD film and booked Sky Mix repeat", |app| {
        app.rows.len() == 3
            && app.rows.iter().all(|r| r.owned && !r.owned_hd)
            && app
                .rows
                .iter()
                .any(|r| r.scheduled == super::scheduled::RecordingStatus::Exact)
    });
    let plain = ui
        .app
        .rows
        .iter()
        .position(|r| r.channel_raw.as_deref() == Some("SKYMIX"))
        .unwrap();
    let hd = ui
        .app
        .rows
        .iter()
        .position(|r| r.channel_raw.as_deref() == Some("SKYMIXHD"))
        .unwrap();
    assert_eq!(ui.app.rows[plain].airing_quality, AiringQuality::Unknown);
    assert_eq!(ui.app.rows[hd].airing_quality, AiringQuality::Hd);
    let showings = ui.app.filtered_showings_for(plain);
    assert_eq!(showings.len(), 3);
    assert_eq!(showings[0], plain);
    assert_eq!(showings[2], hd);
    assert_eq!(
        ui.app.rows[showings[1]].airing_quality,
        AiringQuality::Unknown
    );
    let upgrade_visible = |ui: &mut TestUi| {
        ui.frame(Vec::new())
            .shapes
            .iter()
            .any(|s| matches!(&s.shape, eframe::egui::Shape::Text(t) if t.galley.text() == "HD ↑"))
    };
    // A card represents its earliest matching showing. The later HD airing mustn't
    // lend its upgrade badge to the earlier, unanalysed listing on channel 011.
    assert_eq!(ui.app.filtered_guide().days[0].1, [plain]);
    assert!(!upgrade_visible(&mut ui));
    let card = ui
        .ctx
        .read_response(eframe::egui::Id::new(("card_sel", plain)))
        .unwrap();
    ui.click(card.rect.center());
    ui.label("Owned SD");
    assert!(painted_airing_labels(&mut ui).is_empty());
    ui.press("Other showings (2)");
    ui.label("Scheduled to record");
    ui.label("Airing HD");
    assert_eq!(painted_airing_labels(&mut ui), ["Airing HD"]);
    ui.press("Filters");
    ui.press("HD only");
    assert_eq!(ui.app.filtered_showings_for(hd), [hd]);
    assert_eq!(ui.app.filtered_guide().days[0].1, [hd]);
    assert!(upgrade_visible(&mut ui));
}

#[test]
#[ignore = "parent creates an isolated portable install"]
fn portable_simulcasts() {
    use super::scheduled::RecordingStatus;
    use super::types::AiringQuality;

    let portable = std::path::PathBuf::from(env::var_os("PEX_GUIDE_DETAILS_FIXTURE").unwrap());
    assert_eq!(config::base_dir(), portable);
    let source = portable.join("source");
    fs::create_dir(&source).unwrap();
    let guide = source.join("guide.db");
    let library = source.join("library.db");
    database(&guide);
    database(&library);

    let start = chrono::Utc::now().timestamp() + 1800;
    let repeat = start + 7200;
    let unresolved = start + 3600;
    {
        let db = Connection::open(&guide).unwrap();
        db.execute_batch(
            "ALTER TABLE media_items ADD COLUMN media_analysis_version INTEGER;
             UPDATE metadata_items SET user_thumb_url=NULL;
             DELETE FROM media_items;
             INSERT INTO metadata_items(id,title,year,metadata_type,guid)
             VALUES(2,'Unresolved Fixture',2025,1,'imdb://tt2345678');",
        )
        .unwrap();
        for (movie, time) in [(1, start), (1, repeat), (2, unresolved)] {
            for (station, number, call_sign, label, width, height) in [
                (
                    "lineup-station-sd",
                    "6",
                    "MOV4",
                    "6 MOV4 (Movie Four)",
                    720,
                    576,
                ),
                (
                    "lineup-station-hd",
                    "106",
                    "MOV4HD",
                    "106 MOV4HD (Movie Four HD)",
                    1920,
                    1080,
                ),
            ] {
                let channel = serde_json::json!({
                    "at:channelIdentifier": station, "at:channelVcn": number,
                    "at:channelCallSign": call_sign, "at:channelTitle": label,
                });
                db.execute(
                    "INSERT INTO media_items(metadata_item_id,begins_at,width,height,extra_data,media_analysis_version)
                     VALUES(?1,?2,?3,?4,?5,8)",
                    params![movie, time, width, height, channel.to_string()],
                )
                .unwrap();
            }
        }
        let db = Connection::open(&library).unwrap();
        db.execute_batch(
            "CREATE TABLE media_grabs(status INTEGER, extra_data TEXT);
             CREATE TABLE media_subscriptions(id INTEGER, extra_data TEXT);
             CREATE TABLE metadata_subscription_desired_items(sub_id INTEGER, remote_id TEXT);",
        )
        .unwrap();
        // A single station serves both times. Plex encodes the label inside
        // the ID=label pair, then encodes the entire pair in the subscription.
        let known = serde_json::json!({
            "hi:guid": "imdb://tt1234567", "pr:oneShot": 1,
            "pv:airingTimes": format!("{start}%2C{repeat}"),
            "pv:airingChannels": "lineup-station-hd%3D106%2520MOV4HD%2520%2528Movie%2520Four%2520HD%2529",
        });
        let time_only = serde_json::json!({
            "hi:guid": "imdb://tt2345678", "pr:oneShot": 1,
            "pv:airingTimes": unresolved.to_string(),
        });
        db.execute(
            "INSERT INTO media_subscriptions VALUES(1,?1),(2,?2)",
            params![known.to_string(), time_only.to_string()],
        )
        .unwrap();
    }
    config::save_connection(&guide, Some(&library), false).unwrap();
    let mut ui = TestUi {
        screen: Some(eframe::egui::vec2(1360.0, 1400.0)),
        ..Default::default()
    };
    ui.frames_until("simulcast bookings", |app| {
        app.rows.len() == 6
            && app
                .rows
                .iter()
                .any(|r| r.scheduled != RecordingStatus::None)
    });
    for row in &ui.app.rows {
        let expected = match (row.title.as_str(), row.airing_quality) {
            ("Fixture Movie", AiringQuality::Hd) => RecordingStatus::Exact,
            ("Fixture Movie", AiringQuality::Sd) => RecordingStatus::None,
            ("Unresolved Fixture", AiringQuality::Hd | AiringQuality::Sd) => RecordingStatus::Slot,
            _ => panic!("Unexpected simulcast fixture row"),
        };
        assert_eq!(
            row.scheduled, expected,
            "{} {:?}",
            row.title, row.channel_raw
        );
    }
    ui.press("REC?");
    ui.label("Recording scheduled; channel unconfirmed");
    ui.press("Other REC");
    ui.press("Other showings (3)");
    ui.label("Scheduled to record");
    ui.label("Airing SD");
    ui.label("Airing HD");
    ui.press("Filters");
    ui.press("HD only");
    ui.label("REC");
    // Hiding the SD sibling cannot confirm the station Plex left unspecified.
    ui.label("REC?");
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
        db.execute_batch(
            "ALTER TABLE media_items ADD COLUMN media_analysis_version INTEGER;
             UPDATE metadata_items SET user_thumb_url=NULL;",
        )
        .unwrap();
        db.execute(
            "UPDATE media_items SET begins_at=?1,extra_data=?2,media_analysis_version=8",
            params![start, r#"{"at:channelCallSign":"Channel One"}"#],
        )
        .unwrap();
        for (channel, width, height) in [("Channel Two", 720, 576), ("Channel Three", 1920, 1080)] {
            db.execute(
                "INSERT INTO media_items (metadata_item_id,begins_at,width,height,extra_data,media_analysis_version)
                 VALUES(1,?1,?2,?3,?4,8)",
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
    ui.label("2 Plex movies match this airing.");
    assert!(painted_airing_labels(&mut ui).is_empty());
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
