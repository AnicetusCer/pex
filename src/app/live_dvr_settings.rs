use super::live_schedule::validate_server_url;
use super::PexApp;
use crate::config::{load_config, save_live_dvr, LiveDvrConnection};
use eframe::egui as eg;

pub(super) struct LiveDvrEditor {
    server_url: String,
    token: String,
    has_saved_connection: bool,
    error: Option<String>,
}

impl LiveDvrEditor {
    fn connection(&self, saved: Option<&LiveDvrConnection>) -> Result<LiveDvrConnection, String> {
        let server_url = validate_server_url(self.server_url.trim())?;
        let mut token = self.token.trim();
        if token.is_empty() {
            let previous = saved.ok_or("Enter your Plex token.")?;
            if validate_server_url(&previous.server_url).ok().as_ref() != Some(&server_url) {
                return Err("Enter a token for the new server address.".into());
            }
            token = previous.token.trim();
        }
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("Enter just the Plex token, without spaces or line breaks.".into());
        }
        Ok(LiveDvrConnection {
            server_url: server_url.to_string(),
            token: token.to_owned(),
        })
    }
}

impl PexApp {
    pub(super) fn open_live_dvr_settings(&mut self) {
        if self.live_dvr_editor.is_some() {
            return;
        }
        let saved = load_config().live_dvr;
        self.live_dvr_editor = Some(LiveDvrEditor {
            server_url: saved
                .as_ref()
                .and_then(|cfg| validate_server_url(&cfg.server_url).ok())
                .map(String::from)
                .unwrap_or_default(),
            token: String::new(),
            has_saved_connection: saved.is_some(),
            error: None,
        });
    }

    pub(super) fn render_live_dvr_settings(&mut self, ctx: &eg::Context) {
        let Some(mut editor) = self.live_dvr_editor.take() else {
            return;
        };
        let token_id = eg::Id::new("plex_dvr_token");
        let mut open = true;
        let mut save = false;
        let mut disconnect = false;
        let mut close = false;
        eg::Window::new("Plex DVR Schedule")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(385.0)
            .show(ctx, |ui| {
                ui.label("Read the scheduled showings from the Plex server that supplies this guide.");
                let server_label = ui.label("Plex Media Server address");
                ui.add(
                    eg::TextEdit::singleline(&mut editor.server_url)
                        .id(eg::Id::new("plex_dvr_server_url"))
                        .hint_text("http://your-nas:32400")
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(server_label.id);
                let token_label = ui.label("Plex token");
                ui.add(
                    eg::TextEdit::singleline(&mut editor.token)
                        .id(token_id)
                        .password(true)
                        .hint_text(if editor.has_saved_connection {
                            "Leave blank to keep the token for this server"
                        } else {
                            "Paste the token for an account with DVR access"
                        })
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(token_label.id);
                ui.hyperlink_to(
                    "Find your Plex token",
                    "https://support.plex.tv/articles/204059436-finding-an-authentication-token-x-plex-token/",
                );
                ui.label("PEX saves the token in plain text in your local configuration file. Use HTTPS or a trusted local network.");
                if let Some(error) = &editor.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.horizontal(|ui| {
                    save = ui.button("Save and refresh").clicked();
                    if editor.has_saved_connection {
                        disconnect = ui.button("Disconnect").clicked();
                    }
                    close = ui.button("Close").clicked();
                });
            });
        if save || disconnect {
            let result = if disconnect {
                save_live_dvr(None)
            } else {
                editor
                    .connection(load_config().live_dvr.as_ref())
                    .and_then(|cfg| save_live_dvr(Some(&cfg)))
            };
            match result {
                Ok(_) => {
                    self.refresh_live_dvr();
                    open = false;
                }
                Err(error) => editor.error = Some(error),
            }
        }
        if open && !close {
            self.live_dvr_editor = Some(editor);
        } else {
            ctx.data_mut(|data| data.remove::<eg::text_edit::TextEditState>(token_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_token_is_bound_to_the_saved_address() {
        let saved = LiveDvrConnection {
            server_url: "https://plex.example.test".into(),
            token: "fixture-key".into(),
        };
        let mut editor = LiveDvrEditor {
            server_url: "https://plex.example.test/".into(),
            token: String::new(),
            has_saved_connection: true,
            error: None,
        };
        assert_eq!(editor.connection(Some(&saved)).unwrap().token, saved.token);
        editor.server_url = "https://other.example.test".into();
        assert!(editor.connection(Some(&saved)).is_err());
        assert!(editor.connection(None).is_err());
        editor.token = "new-fixture-key".into();
        assert_eq!(editor.connection(Some(&saved)).unwrap().token, editor.token);
        editor.token = "bad\nheader".into();
        assert!(editor.connection(None).is_err());
    }
}
