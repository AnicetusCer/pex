// src/app/prefs.rs
use std::{fs, io};
use std::path::PathBuf;
use std::time::{Duration, Instant};

impl crate::app::PexApp {
    // ---- tiny flags ----
    #[allow(clippy::missing_const_for_fn)]
    pub(crate) fn mark_dirty(&mut self) {
        self.prefs_dirty = true;
    }

    pub(crate) fn maybe_save_prefs(&mut self) {
        // debounce a bit to avoid writing every frame
        if self.prefs_dirty && self.prefs_last_write.elapsed() >= Duration::from_millis(300) {
            self.save_prefs();
            self.prefs_dirty = false;
            self.prefs_last_write = Instant::now();
        }
    }

    // ---- load/save prefs ----
    pub(crate) fn load_prefs(&mut self) {
        let path = prefs_path();
        let Ok(txt) = fs::read_to_string(&path) else { return; };

        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let Some((k, v)) = line.split_once('=') else { continue; };
            let k = k.trim();
            let v = v.trim();

            match k {
                "day_range" => if let Some(dr) = super::DayRange::from_str(v) { self.current_range = dr; },
                "search" => self.search_query = v.to_string(),
                "sort_key" => if let Some(sk) = super::SortKey::from_str(v) { self.sort_key = sk; },
                "sort_desc" => self.sort_desc = matches!(v, "1" | "true" | "yes"),
                "poster_w" => if let Ok(n) = v.parse::<f32>() { self.poster_width_ui = n.clamp(120.0, 220.0); },
                "detail_w" => if let Ok(n) = v.parse::<f32>() {
                    self.detail_panel_width = n.clamp(260.0, 600.0);
                },
                "workers" => if let Ok(n) = v.parse::<usize>() { self.worker_count_ui = n.clamp(1, 32); },
                "hide_owned" => self.hide_owned = matches!(v, "1" | "true" | "yes"),
                "dim_owned" => self.dim_owned = matches!(v, "1" | "true" | "yes"),
                "dim_strength" => if let Ok(n) = v.parse::<f32>() { self.dim_strength_ui = n.clamp(0.10, 0.90); },
                "channels" => {
                    self.selected_channels.clear();
                    for ch in v.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                        self.selected_channels.insert(ch.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn save_prefs(&self) {
        let path = prefs_path();
        let _ = fs::create_dir_all(path.parent().unwrap_or_else(|| std::path::Path::new(".")));

        let channels_csv = if self.selected_channels.is_empty() {
            String::new()
        } else {
            self.selected_channels
                .iter()
                .map(|s| s.replace(',', " "))
                .collect::<Vec<_>>()
                .join(",")
        };

        let txt = format!(
            "# pex ui prefs\n\
             day_range={}\n\
             search={}\n\
             sort_key={}\n\
             sort_desc={}\n\
             poster_w={:.1}\n\
             detail_w={:.1}\n\
             workers={}\n\
             hide_owned={}\n\
             dim_owned={}\n\
             dim_strength={:.2}\n\
             channels={}\n",
            self.current_range.as_str(),
            self.search_query,
            self.sort_key.as_str(),
            if self.sort_desc { "1" } else { "0" },
            self.poster_width_ui,
            self.detail_panel_width,
            self.worker_count_ui,
            if self.hide_owned { "1" } else { "0" },
            if self.dim_owned { "1" } else { "0" },
            self.dim_strength_ui,
            channels_csv,
        );

        let _ = fs::write(path, txt);
    }

    /// record up to N posters that already have textures this run
    pub(crate) fn save_hotset_manifest(&self, max_items: usize) -> io::Result<()> {
        let mut lines = Vec::new();
        for row in self.rows.iter().filter(|r| r.tex.is_some()).take(max_items) {
            if let Some(p) = &row.path {
                lines.push(format!("{}\t{}", row.key, p.display()));
            }
        }
        fs::write(hotset_manifest_path(), lines.join("\n"))
    }
}

// ---- free helpers kept as functions for reuse at startup ----
pub fn prefs_path() -> PathBuf {
    crate::app::cache::cache_dir().join("ui_prefs.txt")
}

pub fn hotset_manifest_path() -> PathBuf {
    crate::app::cache::cache_dir().join("hotset.txt")
}

pub fn load_hotset_manifest() -> io::Result<std::collections::HashMap<String, PathBuf>> {
    let p = hotset_manifest_path();
    let txt = fs::read_to_string(&p)?;
    let mut out = std::collections::HashMap::new();
    for line in txt.lines() {
        if let Some((k, v)) = line.split_once('\t') {
            if !k.is_empty() && !v.is_empty() {
                out.insert(k.to_string(), PathBuf::from(v));
            }
        }
    }
    Ok(out)
}


