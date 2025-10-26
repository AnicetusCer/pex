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
const CONFIG_FILENAME: &str = "config.json";

static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub cache_dir: Option<PathBuf>,
    pub plex_epg_db_source: Option<PathBuf>,
    pub plex_library_db_source: Option<PathBuf>,
    pub tmdb_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    cache_dir: Option<String>,
    #[serde(alias = "plex_db_source")]
    plex_epg_db_source: Option<String>,
    plex_library_db_source: Option<String>,
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
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(custom) = env::var("PEX_CONFIG") {
        let candidate = PathBuf::from(custom);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            resolve_relative_path(candidate)
        };
        candidates.push(candidate);
    }

    candidates.push(base_dir().join(CONFIG_FILENAME));

    if let Ok(cwd) = env::current_dir() {
        let candidate = cwd.join(CONFIG_FILENAME);
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    for path in candidates {
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

pub fn load_config() -> AppConfig {
    let mut cfg = AppConfig::default();

    if let Some((path, raw)) = read_config_source() {
        match serde_json::from_str::<RawConfig>(&raw) {
            Ok(mut parsed) => {
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
                            "`plex_db_source` is deprecated; rename it to `plex_epg_db_source` in config.json."
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
                            "`omdb_api_key` is deprecated; rename it to `tmdb_api_key` in config.json."
                        );
                    }
                    if raw.contains("\"the_movie_db_api_key\"") {
                        warn!(
                            "`the_movie_db_api_key` is deprecated; rename it to `tmdb_api_key` in config.json."
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
            "No {CONFIG_FILENAME} found near {} (or via PEX_CONFIG); using defaults.",
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
