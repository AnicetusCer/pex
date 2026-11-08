// src/app/prefetch.rs
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui as eg;

struct PosterJob {
    row_indices: Vec<usize>,
    key: String,
    url: String,
    cached_path: Option<PathBuf>,
}

impl crate::app::PexApp {
    /// Start prefetch: queue all rows, but avoid repeated disk lookups by reusing row.path.
    /// Workers will download the SMALL variant (key `__s`) if missing.
    pub(crate) fn start_prefetch(&mut self, ctx: &eg::Context, prefetch_disabled: bool) {
        if self.prefetch_started {
            return;
        }

        self.prefetch_started = true;

        if prefetch_disabled || self.rows.is_empty() {
            let message = if prefetch_disabled {
                format!(
                    "Stage 4/4 - Poster downloads disabled via PEX_DISABLE_PREFETCH. Showing cached artwork for {} listings.",
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
            self.set_startup_progress_floor(1.0);
            self.boot_phase = super::BootPhase::Ready;
            self.prewarm_first_screen(ctx);
            ctx.request_repaint();
            return;
        }

        self.set_startup_progress_floor(crate::app::STARTUP_STAGE4_START);

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

        let (work_tx, work_rx) = mpsc::channel::<PosterJob>();
        let (done_tx, done_rx) = mpsc::channel::<crate::app::PrefetchDone>();
        self.done_rx = Some(done_rx);

        let work_rx = std::sync::Arc::new(std::sync::Mutex::new(work_rx));

        // One shared HTTP client.
        let client = match reqwest::blocking::Client::builder()
            .user_agent("pex/prefetch")
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(16)
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
                let PosterJob {
                    row_indices,
                    key,
                    url,
                    cached_path,
                } = match job {
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

                for row_idx in row_indices {
                    let msg = crate::app::PrefetchDone {
                        row_idx,
                        result: result.clone(),
                    };
                    if done_tx.send(msg).is_err() {
                        return;
                    }
                }
            });
        }

        let now_bucket = crate::app::utils::day_bucket(std::time::SystemTime::now());
        for job in self.poster_jobs(now_bucket) {
            let _ = work_tx.send(job);
        }
        self.prewarm_first_screen(ctx);
        ctx.request_repaint();
    }

    fn poster_jobs(&self, now_bucket: i64) -> Vec<PosterJob> {
        let soon = now_bucket..now_bucket + 2;
        let mut indices: Vec<usize> = (0..self.rows.len()).collect();
        indices.sort_by_key(|&idx| {
            !self.rows[idx]
                .airing
                .is_some_and(|ts| soon.contains(&crate::app::utils::day_bucket(ts)))
        });

        let mut jobs = Vec::<PosterJob>::new();
        let mut by_key = std::collections::HashMap::<&str, usize>::new();
        for idx in indices {
            let row = &self.rows[idx];
            if let Some(&job_idx) = by_key.get(row.key.as_str()) {
                let job = &mut jobs[job_idx];
                job.row_indices.push(idx);
                if job.cached_path.is_none() {
                    job.cached_path.clone_from(&row.path);
                }
            } else {
                by_key.insert(&row.key, jobs.len());
                jobs.push(PosterJob {
                    row_indices: vec![idx],
                    key: row.key.clone(),
                    url: row.url.clone(),
                    cached_path: row.path.clone(),
                });
            }
        }
        jobs
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
                self.set_startup_progress_floor(1.0);
            }
        } else {
            self.loading_progress = 1.0;
            self.set_startup_progress_floor(1.0);
        }

        if drained > 0 {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{types::PosterRow, PexApp, PosterState};

    fn listing(key: &str) -> PosterRow {
        PosterRow {
            title: "Uncached film".into(),
            url: String::new(),
            key: key.into(),
            airing: None,
            year: None,
            channel: None,
            channel_raw: None,
            channel_title: None,
            channel_thumb: None,
            genres: Vec::new(),
            guid: None,
            summary: None,
            audience_rating: None,
            critic_rating: None,
            path: None,
            tex: None,
            state: PosterState::Pending,
            owned: false,
            owned_modified: None,
            owned_hd: false,
            broadcast_hd: false,
            scheduled: false,
        }
    }

    #[test]
    fn offline_prefetch_displays_an_uncached_listing_without_workers() {
        let mut app = PexApp::default();
        app.rows.push(listing("uncached"));
        app.start_prefetch(&eframe::egui::Context::default(), true);
        assert!(app.should_show_grid());
        assert!(app.done_rx.is_none());
        assert_eq!(app.in_flight(), 0);
    }

    #[test]
    fn repeated_airings_share_one_job_and_any_known_cache_path() {
        let mut app = PexApp {
            rows: vec![listing("repeat"), listing("other"), listing("repeat")],
            ..Default::default()
        };
        app.rows[2].path = Some("artwork.jpg".into());
        let jobs = app.poster_jobs(0);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].row_indices, [0, 2]);
        assert_eq!(
            jobs[0].cached_path.as_deref(),
            Some(std::path::Path::new("artwork.jpg"))
        );
        assert_eq!(jobs[1].row_indices, [1]);
    }
}
