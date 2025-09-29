use std::{fs, path::PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

#[derive(Clone, Debug, Default)]
pub struct UiOverrides {
    pub hide_owned: Option<bool>,
    pub dim_owned: Option<bool>,
    pub schedule_window: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub plex_db_local: Option<String>,
    pub cache_dir: Option<String>,
    pub plex_db_source: Option<String>,
    pub library_roots: Vec<String>,
    pub hide_owned_by_default: bool,
    pub dim_owned_by_default: bool,
    pub ui: UiOverrides,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            plex_db_local: Some("plex_epg.db".into()),
            cache_dir: None,
            plex_db_source: None,
            library_roots: Vec::new(),
            hide_owned_by_default: false,
            dim_owned_by_default: false,
            ui: UiOverrides::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawUi {
    hide_owned: Option<bool>,
    dim_owned: Option<bool>,
    schedule_window: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    plex_db_local: Option<String>,
    cache_dir: Option<String>,
    plex_db_source: Option<String>,
    library_roots: Option<Vec<String>>,
    hide_owned_by_default: Option<bool>,
    dim_owned_by_default: Option<bool>,
    ui: Option<RawUi>,
}

pub fn load_config() -> AppConfig {
    let cfg_path = PathBuf::from("config.json");
    let mut cfg = AppConfig::default();

    match fs::read_to_string(&cfg_path) {
        Ok(raw) => match serde_json::from_str::<RawConfig>(&raw) {
            Ok(parsed) => {
                if parsed.plex_db_local.is_some() {
                    cfg.plex_db_local = parsed.plex_db_local;
                }
                if parsed.cache_dir.is_some() {
                    cfg.cache_dir = parsed.cache_dir;
                }
                if parsed.plex_db_source.is_some() {
                    cfg.plex_db_source = parsed.plex_db_source;
                }
                if let Some(list) = parsed.library_roots {
                    cfg.library_roots = list;
                }
                if let Some(flag) = parsed.hide_owned_by_default {
                    cfg.hide_owned_by_default = flag;
                }
                if let Some(flag) = parsed.dim_owned_by_default {
                    cfg.dim_owned_by_default = flag;
                }
                if let Some(ui) = parsed.ui {
                    cfg.ui.hide_owned = ui.hide_owned;
                    cfg.ui.dim_owned = ui.dim_owned;
                    cfg.ui.schedule_window = ui.schedule_window;
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
