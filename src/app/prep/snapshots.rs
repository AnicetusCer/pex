use super::{
    copy_validated_snapshot, last_sync_marker_path, provenance_path, source_state, usable_library,
    FileState,
};
use chrono::{NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{fs, io};

const EXPORT_TIME: &str = "%Y-%m-%dT%H%M%S%6fZ";

struct Generation {
    // Plex scheduled backups supply only a calendar date. On the same date, prefer a
    // timestamped export. Do not invent a time of day in the status message.
    order: NaiveDateTime,
    description: String,
    path: PathBuf,
}

fn generation(path: PathBuf) -> Option<Generation> {
    let name = path.file_name()?.to_str()?;
    let (order, description) = if let Some(date) =
        name.strip_prefix("com.plexapp.plugins.library.db-")
    {
        let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        if parsed.format("%Y-%m-%d").to_string() != date
            || parsed > chrono::Local::now().date_naive()
        {
            return None;
        }
        (
            parsed.and_hms_opt(0, 0, 0)?,
            format!("Plex backup dated {date}"),
        )
    } else {
        let timestamp = name.strip_prefix("pex-library-")?.strip_suffix(".db")?;
        let parsed = NaiveDateTime::parse_from_str(timestamp, EXPORT_TIME).ok()?;
        if parsed.format(EXPORT_TIME).to_string() != timestamp || parsed > Utc::now().naive_utc() {
            return None;
        }
        (
            parsed,
            format!(
                "PEX export from {} UTC",
                parsed.format("%Y-%m-%d %H:%M:%S%.6f")
            ),
        )
    };
    Some(Generation {
        order,
        description,
        path,
    })
}

fn candidates(folder: &Path) -> io::Result<Vec<Generation>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            if let Some(candidate) = generation(entry.path()) {
                found.push(candidate);
            }
        }
    }
    found.sort_by(|a, b| b.order.cmp(&a.order).then_with(|| b.path.cmp(&a.path)));
    Ok(found)
}

// The receipt binds an immutable generation to this particular local installation.
// The path alone cannot detect a replaced same-date backup, or a manually replaced mirror.
#[derive(Deserialize, Serialize)]
struct Receipt {
    source: PathBuf,
    source_state: FileState,
    mirror_state: (FileState, Option<FileState>),
}

impl Receipt {
    fn load(dst: &Path, folder: &Path) -> Option<Self> {
        let record: Self = serde_json::from_slice(&fs::read(provenance_path(dst)).ok()?).ok()?;
        if record.source.parent() == Some(folder)
            && record.matches_local(dst)
            && usable_library(dst)
        {
            Some(record)
        } else {
            None
        }
    }

    fn matches_local(&self, dst: &Path) -> bool {
        source_state(dst).ok().as_ref() == Some(&self.mirror_state)
    }

    fn save(&self, dst: &Path) -> io::Result<()> {
        let path = provenance_path(dst);
        let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        serde_json::to_writer(tmp.as_file_mut(), self)?;
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

fn clear_live_marker(dst: &Path) -> io::Result<()> {
    match fs::remove_file(last_sync_marker_path(dst)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn refresh(
    folder: &Path,
    dst: &Path,
    report: impl Fn(String),
) -> Result<String, String> {
    let previous = Receipt::load(dst, folder);
    let installed = previous.as_ref().and_then(|r| generation(r.source.clone()));
    let mut rejected = Vec::new();
    let found = candidates(folder).unwrap_or_else(|e| {
        rejected.push(format!("Cannot list {} ({e}).", folder.display()));
        Vec::new()
    });
    for file in found {
        let path = &file.path;
        if let (Some(installed), Some(record)) = (&installed, &previous) {
            if file.order < installed.order && record.matches_local(dst) {
                break;
            }
        }
        let attempt = (|| -> io::Result<&str> {
            let (state, wal) = source_state(path)?;
            if wal.is_some_and(|s| s.len != 0) {
                return Err(io::Error::other("backup still has an active WAL"));
            }
            if previous.as_ref().is_some_and(|r| {
                r.source == *path && r.source_state == state && r.matches_local(dst)
            }) {
                return Ok("Reused local copy");
            }
            report(format!("Copying {}...", path.display()));
            // A completed snapshot must never satisfy the live source's daily freshness check.
            clear_live_marker(dst)?;
            let copied = copy_validated_snapshot(path, dst, true, true)?;
            let saved = source_state(dst).and_then(|mirror_state| {
                Receipt {
                    source: path.clone(),
                    source_state: copied,
                    mirror_state,
                }
                .save(dst)
            });
            if let Err(e) = saved {
                report(format!(
                    "Could not record the installed snapshot in {}: {e}",
                    provenance_path(dst).display()
                ));
                return Ok("Copied snapshot (could not save its receipt for reuse)");
            }
            Ok("Copied snapshot")
        })();
        match attempt {
            Ok(action) => {
                let notice = rejected
                    .first()
                    .map(|s| format!(" Skipped a newer candidate: {s}"))
                    .unwrap_or_default();
                return Ok(format!(
                    "{action}: {} ({}). Library-based badges reflect this snapshot.{notice}",
                    file.description,
                    path.display()
                ));
            }
            Err(e) => rejected.push(format!("{} ({e})", path.display())),
        }
    }
    if let (Some(installed), Some(record)) = (installed, previous) {
        if record.matches_local(dst) {
            let reason = rejected.first().cloned().unwrap_or_else(|| {
                format!(
                    "No newer completed snapshot is available in {}.",
                    folder.display()
                )
            });
            return Ok(format!("{reason} Keeping the local {} ({}). Choose Refresh owned scan after a new snapshot is available.", installed.description, installed.path.display()));
        }
    }
    let reason = rejected
        .first()
        .cloned()
        .unwrap_or_else(|| format!("No completed snapshots found in {}.", folder.display()));
    Err(format!("{reason} Choose a folder containing an existing completed copy in Connect to Plex, or select Live library and retry after the recording finishes."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::prep::recover_library;
    use crate::app::test_fixtures::database;
    use rusqlite::Connection;

    const BACKUP: &str = "com.plexapp.plugins.library.db-2020-03-25";
    const EXPORT: &str = "pex-library-2020-03-25T134530123456Z.db";

    fn fixture(folder: &Path, name: &str, title: &str) -> PathBuf {
        let path = folder.join(name);
        database(&path);
        set_title(&path, title);
        path
    }

    fn set_title(path: &Path, title: &str) {
        Connection::open(path)
            .unwrap()
            .execute("UPDATE metadata_items SET title=?1", [title])
            .unwrap();
    }

    fn title(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT title FROM metadata_items", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn only_completed_dated_regular_files_are_candidates() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            EXPORT,
            BACKUP,
            "com.plexapp.plugins.library.db",
            "com.plexapp.plugins.library.db-2020-02-30",
            "pex-library-2020-03-25T134530Z.db",
            "pex-library-2020-03-25T134530123456Z.db.tmp",
            "pex-library-2020-03-25T134530123456Z.db-wal",
            "pex-library-2999-03-25T134530123456Z.db",
            "com.plexapp.plugins.library.db-2999-03-25",
        ] {
            fs::write(root.path().join(name), []).unwrap();
        }
        fs::create_dir(
            root.path()
                .join("com.plexapp.plugins.library.db-2020-03-26"),
        )
        .unwrap();
        let found = candidates(root.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].path, root.path().join(EXPORT));
        assert_eq!(found[1].path, root.path().join(BACKUP));
    }

    #[test]
    fn unchanged_generation_avoids_transfer_and_replacements_get_checked() {
        let root = tempfile::tempdir().unwrap();
        let backup = fixture(root.path(), BACKUP, "First backup");
        let dst = root.path().join("local.db");
        let notice = refresh(root.path(), &dst, |_| {}).unwrap();
        assert!(notice.contains("2020-03-25") && notice.contains("Copied"));
        refresh(root.path(), &dst, |_| {
            panic!("unchanged generation was recopied")
        })
        .unwrap();
        set_title(&backup, "Replaced same dated generation");
        refresh(root.path(), &dst, |_| {}).unwrap();
        assert_eq!(title(&dst), "Replaced same dated generation");
        set_title(&dst, "Local replacement");
        refresh(root.path(), &dst, |_| {}).unwrap();
        assert_eq!(title(&dst), "Replaced same dated generation");
    }

    #[test]
    fn uses_newest_valid_then_keeps_it_if_the_folder_lags_or_is_offline() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("snapshots");
        fs::create_dir(&folder).unwrap();
        fixture(&folder, BACKUP, "Plex");
        let export = fixture(&folder, EXPORT, "Exporter");
        let active = fixture(
            &folder,
            "com.plexapp.plugins.library.db-2020-03-26",
            "Unfinished",
        );
        fs::write(super::super::sqlite_sidecar_path(&active, "wal"), b"active").unwrap();
        fs::write(
            folder.join("com.plexapp.plugins.library.db-2020-03-27"),
            b"corrupt",
        )
        .unwrap();
        let dst = root.path().join("library.db");
        let notice = refresh(&folder, &dst, |_| {}).unwrap();
        assert!(notice.contains("2020-03-25 13:45:30.123456 UTC") && notice.contains("Skipped"));
        assert_eq!(title(&dst), "Exporter");
        fs::remove_file(export).unwrap();
        assert!(refresh(&folder, &dst, |_| {}).unwrap().contains("Keeping"));
        assert_eq!(title(&dst), "Exporter");
        fs::rename(&folder, root.path().join("offline")).unwrap();
        assert!(refresh(&folder, &dst, |_| panic!("offline copy"))
            .unwrap()
            .contains("Keeping"));
        assert!(refresh(&root.path().join("other"), &dst, |_| {}).is_err());
    }

    #[test]
    fn switching_modes_does_not_treat_another_sources_snapshot_as_live() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("published");
        fs::create_dir(&folder).unwrap();
        fixture(&folder, BACKUP, "From snapshot");
        let live = fixture(root.path(), "source.db", "From live");
        let dst = root.path().join("local.db");
        recover_library(&live, &dst, true, |_| {}).unwrap();
        let notice = refresh(&folder, &dst, |_| {}).unwrap();
        assert!(notice.contains(BACKUP));
        assert!(!last_sync_marker_path(&dst).exists());
        fs::write(last_sync_marker_path(&dst), b"stale").unwrap();
        let missing = root.path().join("different-source.db");
        assert!(recover_library(&missing, &dst, false, |_| {}).is_err());
        assert_eq!(title(&dst), "From snapshot");
        recover_library(&live, &dst, false, |_| {}).unwrap();
        assert_eq!(title(&dst), "From live");
        assert!(recover_library(&missing, &dst, false, |_| {}).is_err());
        assert!(refresh(&root.path().join("missing"), &dst, |_| {}).is_err());
    }

    #[test]
    fn receipt_write_failure_preserves_newly_installed_generation() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), EXPORT, "Newest");
        fixture(root.path(), BACKUP, "Older");
        let dst = root.path().join("mirror.db");
        fs::create_dir(provenance_path(&dst)).unwrap();
        let result = refresh(root.path(), &dst, |_| {}).unwrap();
        assert!(result.contains("could not save") && result.contains(EXPORT));
        assert_eq!(title(&dst), "Newest");
    }
}
