use std::{fs, path::PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

pub const LOCAL_DB_DIR: &str = "db";
pub const LOCAL_DB_FILE: &str = "plex_epg.db";

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub cache_dir: Option<String>,
    pub plex_db_source: Option<String>,
    pub ffprobe_cmd: Option<String>,
    pub omdb_api_key: Option<String>,
    pub library_roots: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            cache_dir: None,
            plex_db_source: None,
            ffprobe_cmd: None,
            omdb_api_key: None,
            library_roots: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    cache_dir: Option<String>,
    plex_db_source: Option<String>,
    library_roots: Option<Vec<String>>,
    ffprobe_cmd: Option<String>,
    omdb_api_key: Option<String>,
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
                if parsed.plex_db_source.is_some() {
                    cfg.plex_db_source = parsed.plex_db_source;
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
    PathBuf::from(LOCAL_DB_DIR).join(LOCAL_DB_FILE)
}
