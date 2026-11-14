//! Launch the executable from SNAPSHOT_TEST_ZIP on an isolated synthetic Plex setup.
//! On headless Linux, run the test under Xvfb.

use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
const GUIDE: &str = "tv.plex.providers.epg.cloud-smoke.db";
const LIBRARY: &str = "com.plexapp.plugins.library.db";
const LIVE_MARKER: &str = "db/plex_library.db.last_sync";
const MIRROR: &str = "db/plex_library.db";

fn extract_bundle(bundle: &Path, app: &Path) -> Result {
    fs::create_dir(app)?;
    let mut archive = zip::ZipArchive::new(File::open(bundle)?)?;
    let exe = format!("pex{}", env::consts::EXE_SUFFIX);
    let expected = [
        "LICENSE",
        "NOTICE",
        "README.md",
        "config.json",
        "config_example.linux",
        "config_example.windows",
        &exe,
    ];
    assert_eq!(
        archive.len(),
        expected.len(),
        "unexpected portable package layout"
    );
    for name in expected {
        let mut dst = File::create_new(app.join(name))?;
        io::copy(&mut archive.by_name(name)?, &mut dst)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(app.join(&exe), fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    if let Some(driver) = env::var_os("SNAPSHOT_TEST_SWIFTSHADER") {
        fs::copy(driver, app.join("vulkan-1.dll"))?;
    }
    Ok(())
}

fn seed(database: &Path, title: &str) -> Result {
    let db = Connection::open(database)?;
    db.execute_batch(include_str!("../src/app/test_fixtures.sql"))?;
    db.execute(
        "UPDATE metadata_items SET title=?1, original_title=?1, user_thumb_url='http://127.0.0.1:9/fixture'",
        [title],
    )?;
    let starts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 600;
    db.execute("UPDATE media_items SET begins_at=?1", [starts])?;
    Ok(())
}

fn assert_title(path: &Path, expected: &str) -> Result {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let title: String = db.query_row("SELECT title FROM metadata_items WHERE id=1", [], |r| {
        r.get(0)
    })?;
    assert_eq!(title, expected, "{}", path.display());
    Ok(())
}

struct RunningApp(Child);

impl Drop for RunningApp {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(app: &Path, config: &Value, expected_title: &str, live_copy: bool) -> Result {
    let root = app.parent().unwrap();
    let cache = app.join(".pex_cache/owned_titles_v5.json");
    if cache.try_exists()? {
        fs::remove_file(&cache)?;
    }
    let config_file = app.join("pex-config.json");
    fs::write(&config_file, serde_json::to_vec(config)?)?;
    let mut log = tempfile::tempfile()?;
    let mut command = Command::new(app.join(format!("pex{}", env::consts::EXE_SUFFIX)));
    command
        .current_dir(root)
        .env_remove("PEX_CONFIG")
        .env("PEX_DISABLE_PREFETCH", "1")
        .env_remove("PEX_BASE_DIR")
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("APPDATA", root.join("roaming"))
        .env("LOCALAPPDATA", root.join("local"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    let mut app_process = RunningApp(command.spawn()?);
    let guide_marker = app.join("db/plex_epg.db.last_sync");
    let title_key = format!("{}:2020", expected_title.to_lowercase());
    let start = Instant::now();
    loop {
        let status = app_process.0.try_wait()?;
        if status.is_some() || start.elapsed() > Duration::from_secs(60) {
            let offset = log.metadata()?.len().saturating_sub(4096);
            log.seek(SeekFrom::Start(offset))?;
            let mut tail = Vec::new();
            log.read_to_end(&mut tail)?;
            return Err(format!(
                "PEX did not load {expected_title} after {:?} (exit {status:?}).\n{}",
                start.elapsed(),
                String::from_utf8_lossy(&tail)
            )
            .into());
        }
        if guide_marker.try_exists()? && cache.try_exists()? {
            let saved: Value = serde_json::from_reader(File::open(&cache)?)?;
            if let Some(folder) = config.get("library_snapshot_dir") {
                assert_eq!(saved["source"], json!({"Snapshots": folder}));
                assert_eq!(saved["titles"], json!({}));
                break;
            }
            if saved["titles"][&title_key] == "hd" {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_title(&app.join("db/plex_epg.db"), "Snapshot Fixture")?;
    assert_title(&app.join(MIRROR), expected_title)?;
    assert_eq!(app.join(LIVE_MARKER).try_exists()?, live_copy);
    Ok(())
}

// The fixture sources are flat directories. Compare their complete contents and
// timestamps before and after each scenario, including any SQLite sidecars.
fn source_state(folder: &Path) -> io::Result<BTreeMap<PathBuf, (SystemTime, Vec<u8>)>> {
    let mut files = BTreeMap::new();
    for file in fs::read_dir(folder)? {
        let path = file?.path();
        files.insert(
            path.clone(),
            (path.metadata()?.modified()?, fs::read(path)?),
        );
    }
    Ok(files)
}

#[derive(Default)]
struct ReadOnlyFiles(Vec<(PathBuf, fs::Permissions)>);

impl Drop for ReadOnlyFiles {
    fn drop(&mut self) {
        for (path, permissions) in self.0.drain(..) {
            let _ = fs::set_permissions(path, permissions);
        }
    }
}

fn with_read_only_sources<T>(folders: &[&Path], check: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut restore = ReadOnlyFiles::default();
    let mut originals = Vec::new();
    for &folder in folders {
        let state = source_state(folder)?;
        for path in state.keys() {
            let permissions = path.metadata()?.permissions();
            let mut read_only = permissions.clone();
            read_only.set_readonly(true);
            fs::set_permissions(path, read_only)?;
            restore.0.push((path.clone(), permissions));
        }
        originals.push((folder, state));
    }
    let result = check()?;
    for (folder, before) in originals {
        assert!(
            source_state(folder)? == before,
            "PEX changed files in {}",
            folder.display()
        );
    }
    Ok(result)
}

#[test]
#[ignore = "requires a packaged app in SNAPSHOT_TEST_ZIP and a graphical session"]
fn packaged_app_reads_live_busy_and_completed_sources() -> Result {
    let bundle = PathBuf::from(
        env::var_os("SNAPSHOT_TEST_ZIP").ok_or("set SNAPSHOT_TEST_ZIP to the portable ZIP")?,
    );
    let root = tempfile::tempdir()?;
    let app = root.path().join("portable");
    extract_bundle(&bundle, &app)?;
    let data = root.path().join("Plex Databases");
    let backups = root.path().join("Completed Plex backups");
    fs::create_dir(&data)?;
    fs::create_dir(&backups)?;
    let guide = data.join(GUIDE);
    let library = data.join(LIBRARY);
    seed(&guide, "Snapshot Fixture")?;
    seed(&library, "Live Library")?;
    let live_config = json!({
        "plex_epg_db_source": guide,
        "plex_library_db_source": library,
        "tmdb_api_key": "fixture-ratings-key",
        "custom_setting": {"preserve": true}
    });
    // Upgrade an extracted installation with the direct-DVR preview's saved token.
    // A queued connection reveals a startup request even after the child has exited.
    let old_server = TcpListener::bind("127.0.0.1:0")?;
    old_server.set_nonblocking(true)?;
    let mut preview_config = live_config.clone();
    preview_config["live_dvr"] = json!({
        "server_url": format!("http://{}/", old_server.local_addr()?),
        "token": "fixture-retired-dvr-token"
    });
    with_read_only_sources(&[&data], || {
        launch(&app, &preview_config, "Live Library", true)
    })?;
    let migrated: Value = serde_json::from_reader(File::open(app.join("pex-config.json"))?)?;
    assert_eq!(migrated, live_config);
    assert!(
        old_server
            .accept()
            .is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock),
        "PEX contacted the retired DVR server"
    );

    let marker = app.join(LIVE_MARKER);
    let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
    File::options()
        .write(true)
        .open(&marker)?
        .set_modified(old)?;
    {
        // Hold the writer open so the changed title is still in the WAL when PEX copies it.
        let db = Connection::open(&library)?;
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        assert_eq!(mode, "wal");
        db.pragma_update(None, "wal_autocheckpoint", 0)?;
        db.execute(
            "UPDATE metadata_items SET title=?1, original_title=?1",
            params!["Updated Live Library"],
        )?;
        assert!(data.join(format!("{LIBRARY}-wal")).metadata()?.len() > 0);
        with_read_only_sources(&[&data], || {
            launch(&app, &live_config, "Updated Live Library", true)
        })?;
    }
    assert!(marker.metadata()?.modified()? > old);

    let fresh_app = root.path().join("fallback-portable");
    extract_bundle(&bundle, &fresh_app)?;
    seed(
        &data.join(format!("{LIBRARY}-2024-01-01")),
        "Fallback Backup",
    )?;
    fs::write(data.join(format!("{LIBRARY}-journal")), b"busy fixture")?;
    with_read_only_sources(&[&data], || {
        launch(&fresh_app, &live_config, "Fallback Backup", false)
    })?;

    let snapshot_config = json!({"plex_epg_db_source": guide,
        "plex_library_db_source": data.join("absent-live-source.db"), "library_snapshot_dir": backups});
    seed(
        &backups.join(format!("{LIBRARY}-2024-01-02")),
        "Saved Backup",
    )?;
    let mirror = app.join(MIRROR);
    let saved = with_read_only_sources(&[&data, &backups], || {
        launch(&app, &snapshot_config, "Saved Backup", false)?;
        let stamp = mirror.metadata()?.modified()?;
        let content = fs::read(&mirror)?;
        launch(&app, &snapshot_config, "Saved Backup", false)?;
        assert_eq!(mirror.metadata()?.modified()?, stamp);
        assert!(
            fs::read(&mirror)? == content,
            "same snapshot replaced the mirror"
        );
        Ok(content)
    })?;
    seed(
        &backups.join(format!("{LIBRARY}-2024-01-03")),
        "Newer Backup",
    )?;
    with_read_only_sources(&[&data, &backups], || {
        launch(&app, &snapshot_config, "Newer Backup", false)
    })?;
    assert!(
        fs::read(mirror)? != saved,
        "new snapshot did not replace the mirror"
    );
    Ok(())
}
