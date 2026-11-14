// src/app/prefetch.rs
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui as eg;

pub(super) fn usable_poster_url(raw: &str) -> bool {
    reqwest::Url::parse(raw)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

struct PosterJob {
    row_indices: Vec<usize>,
    key: String,
    url: String,
    cached_path: Option<PathBuf>,
}

impl crate::app::PexApp {
    /// Warm cached artwork first; each uncached poster has one job across all airings.
    pub(crate) fn start_prefetch(&mut self, ctx: &eg::Context, prefetch_disabled: bool) {
        if self.prefetch_started {
            return;
        }
        self.prefetch_started = true;
        self.completed = 0;
        self.failed = 0;
        let today = crate::app::utils::day_bucket(std::time::SystemTime::now());
        let first_cards: Vec<_> = self
            .filtered_guide()
            .days
            .into_iter()
            .flat_map(|(_, cards)| cards)
            .take(super::PREWARM_UPLOADS)
            .collect();
        let jobs = self.poster_jobs(today, &first_cards);
        self.total_targets = jobs.len();
        let mut downloads = Vec::new();
        for job in jobs {
            if let Some(path) = job.cached_path {
                for idx in job.row_indices {
                    let row = &mut self.rows[idx];
                    row.path = Some(path.clone());
                    row.state = super::PosterState::Cached;
                }
                self.completed += 1;
            } else if !prefetch_disabled {
                downloads.push(job);
            }
        }
        for idx in first_cards {
            self.try_lazy_upload_row(ctx, idx);
        }
        if downloads.is_empty() {
            self.total_targets = self.completed;
            self.finish_prefetch(if prefetch_disabled {
                "Poster downloads are disabled. Showing available cached artwork.".into()
            } else if self.total_targets == 0 {
                "Guide ready. No poster artwork available.".into()
            } else {
                format!("Artwork ready ({} cached posters).", self.completed)
            });
            return;
        }

        self.set_phase(super::Phase::Prefetching);
        self.set_startup_progress_floor(crate::app::STARTUP_STAGE4_START);
        self.loading_progress = self.completed as f32 / self.total_targets as f32;
        self.stage4_complete_message = None;
        let client = match reqwest::blocking::Client::builder()
            .user_agent("pex/prefetch")
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(16)
            .build()
        {
            Ok(client) => client,
            Err(e) => {
                self.failed = downloads.len();
                self.finish_prefetch(format!("Could not start poster downloads: {e}"));
                return;
            }
        };

        let worker_count = self.worker_count_ui.clamp(1, downloads.len());
        let queue = std::sync::Arc::new(std::sync::Mutex::new(downloads.into_iter()));
        let (done_tx, done_rx) = mpsc::channel();
        self.done_rx = Some(done_rx);
        for _ in 0..worker_count {
            let queue = std::sync::Arc::clone(&queue);
            let done_tx = done_tx.clone();
            let client = client.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let next = queue.lock().unwrap().next();
                let Some(job) = next else { break };
                let result = crate::app::cache::download_and_store_resized_with_client(
                    &client,
                    &job.url,
                    &job.key,
                    super::RESIZE_MAX_W,
                    super::RESIZE_QUALITY,
                );
                if done_tx
                    .send(crate::app::PrefetchDone {
                        row_indices: job.row_indices,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                ctx.request_repaint();
            });
        }
    }

    fn finish_prefetch(&mut self, message: String) {
        self.loading_progress = 1.0;
        self.set_startup_progress_floor(1.0);
        self.stage4_complete_message = Some(message.clone());
        if !self.owned_scan_in_progress {
            self.set_status(message);
        }
        self.done_rx = None;
    }

    fn poster_jobs(&self, now_bucket: i64, first_cards: &[usize]) -> Vec<PosterJob> {
        let soon = now_bucket..now_bucket + 2;
        let first_cards: std::collections::HashMap<_, _> = first_cards
            .iter()
            .copied()
            .enumerate()
            .map(|(rank, idx)| (idx, rank))
            .collect();
        let mut indices: Vec<usize> = (0..self.rows.len()).collect();
        indices.sort_by_cached_key(|&idx| {
            if let Some(&rank) = first_cards.get(&idx) {
                return rank;
            }
            let airs_soon = self.rows[idx]
                .airing
                .is_some_and(|ts| soon.contains(&crate::app::utils::day_bucket(ts)));
            super::PREWARM_UPLOADS + usize::from(!airs_soon)
        });

        let mut jobs = Vec::<PosterJob>::new();
        let mut by_key = std::collections::HashMap::<&str, usize>::new();
        for idx in indices {
            let row = &self.rows[idx];
            if row.key.is_empty() || (row.path.is_none() && !usable_poster_url(&row.url)) {
                continue;
            }
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
                            self.completed += 1;
                            for idx in msg.row_indices {
                                let row = &mut self.rows[idx];
                                row.path = Some(path.clone());
                                row.state = super::PosterState::Cached;
                            }
                        }
                        Err(_) => {
                            self.failed += 1;
                            for idx in msg.row_indices {
                                self.rows[idx].state = super::PosterState::Failed;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.failed += self.in_flight();
                    break;
                }
            }
        }
        if self.in_flight() == 0 {
            self.finish_prefetch(format!(
                "Artwork ready ({} posters cached, {} failed).",
                self.completed, self.failed
            ));
        } else if drained > 0 {
            self.loading_progress =
                (self.completed + self.failed) as f32 / self.total_targets as f32;
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
            film_key: key.into(),
            guide_media_id: 1,
            movie_subscription_id: None,
            url: format!("https://posters.example.invalid/{key}.jpg"),
            key: key.into(),
            airing: None,
            year: None,
            channel: None,
            channel_raw: None,
            channel_title: None,
            channel_thumb: None,
            guide_channel: Default::default(),
            genres: Vec::new(),
            guid: None,
            summary: None,
            audience_rating: None,
            critic_rating: None,
            path: None,
            state: PosterState::Pending,
            owned: false,
            owned_modified: None,
            owned_quality: Default::default(),
            airing_quality: crate::app::types::AiringQuality::Unknown,
            scheduled: Default::default(),
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
        let jobs = app.poster_jobs(0, &[]);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].row_indices, [0, 2]);
        assert_eq!(
            jobs[0].cached_path.as_deref(),
            Some(std::path::Path::new("artwork.jpg"))
        );
        assert_eq!(jobs[1].row_indices, [1]);
    }

    #[test]
    fn missing_and_invalid_poster_urls_start_no_workers() {
        let mut app = PexApp::default();
        for url in ["", "/library/poster.jpg", "file:///bad", "https://"] {
            let mut row = listing("missing");
            row.url = url.into();
            app.rows.push(row);
        }
        app.start_prefetch(&eframe::egui::Context::default(), false);
        assert!(app.done_rx.is_none());
        assert_eq!(app.in_flight(), 0);
        assert_eq!(app.total_targets, 0);
        assert!(app.should_show_grid());
    }

    #[test]
    fn prewarm_uses_displayed_films_and_shares_textures_across_airings() {
        use std::time::{Duration, SystemTime};

        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("poster.png");
        image::RgbaImage::new(2, 3).save(&path).unwrap();
        let now = SystemTime::now();
        let mut rows = Vec::new();
        for i in 0..=super::super::PREWARM_UPLOADS {
            let mut row = listing("repeated");
            row.airing = Some(now + Duration::from_secs(60 + i as u64));
            row.path = Some(path.clone());
            rows.push(row);
        }
        let mut other = listing("second");
        other.airing = Some(now + Duration::from_secs(3600));
        other.path = Some(path);
        rows.push(other);

        let mut app = PexApp {
            rows,
            ..Default::default()
        };
        let ctx = eframe::egui::Context::default();
        app.start_prefetch(&ctx, true);
        assert!(app.poster_textures.contains_key("second"));
        let first_texture = app.poster_textures[&app.rows[0].key].id();
        // A later showing already has its texture and spends no upload budget.
        assert!(!app.try_lazy_upload_row(&ctx, 1));
        assert_eq!(app.poster_textures[&app.rows[1].key].id(), first_texture);
        assert_eq!(app.poster_textures.len(), 2);
        assert_eq!(app.completed, 2);
    }

    #[test]
    fn offscreen_selection_displays_art_as_soon_as_its_download_completes() {
        use eframe::egui as eg;
        use std::time::{Duration, SystemTime};

        let (tx, rx) = std::sync::mpsc::channel();
        let now = SystemTime::now();
        let rows = (0..40)
            .map(|i| {
                let mut row = listing(&format!("film-{i}"));
                row.airing = Some(now + Duration::from_secs(60 + i));
                row
            })
            .collect();
        let selected = 39;
        let mut ui = crate::app::test_fixtures::TestUi {
            app: PexApp {
                rows,
                did_init: true,
                setup_checked: true,
                prefetch_started: true,
                selected_idx: Some(selected),
                total_targets: 1,
                done_rx: Some(rx),
                ..Default::default()
            },
            screen: Some(eg::vec2(1000.0, 700.0)),
            ..Default::default()
        };
        ui.frame(vec![]);
        let card = ui
            .ctx
            .read_response(eg::Id::new(("card_sel", selected)))
            .unwrap();
        assert!(card.rect.top() > 700.0);
        ui.label("Poster loading…");

        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("poster.png");
        image::RgbaImage::new(2, 3).save(&path).unwrap();
        tx.send(crate::app::PrefetchDone {
            row_indices: vec![selected],
            result: Ok(path),
        })
        .unwrap();
        let output = ui.frame(vec![]);
        let texture = ui.app.poster_textures[&ui.app.rows[selected].key].id();
        let primitives = ui.ctx.tessellate(output.shapes, output.pixels_per_point);
        assert!(primitives.iter().any(|p| {
            matches!(&p.primitive, eg::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture && mesh.calc_bounds().intersects(p.clip_rect))
        }));
        assert_eq!(ui.app.in_flight(), 0);
    }

    #[test]
    fn cold_guide_displays_showing_before_art_and_uploads_it_on_completion() {
        use eframe::egui as eg;
        use std::time::{Duration, SystemTime};

        let mut row = listing("art");
        row.channel = Some("Film Channel".into());
        row.airing = Some(SystemTime::now() + Duration::from_secs(60));
        let repeat = listing("art");
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = PexApp {
            rows: vec![row, repeat],
            did_init: true,
            setup_checked: true,
            prefetch_started: true,
            total_targets: 1,
            done_rx: Some(rx),
            ..Default::default()
        };
        let ctx = eg::Context::default();
        let frame = |app: &mut PexApp| {
            let input = eg::RawInput {
                screen_rect: Some(eg::Rect::from_min_size(
                    eg::Pos2::ZERO,
                    eg::vec2(1600.0, 900.0),
                )),
                ..Default::default()
            };
            ctx.run(input, |ctx| app.update_ui(ctx))
        };
        let output = frame(&mut app);
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, eg::Shape::Text(t)
            if t.galley.text().contains("Uncached film") && t.galley.text().contains("Film Channel") &&
            t.galley.text().contains("UTC"))));
        assert_eq!(app.in_flight(), 1);
        assert!(app.poster_textures.is_empty());

        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("poster.png");
        image::RgbaImage::new(2, 3).save(&path).unwrap();
        tx.send(crate::app::PrefetchDone {
            row_indices: vec![0, 1],
            result: Ok(path.clone()),
        })
        .unwrap();
        frame(&mut app);
        assert!(app.poster_textures.contains_key(&app.rows[0].key));
        assert_eq!(app.rows[1].path.as_ref(), Some(&path));
        assert_eq!(app.in_flight(), 0);
        assert_eq!(app.completed, 1);
    }
}
