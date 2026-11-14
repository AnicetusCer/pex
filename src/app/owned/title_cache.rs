use super::super::types::{OwnedLibrary, OwnedQuality};
use crate::config::{local_library_db_path, AppConfig};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

const CACHE_FILE: &str = "owned_titles_v5.json";
const LEGACY_FILE: &str = "owned_titles_v4.json";

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
    titles: K,
}

#[derive(Deserialize)]
struct LegacyCache {
    source: LibrarySource,
    keys: HashSet<String>,
    hd_keys: HashSet<String>,
}

// Write directly from the scan index. On a large live library, collecting
// these borrowed keys into a second hash table would still cost many megabytes.
struct CachedTitles<'a>(Option<&'a OwnedLibrary>);

impl Serialize for CachedTitles<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map =
            serializer.serialize_map(Some(self.0.map_or(0, |scan| scan.by_title.len())))?;
        if let Some(scan) = self.0 {
            for (key, entries) in &scan.by_title {
                map.serialize_entry(key, &scan.quality(entries.as_slice()))?;
            }
        }
        map.end()
    }
}

pub(in crate::app) fn preload(
    cache_dir: &Path,
    source: Option<&LibrarySource>,
) -> Option<HashMap<String, OwnedQuality>> {
    // Manual replacements and snapshot generations must be checked by the scan.
    let source @ LibrarySource::Live(_) = source? else {
        return None;
    };
    match std::fs::read(cache_dir.join(CACHE_FILE)) {
        Ok(bytes) => {
            let cache: TitleCache<LibrarySource, HashMap<String, OwnedQuality>> =
                serde_json::from_slice(&bytes).ok()?;
            (cache.source == *source).then_some(cache.titles)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let bytes = std::fs::read(cache_dir.join(LEGACY_FILE)).ok()?;
            let cache: LegacyCache = serde_json::from_slice(&bytes).ok()?;
            if cache.source != *source {
                return None;
            }
            Some(
                cache
                    .keys
                    .into_iter()
                    .map(|key| {
                        let quality = if cache.hd_keys.contains(&key) {
                            OwnedQuality::Hd
                        } else {
                            OwnedQuality::Unknown
                        };
                        (key, quality)
                    })
                    .collect(),
            )
        }
        Err(_) => None,
    }
}

pub(super) fn save(
    cache_dir: &Path,
    source: &LibrarySource,
    scan: &OwnedLibrary,
) -> io::Result<()> {
    // A non-live scan only has to invalidate an earlier live generation.
    let titles = CachedTitles(matches!(source, LibrarySource::Live(_)).then_some(scan));
    let mut temp = tempfile::NamedTempFile::new_in(cache_dir)?;
    let mut writer = BufWriter::new(temp.as_file_mut());
    serde_json::to_writer(&mut writer, &TitleCache { source, titles })?;
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
        let scan = super::super::owned_scan_plex::tests::library(&[("Example", Some(2024))]);
        std::fs::write(dir.join("owned_all_v3.txt"), b"legacy:2024").unwrap();
        assert!(preload(dir, Some(&live)).is_none());
        let legacy = serde_json::json!({"source":&live, "keys":["example:2024","unmeasured:2020"], "hd_keys":["example:2024"]});
        std::fs::write(dir.join(LEGACY_FILE), serde_json::to_vec(&legacy).unwrap()).unwrap();
        let old = preload(dir, Some(&live)).unwrap();
        assert_eq!(old["example:2024"], OwnedQuality::Hd);
        assert_eq!(old["unmeasured:2020"], OwnedQuality::Unknown);
        save(dir, &live, &scan).unwrap();
        assert_eq!(
            preload(dir, Some(&live)).unwrap()["example:2024"],
            OwnedQuality::Hd
        );
        assert!(preload(dir, Some(&other)).is_none());
        assert!(preload(dir, None).is_none());
        for mode in [
            LibrarySource::Manual(path.clone()),
            LibrarySource::Snapshots(path),
        ] {
            assert!(preload(dir, Some(&mode)).is_none());
            save(dir, &mode, &scan).unwrap();
            let marker: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join(CACHE_FILE)).unwrap()).unwrap();
            assert!(marker["titles"].as_object().unwrap().is_empty());
            assert!(preload(dir, Some(&live)).is_none());
            assert!(preload(dir, Some(&mode)).is_none());
        }
        std::fs::write(dir.join(CACHE_FILE), b"{").unwrap();
        assert!(preload(dir, Some(&live)).is_none());
    }
}
