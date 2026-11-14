use super::{last_sync_marker_path, provenance_path, sqlite_sidecar_path};
use std::path::{Path, PathBuf};
use std::{fs, io};

// Compare filesystem identities as well as paths: junctions, mapped drives and
// UNC spellings can all reach the same Windows file.
#[cfg(windows)]
fn identity(path: &Path) -> io::Result<(u64, u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    // Request file attributes only, including on directories. The source is
    // never opened for writing and no SQLite handle is involved.
    let file = fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
        .open(path)?;
    let info = winapi_util::file::information(&file)?;
    Ok((
        info.volume_serial_number(),
        info.file_index(),
        info.number_of_links(),
    ))
}

#[cfg(unix)]
fn identity(path: &Path) -> io::Result<(u64, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let info = fs::metadata(path)?;
    Ok((info.dev(), info.ino(), info.nlink()))
}

fn database_files(db: &Path) -> [PathBuf; 4] {
    [
        db.into(),
        sqlite_sidecar_path(db, "wal"),
        sqlite_sidecar_path(db, "shm"),
        sqlite_sidecar_path(db, "journal"),
    ]
}

fn conflict(path: &Path) -> io::Error {
    io::Error::other(format!(
        "Unsafe local mirror at {}. Keep a separate, unlinked database copy in PEX's db folder and select the original Plex source in Connect to Plex.",
        path.display()
    ))
}

fn same_name(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.file_name().zip(b.file_name()).is_some_and(|(a, b)| {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        })
    }
    #[cfg(unix)]
    {
        a.file_name() == b.file_name()
    }
}

fn same_id(a: (u64, u64, u64), b: (u64, u64, u64)) -> bool {
    a.1 != 0 && (a.0, a.1) == (b.0, b.1)
}

fn same_parent(a: &Path, b: &Path) -> bool {
    let (Some(a), Some(b)) = (a.parent(), b.parent()) else {
        return false;
    };
    a == b
        || identity(a)
            .ok()
            .zip(identity(b).ok())
            .is_some_and(|(a, b)| same_id(a, b))
        || fs::canonicalize(a)
            .ok()
            .zip(fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}

/// Call before considering a cached mirror or a backup. Reject linked outputs
/// even when the configured source is offline or its name has changed.
pub(super) fn check<'a>(dst: &Path, sources: impl IntoIterator<Item = &'a Path>) -> io::Result<()> {
    let paths = database_files(dst)
        .into_iter()
        .chain([last_sync_marker_path(dst), provenance_path(dst)]);
    let mut outputs = Vec::new();
    for path in paths {
        let id = match fs::symlink_metadata(&path) {
            Ok(info) if info.file_type().is_symlink() => return Err(conflict(&path)),
            Ok(info) if info.is_file() => {
                let id = identity(&path)?;
                if id.2 > 1 {
                    return Err(conflict(&path));
                }
                Some(id)
            }
            Ok(_) => None,
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        };
        outputs.push((path, id));
    }
    for source in sources {
        let inputs = database_files(source);
        // A configured source may itself be missing. Refuse to create it from a
        // backup even if the two parent paths have different spellings.
        if inputs
            .iter()
            .any(|src| outputs.iter().any(|(out, _)| same_name(src, out)))
            && same_parent(source, dst)
        {
            return Err(conflict(dst));
        }
        if outputs.iter().all(|(_, id)| id.is_none()) {
            continue;
        }
        for src in inputs {
            // Unavailable remote sources still allow a safe local copy to be used.
            let Ok(src_id) = identity(&src) else { continue };
            for (out, id) in &outputs {
                if let Some(id) = id {
                    if same_id(src_id, *id)
                        || (src_id.1 == 0
                            && fs::canonicalize(&src)
                                .ok()
                                .zip(fs::canonicalize(out).ok())
                                .is_some_and(|(a, b)| a == b))
                    {
                        return Err(conflict(out));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{prep::recover_library, test_fixtures::database};

    #[test]
    fn alias_is_terminal_before_opening_cached_mirror_or_falling_back_to_backup() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("com.plexapp.plugins.library.db");
        let backup = dir.path().join("com.plexapp.plugins.library.db-2020-01-01");
        database(&src);
        database(&backup);
        rusqlite::Connection::open(&backup)
            .unwrap()
            .execute("UPDATE metadata_items SET title='Older backup'", [])
            .unwrap();
        let live_before = fs::read(&src).unwrap();
        let backup_before = fs::read(&backup).unwrap();
        assert_ne!(live_before, backup_before);
        let journal = sqlite_sidecar_path(&src, "journal");
        fs::write(&journal, b"recording").unwrap();
        let alias = dir.path().join("plex_library.db");
        for dst in [&src, &alias] {
            if dst == &alias {
                fs::hard_link(&src, &alias).unwrap();
            }
            // A previous mirror from a different source would normally trigger the
            // dated-backup fallback even if the live file itself looked readable.
            let receipt = provenance_path(dst);
            fs::write(&receipt, br#"{"source":"different"}"#).unwrap();
            let marker = last_sync_marker_path(dst);
            fs::write(&marker, b"fresh").unwrap();
            for force in [true, false] {
                let error = recover_library(&src, dst, force, |_| panic!("attempted recovery"))
                    .unwrap_err();
                assert!(error.contains("Unsafe local mirror"), "{error}");
                assert_eq!(fs::read(&src).unwrap(), live_before);
                assert_eq!(fs::read(&backup).unwrap(), backup_before);
                assert_eq!(fs::read(&journal).unwrap(), b"recording");
                assert_eq!(fs::read(&marker).unwrap(), b"fresh");
                assert_eq!(fs::read(&receipt).unwrap(), br#"{"source":"different"}"#);
            }
        }
    }

    #[test]
    fn unrelated_files_in_same_folder_are_allowed_but_linked_sidecars_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live.db");
        let dst = dir.path().join("mirror.db");
        database(&live);
        database(&dst);
        assert!(check(&dst, [&*live]).is_ok());
        for path in [
            sqlite_sidecar_path(&dst, "wal"),
            last_sync_marker_path(&dst),
            provenance_path(&dst),
        ] {
            fs::hard_link(&live, &path).unwrap();
            assert!(check(&dst, [&*live]).is_err());
            fs::remove_file(path).unwrap();
        }
    }
}
