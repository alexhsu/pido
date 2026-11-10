//! The root view: Ely's FocusScope wrapping a sidebar and the session pane.

use ely_gpui_component::forms::{InputEvent, TextInput};
use ely_gpui_component::layout::SplitPane;
use ely_gpui_component::primitives::FocusScope;
use ely_gpui_component::theme::{ActiveTheme, ContainerSize, Mode, Theme};
use gpui::{
    App, AppContext, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, actions, div,
};

use crate::state::{AppState, Conn};
use crate::ui::{chat, composer, sidebar, welcome};

actions!(
    pido,
    [Stop, PinSession, RenameSession, ForkSession, ArchiveSession]
);

/// Height of the session header above the chat pane.
pub const HEADER_HEIGHT: f32 = 52.;

pub struct PidoApp {
    /// Sidebar share of the window, seeded once from settings so a drag stays
    /// put across frames (SplitPane replaces its state when the seed changes).
    split_seed: Vec<f32>,
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
    /// The session-rename dialog: open, with its own field, and the session
    /// it names (None means the open session without a file yet).
    rename_open: bool,
    rename_field: Option<gpui::Entity<TextInput>>,
    rename_target: Option<String>,
    /// The changes panel: open, which file is picked, and its diff layout.
    pub diff_open: bool,
    pub diff_selected: Option<String>,
    pub diff_layout: ely_gpui_component::git::DiffLayout,
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
        let sidebar = crate::settings::Settings::load()
            .sidebar_width
            .unwrap_or(264.0)
            .clamp(200.0, 480.0);
        let window_width = 1280.0;
        let share = (sidebar / window_width).clamp(0.1, 0.5);
        let app = Self {
            split_seed: vec![share, 1.0 - share],
            focus,
            state,
            draft: None,
            welcome_dir: None,
            window_size: None,
            composer_focused: false,
            dialog_field: None,
            dialog_field_for: None,
            rename_open: false,
            rename_field: None,
            rename_target: None,
            diff_open: false,
            diff_selected: None,
            diff_layout: ely_gpui_component::git::DiffLayout::Unified,
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
        let column = theme.container_width(ContainerSize::Md);
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
                .child(
                    div()
                        .flex_none()
                        .child(chat::render_header(&state, &cx.entity(), cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(chat::render_transcript(&state, cx)),
                )
                .child(
                    // The composer sits in the same centered column as the
                    // transcript, so the field lines up with the messages.
                    div()
                        .flex_none()
                        .w_full()
                        .flex()
                        .justify_center()
                        .px_4()
                        .pt_2()
                        .pb_2()
                        .child(
                            div()
                                .w_full()
                                .max_w(column)
                                .min_w_0()
                                .child(composer::render(&state, self, cx)),
                        ),
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

        // Extension and rename dialogs ride at the root, above every pane.
        let overlay = self
            .extension_dialog_overlay(&state, window, cx)
            .or_else(|| self.rename_dialog(cx));
        // The changes panel slides over the chat; it closes itself when a new
        // session empties the list.
        let changes_open = self.diff_open && !state.read(cx).changes.is_empty();
        self.diff_open = changes_open;
        let changes = changes_open.then(|| {
            crate::ui::changes::render(
                &state,
                &cx.entity(),
                self.diff_selected.clone(),
                self.diff_layout,
                cx,
            )
            .into_any_element()
        });

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
                    .relative()
                    .flex()
                    .flex_row()
                    .bg(bg)
                    .text_color(fg)
                    .child(
                        SplitPane::new("shell", gpui::Axis::Horizontal, gpui::px(200.))
                            .sizes(&self.split_seed)
                            .pane(sidebar::render(&state, &cx.entity(), window, cx))
                            .pane(main)
                            .on_resize({
                                let width = f32::from(window.bounds().size.width);
                                move |shares, _, _| {
                                    // Shares are fractions of the window; the
                                    // sidebar's own width is what to keep.
                                    let sidebar = shares[0] * width;
                                    crate::settings::save_setting(|s| {
                                        s.sidebar_width = Some(sidebar);
                                    });
                                }
                            }),
                    )
                    // Dialogs sit above the review panel: an extension asking
                    // for permission must not hide behind it.
                    .children(changes)
                    .children(overlay)
                    // Esc stops a running agent (clear queue + abort, the
                    // protocol's recommended Esc semantics). TextInput does not
                    // consume Escape, so this also works while the composer is
                    // focused.
                    .on_action(cx.listener(|this, _: &Stop, _window, cx| {
                        // Esc backs out of the review before it stops a run.
                        if this.diff_open {
                            this.diff_open = false;
                            cx.notify();
                            return;
                        }
                        if this.state.read(cx).streaming {
                            this.state.update(cx, |s, cx| s.stop(cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PinSession, _window, cx| {
                        if let Some(file) = this.state.read(cx).session_file.clone() {
                            this.state.update(cx, |s, cx| s.toggle_pin(&file, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &RenameSession, window, cx| {
                        if this.state.read(cx).conn == Conn::Live {
                            this.open_rename(window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ForkSession, _window, cx| {
                        if let Some(file) = this.state.read(cx).session_file.clone() {
                            this.state.update(cx, |s, cx| s.fork_session(&file, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ArchiveSession, _window, cx| {
                        if let Some(file) = this.state.read(cx).session_file.clone() {
                            this.state.update(cx, |s, cx| s.toggle_archive(&file, cx));
                        }
                    })),
            )
    }
}

impl PidoApp {
    /// Open the rename dialog over the current session's name.
    pub fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (target, current) = {
            let state = self.state.read(cx);
            (
                state.session_file.clone(),
                state.session_name.clone().unwrap_or_default(),
            )
        };
        self.open_rename_with(target, current, window, cx);
    }

    /// Open the rename dialog for a session picked in the sidebar: its own
    /// name prefills the field, and saving renames that session rather than
    /// whichever one is open.
    pub fn open_rename_at(
        &mut self,
        path: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefill = {
            let state = self.state.read(cx);
            let open = state.session_file.as_deref().is_some_and(|current| {
                crate::state::session_key(current) == crate::state::session_key(&path)
            });
            if open {
                state.session_name.clone().unwrap_or_default()
            } else {
                title
            }
        };
        self.open_rename_with(Some(path), prefill, window, cx);
    }

    fn open_rename_with(
        &mut self,
        target: Option<String>,
        prefill: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            if !prefill.is_empty() {
                input.set_text(prefill, cx);
            }
            input
        });
        self.rename_field = Some(field.clone());
        self.rename_open = true;
        self.rename_target = target;
        // Enter in the field saves, the same path the Save button takes.
        cx.subscribe(&field, move |this, field, event: &InputEvent, cx| {
            if *event != InputEvent::Submit {
                return;
            }
            this.finish_rename(&field, cx);
        })
        .detach();
        cx.notify();
    }

    /// Commit the rename dialog's field, then close it.
    fn finish_rename(&mut self, field: &gpui::Entity<TextInput>, cx: &mut Context<Self>) {
        let name = field.read(cx).text().to_string();
        match self.rename_target.clone() {
            Some(path) => self
                .state
                .update(cx, |state, cx| state.rename_session_at(&path, name, cx)),
            None => self
                .state
                .update(cx, |state, cx| state.rename_session(name, cx)),
        }
        self.rename_open = false;
        self.rename_field = None;
        self.rename_target = None;
        cx.notify();
    }

    /// The rename dialog, while open.
    fn rename_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::buttons::{Button, ButtonVariant};
        use ely_gpui_component::overlays::Dialog;

        if !self.rename_open {
            return None;
        }
        let field = self.rename_field.clone()?;
        let close_me = cx.entity();
        let shut = move |_: &mut Window, cx: &mut App| {
            close_me.update(cx, |this, cx| {
                this.rename_open = false;
                this.rename_field = None;
                this.rename_target = None;
                cx.notify();
            });
        };
        let save_field = field.clone();
        let save_me = cx.entity();
        // The dialog is rebuilt every frame, so focus goes back to the field
        // each time — otherwise the composer would keep the keystrokes.
        let focus = field.read(cx).focus().clone();
        // Labels resolve here: the action closures must be 'static, so they
        // cannot borrow `cx` to look a string up when they run.
        let title = crate::i18n::t(cx, "rename");
        let cancel_label = crate::i18n::t(cx, "ext_cancel");
        let save_label = crate::i18n::t(cx, "save");
        Some(
            Dialog::new("rename-dialog", title, shut)
                .focus_first(focus)
                .child(field)
                .action({
                    let me = cx.entity();
                    move |close| {
                        Button::new("rename-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| {
                                me.update(cx, |this, cx| {
                                    this.rename_open = false;
                                    this.rename_field = None;
                                    this.rename_target = None;
                                    cx.notify();
                                });
                                close(window, cx);
                            })
                    }
                })
                .action(move |close| {
                    Button::new("rename-save", save_label.clone())
                        .primary()
                        .on_click(move |_, window, cx| {
                            save_me.update(cx, |this, cx| this.finish_rename(&save_field, cx));
                            close(window, cx);
                        })
                })
                .into_any_element(),
        )
    }

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
