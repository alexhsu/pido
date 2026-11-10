//! The root view: Ely's FocusScope wrapping a sidebar and the session pane.

use ely_gpui_component::forms::{InputEvent, TextInput};
use ely_gpui_component::primitives::FocusScope;
use ely_gpui_component::theme::{ActiveTheme, Mode, Theme};
use gpui::{
    App, AppContext, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, actions, div,
};

use crate::state::{AppState, Conn};
use crate::ui::{chat, composer, sidebar, welcome};

actions!(pido, [Stop]);

/// Height of the session header above the chat pane.
pub const HEADER_HEIGHT: f32 = 52.;

pub struct PidoApp {
    pub focus: FocusHandle,
    pub state: gpui::Entity<AppState>,
    /// The composer's multi-line field, owned here so it survives re-renders.
    pub draft: Option<gpui::Entity<TextInput>>,
    /// The welcome screen's project-directory field, created lazily.
    pub welcome_dir: Option<gpui::Entity<TextInput>>,
    /// Last observed window size, persisted on quit.
    window_size: Option<(f32, f32)>,
    /// Whether the composer has been focused since the session went live.
    composer_focused: bool,
    /// The dialog's text field (input/editor), per pending request id.
    dialog_field: Option<gpui::Entity<TextInput>>,
    /// The request id `dialog_field` was built for.
    dialog_field_for: Option<String>,
}

impl PidoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let state = AppState::new(cx);
        // Re-render whenever the state entity changes, and drain any text a
        // cancelled/failed action wants restored into the composer.
        cx.observe(&state, |this, state, cx| {
            let restored = state.update(cx, |state, _| state.take_pending_composer_text());
            if let (Some(text), Some(draft)) = (restored, this.draft.clone()) {
                draft.update(cx, |input, cx| {
                    if !input.text().trim().is_empty() {
                        input.insert("\n", cx);
                    }
                    input.insert(&text, cx);
                });
            }
            cx.notify();
        })
        .detach();
        let app = Self {
            focus,
            state,
            draft: None,
            welcome_dir: None,
            window_size: None,
            composer_focused: false,
            dialog_field: None,
            dialog_field_for: None,
        };
        app.observe_window_for_quit(cx);
        app
    }

    /// Stash the window size on every render and persist it when the app
    /// quits, so the window reopens where the user left it.
    fn observe_window_for_quit(&self, cx: &mut Context<Self>) {
        cx.on_app_quit(|this, _cx| {
            if let Some((width, height)) = this.window_size {
                crate::settings::save_setting(|s| {
                    s.window = Some(crate::settings::WindowState { width, height });
                });
            }
            async {}
        })
        .detach();
    }

    /// Called once the window is open: fields that need a Window are created
    /// here (see main.rs).
    pub fn setup(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.draft = Some(cx.new(|cx| {
            TextInput::new(window, cx)
                .multi_line(1, 8)
                .placeholder_key("idle_hint")
        }));
        cx.notify();
    }

    /// Reconnect at startup so the user lands straight in their session.
    ///
    /// `PIDO_AUTOCONNECT` (used by automated checks) overrides the directory;
    /// otherwise the last project from settings is used, resuming its last
    /// session. First run, or a project directory that has vanished, falls
    /// through to the welcome screen.
    pub fn autoconnect(&mut self, cx: &mut Context<Self>) {
        let (dir, resume): (Option<std::path::PathBuf>, Option<String>) =
            match std::env::var("PIDO_AUTOCONNECT") {
                Ok(dir) => (Some(std::path::PathBuf::from(dir)), None),
                Err(_) => {
                    let settings = crate::settings::Settings::load();
                    log::info!(
                        "autoconnect: last_project={:?} last_session={:?}",
                        settings.last_project,
                        settings.last_session
                    );
                    let dir = settings.last_project.map(std::path::PathBuf::from);
                    let resume = settings
                        .last_session
                        .filter(|session| std::path::Path::new(session).is_file());
                    (dir, resume)
                }
            };
        // PIDO_EXTRA_ARGS passes raw pi flags (e.g. `-e ./my-ext.ts`), for
        // trying extensions without a settings UI.
        let extra: Vec<String> = std::env::var("PIDO_EXTRA_ARGS")
            .map(|args| args.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(path) = dir {
            self.state
                .update(cx, |s, cx| s.connect(path, resume, extra, cx));
        }
    }
}

impl Render for PidoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.colors.bg, theme.colors.fg);
        let bounds = window.bounds();
        self.window_size = Some((f32::from(bounds.size.width), f32::from(bounds.size.height)));
        let state = self.state.clone();
        let live = state.read(cx).conn == Conn::Live;

        if live && !self.composer_focused {
            if let Some(draft) = &self.draft {
                let focus = draft.read(cx).focus().clone();
                window.focus(&focus, cx);
            }
            self.composer_focused = true;
        } else if !live {
            self.composer_focused = false;
        }

        let main: gpui::AnyElement = if live {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(div().flex_none().child(chat::render_header(&state, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(chat::render_transcript(&state, cx)),
                )
                .child(
                    div()
                        .flex_none()
                        .px_4()
                        .pt_2()
                        .pb_2()
                        .child(composer::render(&state, self, cx)),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .min_h_0()
                .child(welcome::render(&state, self, window, cx))
                .into_any_element()
        };

        // The extension dialog rides at the root, above every pane.
        let overlay = self.extension_dialog_overlay(&state, window, cx);

        // The FocusScope must carry the window size itself: a percentage
        // height inside an unsized root resolves against auto, so the whole
        // tree would otherwise grow to content height and push the composer
        // below the window.
        FocusScope::new(&self.focus)
            .root()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .id("pido-root")
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .bg(bg)
                    .text_color(fg)
                    .child(sidebar::render(&state, window, cx))
                    .child(main)
                    .children(overlay)
                    // Esc stops a running agent (clear queue + abort, the
                    // protocol's recommended Esc semantics). TextInput does not
                    // consume Escape, so this also works while the composer is
                    // focused.
                    .on_action(cx.listener(|this, _: &Stop, _window, cx| {
                        if this.state.read(cx).streaming {
                            this.state.update(cx, |s, cx| s.stop(cx));
                        }
                    })),
            )
    }
}

impl PidoApp {
    /// The native dialog for a pending extension request, if any. The text
    /// field for input/editor is created here, where a Window is available,
    /// and rebuilt only when the request changes.
    fn extension_dialog_overlay(
        &mut self,
        state: &gpui::Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use ely_gpui_component::buttons::{Button, ButtonVariant};
        use ely_gpui_component::overlays::{ConfirmDialog, Dialog};

        let request = state.read(cx).pending_dialog.clone()?;

        // Keep one field alive per request; rebuild when the request changes.
        if self.dialog_field_for.as_deref() != Some(request.id.as_str()) {
            self.dialog_field_for = Some(request.id.clone());
            let field = cx.new(|cx| {
                let mut input = match request.method {
                    pi_rpc::protocol::ExtensionUiMethod::Editor => {
                        TextInput::new(window, cx).multi_line(6, 24)
                    }
                    _ => TextInput::new(window, cx),
                };
                if let Some(prefill) = &request.prefill {
                    let prefill = prefill.clone();
                    input.set_text(prefill, cx);
                }
                input
            });
            self.dialog_field = Some(field.clone());

            // Enter (or the multi-line submit key) answers the dialog; this
            // is the same path the buttons take.
            let submit_state = state.clone();
            let submit_field = field.clone();
            let submit_id = request.id.clone();
            cx.subscribe(&field, move |_, _field, event: &InputEvent, cx| {
                if *event != InputEvent::Submit {
                    return;
                }
                let text = submit_field.read(cx).text().to_string();
                submit_state.update(cx, |s, cx| s.answer_dialog_value(&submit_id, text, cx));
            })
            .detach();
        }

        let title = request.title.clone().unwrap_or_else(|| {
            match request.method {
                pi_rpc::protocol::ExtensionUiMethod::Select => crate::i18n::t(cx, "ext_select"),
                pi_rpc::protocol::ExtensionUiMethod::Confirm => crate::i18n::t(cx, "ext_confirm"),
                pi_rpc::protocol::ExtensionUiMethod::Input => crate::i18n::t(cx, "ext_input"),
                pi_rpc::protocol::ExtensionUiMethod::Editor => crate::i18n::t(cx, "ext_editor"),
                _ => crate::i18n::t(cx, "extension_notify"),
            }
            .to_string()
        });

        let field = self.dialog_field.clone()?;
        let cancel_state = state.clone();
        let cancel_id = request.id.clone();
        let shut = move |_: &mut Window, cx: &mut App| {
            cancel_state.update(cx, |s, cx| s.cancel_dialog(&cancel_id, cx));
        };

        let cancel_label = crate::i18n::t(cx, "ext_cancel");
        let cancel_label_for_closure = cancel_label.clone();
        let submit_label = crate::i18n::t(cx, "ext_submit");
        let confirm_label = crate::i18n::t(cx, "ext_confirm");

        let overlay: gpui::AnyElement = match request.method {
            pi_rpc::protocol::ExtensionUiMethod::Select => {
                let mut list = div().flex().flex_col().gap_2();
                for (ix, option) in request
                    .options
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                {
                    let answer_state = state.clone();
                    let id = request.id.clone();
                    let value = option.clone();
                    list = list.child(div().w_full().child(
                        Button::new(("ext-option", ix), option).on_click(move |_, _, cx| {
                            answer_state
                                .update(cx, |s, cx| s.answer_dialog_value(&id, value.clone(), cx));
                        }),
                    ))
                }
                let mut select_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                );
                if let Some(message) = request.message.clone() {
                    select_dialog = select_dialog.detail(message);
                }
                select_dialog
                    .child(list)
                    .action(move |close| {
                        Button::new("ext-select-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .into_any_element()
            }
            pi_rpc::protocol::ExtensionUiMethod::Confirm => {
                let confirm_state = state.clone();
                let confirm_id = request.id.clone();
                ConfirmDialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    request.message.clone().unwrap_or_default(),
                    shut,
                )
                .confirm(confirm_label)
                .on_confirm(move |_, cx| {
                    confirm_state
                        .update(cx, |s, cx| s.answer_dialog_confirm(&confirm_id, true, cx));
                })
                .into_any_element()
            }
            pi_rpc::protocol::ExtensionUiMethod::Input => {
                // A hand-built dialog rather than PromptDialog: that one
                // closes (which reads as a cancel) *before* handing over the
                // text, so every answer arrived as `cancelled`.
                let submit_state = state.clone();
                let submit_field = field.clone();
                let submit_id = request.id.clone();
                let submit_label = submit_label.clone();
                let cancel_label = cancel_label_for_closure.clone();
                let field_focus = field.read(cx).focus().clone();
                let mut input_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                )
                .focus_first(field_focus);
                if let Some(placeholder) = request.placeholder.clone() {
                    input_dialog = input_dialog.detail(placeholder);
                }
                input_dialog
                    .child(field)
                    .action(move |close| {
                        Button::new("ext-input-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .action(move |close| {
                        Button::new("ext-input-submit", submit_label.clone())
                            .primary()
                            .on_click(move |_, window, cx| {
                                let text = submit_field.read(cx).text().to_string();
                                submit_state.update(cx, |s, cx| {
                                    s.answer_dialog_value(&submit_id, text, cx)
                                });
                                close(window, cx);
                            })
                    })
                    .into_any_element()
            }
            pi_rpc::protocol::ExtensionUiMethod::Editor => {
                let submit_state = state.clone();
                let submit_field = field.clone();
                let submit_id = request.id.clone();
                let mut editor_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                );
                if let Some(message) = request.message.clone() {
                    editor_dialog = editor_dialog.detail(message);
                }
                editor_dialog
                    .child(div().min_h(gpui::px(160.)).child(field))
                    .action(move |close| {
                        Button::new("ext-editor-cancel", cancel_label)
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .action(move |close| {
                        Button::new("ext-editor-submit", submit_label)
                            .primary()
                            .on_click(move |_, window, cx| {
                                let text = submit_field.read(cx).text().to_string();
                                submit_state.update(cx, |s, cx| {
                                    s.answer_dialog_value(&submit_id, text, cx)
                                });
                                close(window, cx);
                            })
                    })
                    .into_any_element()
            }
            _ => return None,
        };
        Some(overlay)
    }
}

/// A stable numeric slot for a dialog element id, from its request id.
fn dialog_slot(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// Toggle light/dark (for the header button) and persist the choice.
pub fn toggle_theme(cx: &mut App) {
    let next = if cx.theme().is_dark() {
        Mode::Light
    } else {
        Mode::Dark
    };
    crate::settings::save_setting(|s| {
        s.theme = Some(if next == Mode::Light { "light" } else { "dark" }.to_string());
    });
    Theme::set_mode(next, cx);
}
