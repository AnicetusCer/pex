use super::{live_schedule, scheduled::ScheduledIndex, PexApp};
use crate::config::LiveDvrConnection;
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct LiveDvr {
    connection: Option<LiveDvrConnection>,
    receiver: Option<Receiver<Result<ScheduledIndex, String>>>,
    superseded: bool,
    pub(super) index: Option<Box<ScheduledIndex>>,
}

impl PexApp {
    pub(super) fn live_dvr_configured(&self) -> bool {
        self.live_dvr.connection.is_some()
    }

    pub(super) fn live_dvr_busy(&self) -> bool {
        self.live_dvr.receiver.is_some()
    }

    pub(super) fn refresh_live_dvr(&mut self) {
        self.configure_live_dvr(crate::config::load_config().live_dvr, true);
    }

    pub(super) fn configure_live_dvr(
        &mut self,
        connection: Option<LiveDvrConnection>,
        refresh: bool,
    ) {
        let changed = connection != self.live_dvr.connection;
        if changed {
            self.live_dvr.connection = connection;
            self.live_dvr.superseded = self.live_dvr_busy();
            self.live_dvr.index = None;
            self.live_dvr_status = None;
            self.apply_scheduled_flags();
        }
        if (refresh || changed) && self.live_dvr_configured() {
            self.start_live_dvr_fetch();
        }
    }

    fn start_live_dvr_fetch(&mut self) {
        if self.live_dvr_busy() {
            return;
        }
        let Some(connection) = self.live_dvr.connection.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.live_dvr.receiver = Some(receiver);
        self.live_dvr_status = Some("Reading the Plex DVR schedule…".into());
        std::thread::spawn(move || {
            let _ = sender.send(live_schedule::fetch_schedule(&connection));
        });
    }

    pub(super) fn poll_live_dvr(&mut self) {
        let Some(receiver) = self.live_dvr.receiver.as_ref() else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The schedule worker stopped.".into()),
        };
        self.live_dvr.receiver = None;
        if std::mem::take(&mut self.live_dvr.superseded) {
            self.start_live_dvr_fetch();
            return;
        }
        match result {
            Ok(index) => {
                self.live_dvr.index = Some(Box::new(index));
                let time = chrono::Local::now().format("%H:%M");
                self.live_dvr_status = Some(format!("Plex DVR schedule updated at {time}."));
            }
            Err(error) => {
                self.live_dvr.index = None;
                self.live_dvr_status = Some(format!(
                    "Could not read the Plex DVR schedule. {error} Using the library schedule when available."
                ));
            }
        }
        self.apply_scheduled_flags();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_or_replaced_connections_discard_in_flight_results() {
        let mut app = PexApp::default();
        let old = LiveDvrConnection {
            server_url: "http://127.0.0.1:32400".into(),
            token: "fixture-only".into(),
        };
        app.live_dvr.connection = Some(old.clone());
        app.live_dvr.index = Some(Box::default());
        let (sender, receiver) = mpsc::channel();
        app.live_dvr.receiver = Some(receiver);

        // A repeated refresh shares the current request. Replacing only the token
        // also invalidates that request, even when the server address is unchanged.
        app.configure_live_dvr(Some(old.clone()), true);
        assert!(!app.live_dvr.superseded);
        app.configure_live_dvr(
            Some(LiveDvrConnection {
                token: "replacement-fixture".into(),
                ..old
            }),
            true,
        );
        assert!(app.live_dvr.superseded);
        assert!(app.live_dvr.index.is_none());
        assert!(app.live_dvr_busy());
        app.configure_live_dvr(None, true);
        assert!(!app.live_dvr_configured());

        sender.send(Ok(ScheduledIndex::default())).unwrap();
        app.poll_live_dvr();
        assert!(!app.live_dvr_busy());
        assert!(app.live_dvr.index.is_none());
        assert!(app.live_dvr_status.is_none());
    }
}
