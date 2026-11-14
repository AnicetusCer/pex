use super::super::types::OwnedDone;
use crate::config::{local_library_db_path, AppConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

const CACHE_FILE: &str = "owned_titles_v4.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::app) enum LibrarySource {
    Live(PathBuf),
    Snapshots(PathBuf),
    Manual(PathBuf),
}

impl LibrarySource {
    pub(in crate::app) fn configured(cfg: &AppConfig) -> Option<Self> {
        if cfg.guide_only {
            return None;
        }
        Some(if let Some(dir) = &cfg.library_snapshot_dir {
            Self::Snapshots(dir.clone())
        } else if let Some(path) = &cfg.plex_library_db_source {
            Self::Live(path.clone())
        } else {
            Self::Manual(local_library_db_path())
        })
    }
}

#[derive(Serialize, Deserialize)]
struct TitleCache<S, K> {
    source: S,
    keys: K,
    hd_keys: K,
}

pub(in crate::app) fn preload(
    cache_dir: &Path,
    source: Option<&LibrarySource>,
) -> Option<(HashSet<String>, HashSet<String>)> {
    // Manual replacements and snapshot generations must be checked by the scan.
    let source @ LibrarySource::Live(_) = source? else {
        return None;
    };
    let bytes = std::fs::read(cache_dir.join(CACHE_FILE)).ok()?;
    let cache: TitleCache<LibrarySource, HashSet<String>> = serde_json::from_slice(&bytes).ok()?;
    (cache.source == *source).then_some((cache.keys, cache.hd_keys))
}

pub(super) fn save(cache_dir: &Path, source: &LibrarySource, scan: &OwnedDone) -> io::Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(cache_dir)?;
    let mut writer = BufWriter::new(temp.as_file_mut());
    serde_json::to_writer(
        &mut writer,
        &TitleCache {
            source,
            keys: &scan.keys,
            hd_keys: &scan.hd_keys,
        },
    )?;
    writer.flush()?;
    drop(writer);
    temp.as_file().sync_all()?;
    temp.persist(cache_dir.join(CACHE_FILE))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_complete_cache_for_the_same_live_source_can_preload() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("library.db");
        let live = LibrarySource::Live(path.clone());
        let other = LibrarySource::Live(dir.join("other/library.db"));
        let mut scan = OwnedDone::default();
        scan.keys.insert("example:2024".into());
        scan.hd_keys.clone_from(&scan.keys);
        std::fs::write(dir.join("owned_all_v3.txt"), b"legacy:2024").unwrap();
        assert!(preload(dir, Some(&live)).is_none());
        save(dir, &live, &scan).unwrap();
        let (all, hd) = preload(dir, Some(&live)).unwrap();
        assert_eq!(all, scan.keys);
        assert_eq!(hd, scan.hd_keys);
        assert!(preload(dir, Some(&other)).is_none());
        assert!(preload(dir, None).is_none());
        for mode in [
            LibrarySource::Manual(path.clone()),
            LibrarySource::Snapshots(path),
        ] {
            assert!(preload(dir, Some(&mode)).is_none());
            save(dir, &mode, &scan).unwrap();
            assert!(preload(dir, Some(&live)).is_none());
            assert!(preload(dir, Some(&mode)).is_none());
        }
        std::fs::write(dir.join(CACHE_FILE), b"{").unwrap();
        assert!(preload(dir, Some(&live)).is_none());
    }
}
