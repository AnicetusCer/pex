use serde::Deserialize;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{info, warn};

pub const LOCAL_DB_DIR: &str = "db";
pub const LOCAL_EPG_DB_FILE: &str = "plex_epg.db";
pub const LOCAL_LIBRARY_DB_FILE: &str = "plex_library.db";
pub const CONFIG_FILENAME: &str = "pex-config.json";
pub const HOME_CONFIG_FILENAME: &str = ".pex-config.json";
const LEGACY_CONFIG_FILENAME: &str = "config.json";

static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub guide_only: bool,
    pub log_level: Option<String>,
    pub cache_dir: Option<PathBuf>,
    pub plex_epg_db_source: Option<PathBuf>,
    pub plex_library_db_source: Option<PathBuf>,
    pub library_snapshot_dir: Option<PathBuf>,
    pub tmdb_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    guide_only: bool,
    log_level: Option<String>,
    cache_dir: Option<String>,
    #[serde(alias = "plex_db_source")]
    plex_epg_db_source: Option<String>,
    plex_library_db_source: Option<String>,
    library_snapshot_dir: Option<String>,
    #[serde(alias = "omdb_api_key")]
    #[serde(alias = "the_movie_db_api_key")]
    tmdb_api_key: Option<String>,
}

pub fn base_dir() -> &'static Path {
    BASE_DIR.get_or_init(determine_base_dir).as_path()
}

fn determine_base_dir() -> PathBuf {
    if let Ok(override_dir) = env::var("PEX_BASE_DIR") {
        let path = PathBuf::from(override_dir);
        if path.is_absolute() {
            return path;
        }
        if let Ok(cwd) = env::current_dir() {
            return cwd.join(path);
        }
    }

    if let Ok(exe_path) = env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            return parent.to_path_buf();
        }
    }

    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn resolve_relative_path<P: AsRef<Path>>(input: P) -> PathBuf {
    let p = input.as_ref();
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir().join(p)
    }
}

fn read_config_source() -> Option<(PathBuf, String)> {
    for path in config_candidates() {
        match fs::read_to_string(&path) {
            Ok(raw) => return Some((path, raw)),
            Err(err) => {
                if err.kind() == ErrorKind::NotFound {
                    continue;
                }
                warn!("Failed to read {}: {err}", path.display());
            }
        }
    }

    None
}

fn push_unique(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.contains(&path) {
        candidates.push(path);
    }
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

fn config_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(custom) = env::var("PEX_CONFIG") {
        let candidate = PathBuf::from(custom);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            resolve_relative_path(candidate)
        };
        push_unique(&mut candidates, candidate);
    }

    // Preferred defaults: executable/base dir, then cwd, then home.
    push_unique(&mut candidates, base_dir().join(CONFIG_FILENAME));

    if let Ok(cwd) = env::current_dir() {
        push_unique(&mut candidates, cwd.join(CONFIG_FILENAME));
    }

    if let Some(home) = home_dir() {
        push_unique(&mut candidates, home.join(HOME_CONFIG_FILENAME));
    }

    // Backward compatibility for existing setups.
    push_unique(&mut candidates, base_dir().join(LEGACY_CONFIG_FILENAME));
    if let Ok(cwd) = env::current_dir() {
        push_unique(&mut candidates, cwd.join(LEGACY_CONFIG_FILENAME));
    }

    candidates
}

pub fn discover_config_path() -> Option<PathBuf> {
    config_candidates().into_iter().find(|p| p.exists())
}

pub fn load_config() -> AppConfig {
    let mut cfg = AppConfig::default();

    if let Some((path, raw)) = read_config_source() {
        match serde_json::from_str::<RawConfig>(&raw) {
            Ok(mut parsed) => {
                cfg.guide_only = parsed.guide_only;
                cfg.library_snapshot_dir = parsed
                    .library_snapshot_dir
                    .as_deref()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(resolve_relative_path);
                cfg.log_level = parsed.log_level;
                if let Some(cache_dir) = parsed.cache_dir.take() {
                    let trimmed = cache_dir.trim();
                    if !trimmed.is_empty() {
                        cfg.cache_dir = Some(resolve_relative_path(trimmed));
                    }
                }

                if let Some(epg) = parsed.plex_epg_db_source.take() {
                    let trimmed = epg.trim();
                    if !trimmed.is_empty() {
                        cfg.plex_epg_db_source = Some(resolve_relative_path(trimmed));
                    }
                    if raw.contains("\"plex_db_source\"") {
                        warn!(
                            "`plex_db_source` is deprecated; rename it to `plex_epg_db_source` in {CONFIG_FILENAME}."
                        );
                    }
                }

                if let Some(library) = parsed.plex_library_db_source.take() {
                    let trimmed = library.trim();
                    if !trimmed.is_empty() {
                        cfg.plex_library_db_source = Some(resolve_relative_path(trimmed));
                    }
                }

                if let Some(api_key) = parsed.tmdb_api_key.take() {
                    cfg.tmdb_api_key = Some(api_key);
                    if raw.contains("\"omdb_api_key\"") {
                        warn!(
                            "`omdb_api_key` is deprecated; rename it to `tmdb_api_key` in {CONFIG_FILENAME}."
                        );
                    }
                    if raw.contains("\"the_movie_db_api_key\"") {
                        warn!(
                            "`the_movie_db_api_key` is deprecated; rename it to `tmdb_api_key` in {CONFIG_FILENAME}."
                        );
                    }
                }

                info!("Loaded config from {}", path.display());
            }
            Err(err) => {
                warn!(
                    "Failed to parse {} ({err}). Using defaults.",
                    path.display()
                );
            }
        }
    } else {
        info!(
            "No config found ({CONFIG_FILENAME}, {HOME_CONFIG_FILENAME}, or legacy {LEGACY_CONFIG_FILENAME}) near {} (or via PEX_CONFIG); using defaults.",
            base_dir().display()
        );
    }

    cfg
}

pub fn local_db_path() -> PathBuf {
    resolve_relative_path(Path::new(LOCAL_DB_DIR)).join(LOCAL_EPG_DB_FILE)
}

pub fn local_library_db_path() -> PathBuf {
    resolve_relative_path(Path::new(LOCAL_DB_DIR)).join(LOCAL_LIBRARY_DB_FILE)
}

/// Keep discovery as filesystem I/O: never open SQLite against the Plex share.
pub fn discover_databases(folder: &Path) -> Result<(Vec<PathBuf>, Option<PathBuf>), String> {
    let entries = fs::read_dir(folder).map_err(|e| {
        format!(
            "Cannot read {}: {e}. Check the folder path and read permissions, then try again.",
            folder.display()
        )
    })?;
    let mut guides = Vec::new();
    let mut library = None;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("tv.plex.providers.epg.cloud") && name.ends_with(".db") {
            guides.push(entry.path());
        } else if name == "com.plexapp.plugins.library.db" {
            library = Some(entry.path());
        }
    }
    guides.sort();
    Ok((guides, library))
}

pub fn readable_database(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut header = [0; 16];
    fs::File::open(path).and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| format!("Cannot read {}: {e}. Connect the network share or choose another Databases folder.", path.display()))?;
    if &header != b"SQLite format 3\0" {
        return Err(format!(
            "{} is not a SQLite database. Choose Plex's Databases folder.",
            path.display()
        ));
    }
    Ok(())
}

pub fn config_problem() -> Option<String> {
    let path = discover_config_path()?;
    fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str::<RawConfig>(&s).map_err(|e| e.to_string()))
        .err()
        .map(|e| {
            format!(
                "Cannot load {}: {e}. Repair this file and retry checks.",
                path.display()
            )
        })
}

/// Retain settings, but migrate legacy files on explicit save so an upgraded ZIP's
/// bundled config.json cannot replace the user's connection.
pub fn save_connection(
    epg: &Path,
    library: Option<&Path>,
    guide_only: bool,
) -> Result<PathBuf, String> {
    let existing = load_config();
    save_connection_with_snapshots(
        epg,
        library,
        existing.library_snapshot_dir.as_deref(),
        guide_only,
    )
}

pub fn save_connection_with_snapshots(
    epg: &Path,
    library: Option<&Path>,
    snapshot_dir: Option<&Path>,
    guide_only: bool,
) -> Result<PathBuf, String> {
    let source = discover_config_path();
    let path = env::var_os("PEX_CONFIG")
        .map(|p| resolve_relative_path(PathBuf::from(p)))
        .unwrap_or_else(|| match source.as_deref() {
            Some(p) if p.file_name().is_some_and(|n| n == LEGACY_CONFIG_FILENAME) => {
                p.with_file_name(CONFIG_FILENAME)
            }
            Some(p) => p.to_path_buf(),
            None => base_dir().join(CONFIG_FILENAME),
        });
    let mut value = if let Some(source) = source {
        let raw = fs::read_to_string(&source).map_err(|e| e.to_string())?;
        serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|e| format!("Repair {} before saving: {e}", source.display()))?
    } else {
        serde_json::json!({})
    };
    let object = value
        .as_object_mut()
        .ok_or("Config must contain a JSON object")?;
    object.remove("plex_db_source");
    object.insert("plex_epg_db_source".into(), serde_json::json!(epg));
    object.insert("plex_library_db_source".into(), serde_json::json!(library));
    if let Some(folder) = snapshot_dir {
        object.insert("library_snapshot_dir".into(), serde_json::json!(folder));
    } else {
        object.remove("library_snapshot_dir");
    }
    object.insert("guide_only".into(), serde_json::json!(guide_only));
    let parent = path.parent().ok_or("Config has no parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    use std::io::Write;
    staged
        .write_all(serde_json::to_string_pretty(&value).unwrap().as_bytes())
        .map_err(|e| e.to_string())?;
    staged.as_file().sync_all().map_err(|e| e.to_string())?;
    staged
        .persist(&path)
        .map_err(|e| format!("Cannot save {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_returns_all_guide_choices_and_only_the_live_library() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "tv.plex.providers.epg.cloud-a.db",
            "tv.plex.providers.epg.cloud-b.db",
            "com.plexapp.plugins.library.db",
            "com.plexapp.plugins.library.db-2020-01-01",
            "tv.plex.providers.epg.cloud-a.db-wal",
            "unrelated.db",
        ] {
            fs::write(dir.path().join(name), b"fixture file").unwrap();
        }
        let (guides, library) = discover_databases(dir.path()).unwrap();
        assert_eq!(guides.len(), 2);
        assert!(guides[0].ends_with("tv.plex.providers.epg.cloud-a.db"));
        assert_eq!(
            library,
            Some(dir.path().join("com.plexapp.plugins.library.db"))
        );
    }
}
