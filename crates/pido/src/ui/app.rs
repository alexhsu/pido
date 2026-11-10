//! The root view: Ely's FocusScope wrapping a sidebar and the session pane.

use ely_gpui_component::forms::TextInput;
use ely_gpui_component::primitives::FocusScope;
use ely_gpui_component::theme::{ActiveTheme, Mode, Theme};
use gpui::{
    App, AppContext, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, actions, div,
};

use crate::state::{AppState, Conn};
use crate::ui::{chat, composer, sidebar, welcome};

actions!(pido, [Stop]);

pub struct PidoApp {
    pub focus: FocusHandle,
    pub state: gpui::Entity<AppState>,
    /// The composer's multi-line field, owned here so it survives re-renders.
    pub draft: Option<gpui::Entity<TextInput>>,
    /// The welcome screen's project-directory field, created lazily.
    pub welcome_dir: Option<gpui::Entity<TextInput>>,
    /// Last observed window size, persisted on quit.
    window_size: Option<(f32, f32)>,
}

impl PidoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let state = AppState::new(cx);
        // Re-render whenever the state entity changes.
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let app = Self {
            focus,
            state,
            draft: None,
            welcome_dir: None,
            window_size: None,
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
                .placeholder(crate::i18n::t(cx, "idle_hint"))
        }));
        cx.notify();
    }

    /// Start a session immediately when `PIDO_AUTOCONNECT` names a directory.
    ///
    /// Used by automated checks, and handy as a future deep link.
    pub fn autoconnect(&mut self, cx: &mut Context<Self>) {
        if let Ok(dir) = std::env::var("PIDO_AUTOCONNECT") {
            let path = std::path::PathBuf::from(dir);
            self.state.update(cx, |s, cx| s.connect(path, cx));
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

        let main: gpui::AnyElement = if live {
            // Ely's ChatContainer supplies the canonical chat layout: a
            // full-height message column with the composer centered and
            // width-constrained beneath it.
            div()
                .flex_1()
                .flex()
                .flex_col()
                .min_w_0()
                .child(chat::render_header(&state, cx))
                .child(
                    ely_gpui_component::chat::ChatContainer::new(chat::render_transcript(
                        &state, window, cx,
                    ))
                    .composer(composer::render(&state, self, cx)),
                )
                .into_any_element()
        } else {
            welcome::render(&state, self, window, cx).into_any_element()
        };

        FocusScope::new(&self.focus).root().child(
            div()
                .id("pido-root")
                .size_full()
                .flex()
                .flex_row()
                .bg(bg)
                .text_color(fg)
                .child(sidebar::render(&state, cx))
                .child(main)
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
