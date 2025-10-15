// src/app/prefetch.rs
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui as eg;

impl crate::app::PexApp {
    /// Start prefetch: queue all rows, but avoid repeated disk lookups by reusing row.path.
    /// Workers will download the SMALL variant (key `__s`) if missing.
    pub(crate) fn start_prefetch(&mut self, ctx: &eg::Context) {
        if self.prefetch_started {
            return;
        }

        // Allow opting out instead of opting in.
        let prefetch_disabled = std::env::var_os("PEX_DISABLE_PREFETCH").is_some();

        if prefetch_disabled || self.rows.is_empty() {
            let message = if prefetch_disabled {
                format!(
                    "Stage 4/4 - Prefetch disabled via PEX_DISABLE_PREFETCH (posters will load on demand). {} items queued.",
                    self.rows.len()
                )
            } else {
                "Stage 4/4 - No posters to prefetch (empty dataset).".into()
            };
            self.stage4_complete_message = Some(message.clone());
            if !self.owned_scan_in_progress {
                self.set_status(message.clone());
            }
            self.last_item_msg = message;
            self.total_targets = 0;
            self.completed = 0;
            self.failed = 0;
            self.loading_progress = 1.0;
            self.boot_phase = super::BootPhase::Ready;
            self.prewarm_first_screen(ctx);
            ctx.request_repaint();
            return;
        }

        self.prefetch_started = true;

        self.completed = 0;
        self.failed = 0;
        self.total_targets = self.rows.len();
        self.loading_progress = if self.total_targets == 0 { 1.0 } else { 0.0 };
        self.last_item_msg = if self.total_targets > 0 {
            format!(
                "Artwork cache progress: 0/{} cached (0 failed).",
                self.total_targets
            )
        } else {
            String::new()
        };
        self.set_phase(super::Phase::Prefetching);
        self.stage4_complete_message = None;

        let (work_tx, work_rx) = mpsc::channel::<super::WorkItem>();
        let (done_tx, done_rx) = mpsc::channel::<crate::app::PrefetchDone>();
        self.work_tx = Some(work_tx.clone());
        self.done_rx = Some(done_rx);

        let work_rx = std::sync::Arc::new(std::sync::Mutex::new(work_rx));

        // One shared HTTP client.
        let client = match reqwest::blocking::Client::builder()
            .user_agent("pex/prefetch")
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(16)
            .default_headers({
                use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
                let mut h = HeaderMap::new();
                h.insert(
                    ACCEPT,
                    HeaderValue::from_static("image/avif,image/webp,image/*;q=0.8,*/*;q=0.5"),
                );
                h
            })
            .build()
        {
            Ok(c) => std::sync::Arc::new(c),
            Err(e) => {
                self.set_status(format!("http client build failed: {e}"));
                self.failed = self.total_targets;
                self.loading_progress = 1.0;
                self.boot_phase = super::BootPhase::Ready;
                return;
            }
        };

        for _ in 0..self.worker_count_ui {
            let work_rx = std::sync::Arc::clone(&work_rx);
            let done_tx = done_tx.clone();
            let client = std::sync::Arc::clone(&client);

            std::thread::spawn(move || loop {
                let job = {
                    let rx = work_rx.lock().unwrap();
                    rx.recv()
                };
                let (row_idx, key, url, cached_path) = match job {
                    Ok(t) => t,
                    Err(_) => break,
                };

                let result: Result<PathBuf, String> = cached_path.map_or_else(
                    || {
                        crate::app::cache::download_and_store_resized_with_client(
                            &client,
                            &url,
                            &key,
                            super::RESIZE_MAX_W,
                            super::RESIZE_QUALITY,
                        )
                        .or_else(|_e| crate::app::cache::download_and_store(&url, &key))
                    },
                    Ok,
                );

                let _ = done_tx.send(crate::app::PrefetchDone { row_idx, result });
            });
        }

        // Prioritize “soon” items (next 2 days), then the rest.
        let now_bucket = crate::app::utils::day_bucket(std::time::SystemTime::now());
        let soon_cutoff = now_bucket + 2;

        let mut indices: Vec<(bool, usize)> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let prio = r
                    .airing
                    .map(|ts| crate::app::utils::day_bucket(ts) < soon_cutoff)
                    .unwrap_or(false);
                (prio, i)
            })
            .collect();

        indices.sort_by_key(|(prio, i)| (std::cmp::Reverse(*prio), *i));

        for (_, idx) in indices {
            let row = &mut self.rows[idx];
            row.state = if row.path.is_some() {
                super::PosterState::Cached
            } else {
                super::PosterState::Pending
            };
            let _ = work_tx.send((idx, row.key.clone(), row.url.clone(), row.path.clone()));
        }

        // Perceptual boost
        self.prewarm_first_screen(ctx);
        ctx.request_repaint();
    }

    /// Poll prefetch completions and update progress/splash.
    pub(crate) fn poll_prefetch_done(&mut self, ctx: &eg::Context) {
        let mut drained = 0usize;

        while drained < super::MAX_DONE_PER_FRAME {
            let Some(rx) = &self.done_rx else {
                break;
            };

            match rx.try_recv() {
                Ok(msg) => {
                    drained += 1;
                    match msg.result {
                        Ok(path) => {
                            if let Some(row) = self.rows.get_mut(msg.row_idx) {
                                row.path = Some(path);
                                row.state = super::PosterState::Cached; // will be uploaded lazily during paint
                                self.completed += 1;
                                self.last_item_msg = format!("Cached: {}", row.title);
                            } else {
                                self.failed += 1;
                            }
                        }
                        Err(e) => {
                            if let Some(row) = self.rows.get_mut(msg.row_idx) {
                                row.state = super::PosterState::Failed;
                                self.failed += 1;
                                self.last_item_msg =
                                    format!("Download failed: {} — {}", row.title, e);
                            } else {
                                self.failed += 1;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }

        if self.total_targets > 0 {
            self.loading_progress =
                ((self.completed + self.failed) as f32 / self.total_targets as f32).clamp(0.0, 1.0);

            self.last_item_msg = format!(
                "Artwork cache progress: {}/{} cached ({} failed).",
                self.completed, self.total_targets, self.failed
            );

            if (self.completed + self.failed) >= self.total_targets {
                let message = format!(
                    "Stage 4/4 - Artwork cache ready ({} posters cached, {} failed).",
                    self.completed, self.failed
                );
                self.stage4_complete_message = Some(message.clone());
                if !self.owned_scan_in_progress {
                    self.set_status(message.clone());
                }
                self.last_item_msg = message;
            }
        } else {
            self.loading_progress = 1.0;
        }

        if drained > 0 {
            ctx.request_repaint();
        }
    }
}
