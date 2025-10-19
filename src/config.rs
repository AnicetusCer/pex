use std::{fs, path::PathBuf, str::FromStr};

use serde::Deserialize;
use tracing::{info, warn};

pub const LOCAL_DB_DIR: &str = "db";
pub const LOCAL_EPG_DB_FILE: &str = "plex_epg.db";
pub const LOCAL_LIBRARY_DB_FILE: &str = "plex_library.db";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnedSourceKind {
    Filesystem,
    PlexLibrary,
}

impl Default for OwnedSourceKind {
    fn default() -> Self {
        Self::Filesystem
    }
}

impl FromStr for OwnedSourceKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "filesystem" => Ok(Self::Filesystem),
            "plex_library" | "plex" | "plexlibrary" => Ok(Self::PlexLibrary),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub cache_dir: Option<String>,
    pub plex_epg_db_source: Option<String>,
    pub plex_library_db_source: Option<String>,
    pub ffprobe_cmd: Option<String>,
    pub omdb_api_key: Option<String>,
    pub library_roots: Vec<String>,
    pub owned_source: OwnedSourceKind,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    cache_dir: Option<String>,
    #[serde(alias = "plex_db_source")]
    plex_epg_db_source: Option<String>,
    plex_library_db_source: Option<String>,
    library_roots: Option<Vec<String>>,
    ffprobe_cmd: Option<String>,
    omdb_api_key: Option<String>,
    owned_source: Option<String>,
}

pub fn load_config() -> AppConfig {
    let cfg_path = PathBuf::from("config.json");
    let mut cfg = AppConfig::default();

    match fs::read_to_string(&cfg_path) {
        Ok(raw) => match serde_json::from_str::<RawConfig>(&raw) {
            Ok(parsed) => {
                if parsed.cache_dir.is_some() {
                    cfg.cache_dir = parsed.cache_dir;
                }
                if parsed.plex_epg_db_source.is_some() {
                    cfg.plex_epg_db_source = parsed.plex_epg_db_source;
                    if raw.contains("\"plex_db_source\"") {
                        warn!(
                            "`plex_db_source` is deprecated; rename it to `plex_epg_db_source` in config.json."
                        );
                    }
                }
                if parsed.plex_library_db_source.is_some() {
                    cfg.plex_library_db_source = parsed.plex_library_db_source;
                }
                if parsed.ffprobe_cmd.is_some() {
                    cfg.ffprobe_cmd = parsed.ffprobe_cmd;
                }
                if parsed.omdb_api_key.is_some() {
                    cfg.omdb_api_key = parsed.omdb_api_key;
                }
                if let Some(list) = parsed.library_roots {
                    cfg.library_roots = list;
                }
                if let Some(mode) = parsed.owned_source {
                    match mode.parse::<OwnedSourceKind>() {
                        Ok(kind) => cfg.owned_source = kind,
                        Err(_) => warn!(
                            "Unknown owned_source `{mode}` in config.json; falling back to filesystem."
                        ),
                    }
                }
                info!("Loaded config from {}", cfg_path.display());
            }
            Err(err) => {
                warn!("Failed to parse config.json ({}). Using defaults.", err);
            }
        },
        Err(_) => {
            info!("No config.json found; using defaults");
        }
    }

    cfg
}

pub fn local_db_path() -> PathBuf {
    PathBuf::from(LOCAL_DB_DIR).join(LOCAL_EPG_DB_FILE)
}

pub fn local_library_db_path() -> PathBuf {
    PathBuf::from(LOCAL_DB_DIR).join(LOCAL_LIBRARY_DB_FILE)
}
