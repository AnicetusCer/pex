use std::{fs, path::PathBuf};

use tracing::info;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub plex_db_local: Option<String>,
    pub cache_dir: Option<String>,
    pub plex_db_source: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            plex_db_local: Some("plex_epg.db".into()),
            cache_dir: None,
            plex_db_source: None,
        }
    }
}

pub fn load_config() -> AppConfig {
    let cfg_path = PathBuf::from("config.json");
    if let Ok(s) = fs::read_to_string(&cfg_path) {
        // naive parsing (expects flat keys; we avoid pulling serde here)
        let mut cfg = AppConfig::default();
        if s.contains("\"plex_db_local\"") {
            if let Some(v) = extract_string(&s, "plex_db_local") { cfg.plex_db_local = Some(v); }
        }
        if s.contains("\"cache_dir\"") {
            if let Some(v) = extract_string(&s, "cache_dir") { cfg.cache_dir = Some(v); }
        }
                if s.contains("\"plex_db_source\"") {
            if let Some(v) = extract_string(&s, "plex_db_source") {
                cfg.plex_db_source = Some(v);
            }
        }
        info!("Loaded config from config.json");
        cfg
    } else {
        info!("No config.json found; using defaults");
        AppConfig::default()
    }
}

fn extract_string(src: &str, key: &str) -> Option<String> {
    // extremely small helper; not robust JSON, just for this file
    let needle = format!("\"{}\"", key);
    let idx = src.find(&needle)?;
    let rest = &src[idx + needle.len()..];
    let colon = rest.find(':')?;
    let rest = &rest[colon+1..];
    let first_quote = rest.find('"')?;
    let rest = &rest[first_quote+1..];
    let second_quote = rest.find('"')?;
    Some(rest[..second_quote].replace("\\\\", "\\").replace("\\\"", "\""))
}