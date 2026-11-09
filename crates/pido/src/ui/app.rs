//! The root view: Ely's FocusScope wrapping a sidebar and the session pane.

use ely_gpui_component::forms::TextInput;
use ely_gpui_component::primitives::FocusScope;
use ely_gpui_component::theme::{ActiveTheme, Mode, Theme};
use gpui::{
    App, AppContext, Context, FocusHandle, IntoElement, ParentElement, Render, Styled, Window,
    div,
};

use crate::state::{AppState, Conn};
use crate::ui::{chat, composer, sidebar, welcome};

pub struct PidoApp {
    pub focus: FocusHandle,
    pub state: gpui::Entity<AppState>,
    /// The composer's multi-line field, owned here so it survives re-renders.
    pub draft: Option<gpui::Entity<TextInput>>,
    /// The welcome screen's project-directory field, created lazily.
    pub welcome_dir: Option<gpui::Entity<TextInput>>,
}

impl PidoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let state = AppState::new(cx);
        // Re-render whenever the state entity changes.
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            focus,
            state,
            draft: None,
            welcome_dir: None,
        }
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
        let state = self.state.clone();
        let live = state.read(cx).conn == Conn::Live;

        let main: gpui::AnyElement = if live {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .min_w_0()
                .child(chat::render_header(&state, cx))
                .child(chat::render_transcript(&state, window, cx))
                .child(composer::render(&state, self, cx))
                .into_any_element()
        } else {
            welcome::render(&state, self, window, cx).into_any_element()
        };

        FocusScope::new(&self.focus)
            .root()
            .size_full()
            .flex()
            .flex_row()
            .bg(bg)
            .text_color(fg)
            .child(sidebar::render(&state, cx))
            .child(main)
    }
}

/// Toggle light/dark (for the header button).
pub fn toggle_theme(cx: &mut App) {
    let next = if cx.theme().is_dark() {
        Mode::Light
    } else {
        Mode::Dark
    };
    Theme::set_mode(next, cx);
}
