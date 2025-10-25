use std::{fs, path::PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

pub const LOCAL_DB_DIR: &str = "db";
pub const LOCAL_EPG_DB_FILE: &str = "plex_epg.db";
pub const LOCAL_LIBRARY_DB_FILE: &str = "plex_library.db";

#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub cache_dir: Option<String>,
    pub plex_epg_db_source: Option<String>,
    pub plex_library_db_source: Option<String>,
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

pub fn load_config() -> AppConfig {
    let cfg_path = PathBuf::from("config.json");
    let mut cfg = AppConfig::default();

    match fs::read_to_string(&cfg_path) {
        Ok(raw) => match serde_json::from_str::<RawConfig>(&raw) {
            Ok(mut parsed) => {
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
