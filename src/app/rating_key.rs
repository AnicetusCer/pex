use super::{PexApp, RatingLookup};
use crate::config::{load_config, save_tmdb_api_key};
use eframe::egui as eg;

pub(super) struct RatingKeyEditor {
    draft: String,
    has_saved_key: bool,
    requested_film: Option<RatingLookup>,
    error: Option<String>,
    focus_input: bool,
}

impl PexApp {
    pub(super) fn open_rating_key_editor(&mut self, requested_film: Option<RatingLookup>) {
        if self.rating_key_editor.is_some() {
            return;
        }
        self.rating_key_feedback = None;
        self.has_ratings_key = load_config()
            .tmdb_api_key
            .is_some_and(|key| !key.trim().is_empty());
        self.rating_key_editor = Some(RatingKeyEditor {
            draft: String::new(),
            has_saved_key: self.has_ratings_key,
            requested_film,
            error: None,
            focus_input: true,
        });
    }

    pub(super) fn ui_render_rating_key(&mut self, ctx: &eg::Context) {
        let Some(mut editor) = self.rating_key_editor.take() else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut remove = false;
        let mut cancel = false;
        let input_id = eg::Id::new("tmdb_rating_key_input");
        eg::Window::new("TMDb ratings key")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                let label = ui.label(if editor.has_saved_key {
                    "A key is saved. Paste a new TMDb v3 API key to replace it."
                } else {
                    "Paste your TMDb v3 API key to look up film ratings."
                });
                let input = ui
                    .add(
                        eg::TextEdit::singleline(&mut editor.draft)
                            .id(input_id)
                            .password(true)
                            .hint_text("TMDb v3 API key")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label.id);
                if editor.focus_input {
                    input.request_focus();
                    editor.focus_input = false;
                }
                ui.hyperlink_to(
                    "Get a TMDb API key",
                    "https://www.themoviedb.org/settings/api",
                );
                ui.label("PEX saves the key in your local configuration file.");
                if let Some(error) = &editor.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.horizontal(|ui| {
                    let has_input = !editor.draft.trim().is_empty();
                    save = ui
                        .add_enabled(has_input, eg::Button::new("Save key"))
                        .clicked()
                        || (has_input
                            && input.lost_focus()
                            && ui.input(|i| i.key_pressed(eg::Key::Enter)));
                    if editor.has_saved_key {
                        remove = ui.button("Remove key").clicked();
                    }
                    cancel = ui.button("Cancel").clicked();
                });
            });

        if save || remove {
            let key = if remove {
                None
            } else {
                Some(editor.draft.as_str())
            };
            match save_tmdb_api_key(key) {
                Ok(_) => {
                    self.has_ratings_key = !remove;
                    self.rating_tx = None;
                    self.rating_rx = None;
                    self.rating_states.clear();
                    self.rating_key_feedback = Some(if remove {
                        "Ratings key removed."
                    } else {
                        "Ratings key saved."
                    });
                    if let (Some(film), Some(key)) = (editor.requested_film.take(), key) {
                        self.start_rating_lookup(film, key.trim().to_owned());
                    }
                    open = false;
                }
                Err(e) => editor.error = Some(format!("Could not save ratings settings. {e}")),
            }
        }
        if open && !cancel {
            self.rating_key_editor = Some(editor);
        } else {
            // Discard the field's undo history along with the draft.
            ctx.data_mut(|data| data.remove::<eg::text_edit::TextEditState>(input_id));
        }
    }
}
