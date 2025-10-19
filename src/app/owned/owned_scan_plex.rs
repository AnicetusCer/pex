use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use tracing::warn;

use super::owned_scan_fs::{persist_owned_hd_sidecar, persist_owned_keys_sidecar};
use crate::app::cache;
use crate::app::types::OwnedMsg;
use crate::app::PexApp;
use crate::config::local_library_db_path;

pub struct OwnedScanPlex;

impl OwnedScanPlex {
    pub(crate) fn spawn_scan(tx: Sender<OwnedMsg>, library_roots: Vec<PathBuf>) {
        thread::spawn(move || {
            use OwnedMsg::{Done, Error, Info};

            let _ = tx.send(Info(
                "Stage 3/4 - Loading owned titles from Plex library database.".into(),
            ));

            let db_path = local_library_db_path();
            let timeout = Duration::from_secs(60);
            let start = Instant::now();
            let mut wait_logged = false;

            let conn = loop {
                if db_path.exists() {
                    match Connection::open_with_flags(
                        &db_path,
                        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    ) {
                        Ok(conn) => {
                            let _ = conn.busy_timeout(Duration::from_secs(5));
                            break conn;
                        }
                        Err(err) => {
                            if start.elapsed() >= timeout {
                                let _ = tx.send(Error(format!(
                                    "Failed to open Plex library DB {}: {err}",
                                    db_path.display()
                                )));
                                return;
                            }
                        }
                    }
                } else if start.elapsed() >= timeout {
                    let _ = tx.send(Error(format!(
                        "Plex library DB not found at {} after waiting {}s. Run a sync or configure `plex_library_db_source`.",
                        db_path.display(),
                        timeout.as_secs()
                    )));
                    return;
                }

                if !wait_logged {
                    let _ = tx.send(Info(format!(
                        "Waiting for Plex library DB to become available at {}...",
                        db_path.display()
                    )));
                    wait_logged = true;
                }
                std::thread::sleep(Duration::from_millis(300));
            };

            match collect_plex_owned_entries(&conn, &library_roots) {
                Ok((entries, any_root_match)) => {
                    if !library_roots.is_empty() && !any_root_match {
                        let _ = tx.send(Info(
                            "Configured library_roots did not match any Plex library files; returning all movies.".into(),
                        ));
                    }

                    let mut owned: HashSet<String> = HashSet::new();
                    let mut hd_keys: HashSet<String> = HashSet::new();
                    let mut owned_dates: HashMap<String, Option<u64>> = HashMap::new();

                    for entry in entries {
                        accumulate_owned_entry(&entry, &mut owned, &mut hd_keys, &mut owned_dates);
                    }

                    let cache_dir = cache::cache_dir();
                    if let Err(err) = persist_owned_keys_sidecar(&cache_dir, &owned) {
                        warn!("Failed to persist owned sidecar: {err}");
                    }
                    if let Err(err) = persist_owned_hd_sidecar(&cache_dir, &hd_keys) {
                        warn!("Failed to persist owned HD sidecar: {err}");
                    }

                    let count = owned.len();
                    let _ = tx.send(Info(format!(
                        "Stage 3/4 - Plex library owned scan complete ({count} keys)."
                    )));
                    let _ = tx.send(Done {
                        keys: owned,
                        modified: owned_dates,
                    });
                }
                Err(err) => {
                    let _ = tx.send(Error(err));
                }
            }
        });
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PlexOwnedEntry {
    metadata_id: i64,
    guid: Option<String>,
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    updated_at: Option<u64>,
}

fn collect_plex_owned_entries(
    conn: &Connection,
    library_roots: &[PathBuf],
) -> Result<(Vec<PlexOwnedEntry>, bool), String> {
    let sql = r#"
        SELECT
            m.id            AS metadata_id,
            m.guid          AS guid,
            m.title         AS title,
            m.original_title AS original_title,
            m.year          AS year,
            m.updated_at    AS meta_updated_at,
            m.added_at      AS meta_added_at,
            mi.id           AS media_item_id,
            mi.width        AS width,
            mi.height       AS height,
            mi.updated_at   AS media_updated_at,
            mp.id           AS media_part_id,
            mp.file         AS file_path,
            mp.size         AS file_size,
            mp.updated_at   AS part_updated_at
        FROM metadata_items m
        JOIN media_items mi ON mi.metadata_item_id = m.id
        JOIN media_parts mp ON mp.media_item_id = mi.id
        WHERE m.metadata_type = 1
          AND mp.file IS NOT NULL
          AND mp.file <> ''
        ORDER BY
            m.id ASC,
            COALESCE(mi.width, 0) DESC,
            COALESCE(mi.height, 0) DESC,
            COALESCE(mp.size, 0) DESC
    "#;

    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| format!("Failed to prepare Plex library query: {err}"))?;

    let mut seen_ids: HashSet<i64> = HashSet::new();
    let normalized_roots: Vec<PathBuf> = library_roots
        .iter()
        .filter(|root| !root.as_os_str().is_empty())
        .cloned()
        .collect();

    let rows = stmt
        .query_map([], |row| {
            let metadata_id: i64 = row.get("metadata_id")?;
            let guid: Option<String> = row.get("guid")?;
            let title: String = row.get("title")?;
            let original_title: Option<String> = row.get("original_title")?;
            let year: Option<i32> = row.get("year")?;
            let width: Option<i64> = row.get("width")?;
            let height: Option<i64> = row.get("height")?;
            let part_updated_at: Option<i64> = row.get("part_updated_at")?;
            let media_updated_at: Option<i64> = row.get("media_updated_at")?;
            let meta_updated_at: Option<i64> = row.get("meta_updated_at")?;
            let meta_added_at: Option<i64> = row.get("meta_added_at")?;
            let file: String = row.get("file_path")?;
            let size: Option<i64> = row.get("file_size")?;

            Ok((
                metadata_id,
                guid,
                title,
                original_title,
                year,
                width,
                height,
                part_updated_at,
                media_updated_at,
                meta_updated_at,
                meta_added_at,
                file,
                size,
            ))
        })
        .map_err(|err| format!("Failed to iterate Plex library rows: {err}"))?;

    let mut results: Vec<PlexOwnedEntry> = Vec::new();
    let mut any_root_match = false;

    for row in rows {
        let (
            metadata_id,
            guid,
            title,
            original_title,
            year,
            width,
            height,
            part_updated_at,
            media_updated_at,
            meta_updated_at,
            meta_added_at,
            file,
            _size,
        ) = row.map_err(|err| format!("Failed to read Plex library row: {err}"))?;

        if !seen_ids.insert(metadata_id) {
            continue;
        }

        if title.trim().is_empty() {
            continue;
        }

        let path = PathBuf::from(file);
        if !normalized_roots.is_empty() && path_matches_any_root(&path, &normalized_roots) {
            any_root_match = true;
        }

        let width = width.map(|v| v.max(0) as u32);
        let height = height.map(|v| v.max(0) as u32);
        let updated_at = part_updated_at
            .or(media_updated_at)
            .or(meta_updated_at)
            .or(meta_added_at)
            .map(|ts| ts.max(0) as u64);

        results.push(PlexOwnedEntry {
            metadata_id,
            guid,
            title,
            original_title,
            year,
            width,
            height,
            updated_at,
        });
    }

    Ok((results, any_root_match))
}

fn accumulate_owned_entry(
    entry: &PlexOwnedEntry,
    owned: &mut HashSet<String>,
    hd_keys: &mut HashSet<String>,
    owned_dates: &mut HashMap<String, Option<u64>>,
) {
    let hd = is_hd(entry.width, entry.height);
    let mut inserted_keys: HashSet<String> = HashSet::new();

    let mut insert_key = |key: String| {
        if inserted_keys.insert(key.clone()) {
            owned.insert(key.clone());
            if hd {
                hd_keys.insert(key.clone());
            }
            owned_dates.insert(key, entry.updated_at);
        }
    };

    let mut push_keys_for = |title: &str| {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return;
        }
        for key in PexApp::owned_key_variants(trimmed, entry.year) {
            insert_key(key);
        }
    };

    push_keys_for(&entry.title);
    if let Some(original) = entry.original_title.as_deref() {
        push_keys_for(original);
    }
}

fn path_matches_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn is_hd(width: Option<u32>, height: Option<u32>) -> bool {
    width.map(|w| w >= 1280).unwrap_or(false) || height.map(|h| h >= 720).unwrap_or(false)
}
