//! The welcome screen: environment checks, a project-directory picker, and
//! the start button.

use ely_gpui_component::buttons::Button;
use ely_gpui_component::forms::{PathInput, TextInput};
use ely_gpui_component::theme::ActiveTheme;
use ely_gpui_component::typography::{Caption, Paragraph, Title};
use gpui::{
    AppContext, Context, Hsla, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use crate::i18n;
use crate::state::{AppState, Conn};

/// Render the welcome pane shown before a session starts.
pub fn render(
    state: &gpui::Entity<AppState>,
    app: &mut crate::ui::app::PidoApp,
    window: &mut Window,
    cx: &mut Context<crate::ui::app::PidoApp>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (danger_fill, danger_ink) = (theme.colors.danger_subtle, theme.colors.danger);
    let s = state.read(cx);
    let connecting = s.conn == Conn::Connecting;
    let dead_reason = match &s.conn {
        Conn::Dead(reason) => Some(reason.clone()),
        _ => None,
    };

    // The path field needs a Window, so it is created lazily on first render.
    let dir_field = app.welcome_dir.clone().unwrap_or_else(|| {
        let cwd_guess = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let field = cx.new(|cx| {
            let mut input = TextInput::new(window, cx).placeholder("D:\\develop\\my-project");
            input.set_text(cwd_guess, cx);
            input
        });
        app.welcome_dir = Some(field.clone());
        field
    });
    let typed = dir_field.read(cx).text().trim().to_string();

    let start_state = state.clone();
    let dir_for_start = dir_field.clone();

    div()
        .flex_1()
        .flex()
        .items_start()
        .justify_center()
        .py_16()
        .child(
            div()
                .w(px(520.))
                .flex()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(Title::new(i18n::t(cx, "welcome_title")))
                        .child(Caption::new(i18n::t(cx, "welcome_desc"))),
                )
                .child(render_environment(state, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(Caption::new(i18n::t(cx, "project_dir")))
                        .child(PathInput::new("project-dir", &dir_field).directories()),
                )
                .child(Button::new(
                    "start",
                    if connecting {
                        i18n::t(cx, "starting")
                    } else {
                        i18n::t(cx, "start")
                    },
                )
                .disabled(connecting || typed.is_empty())
                .on_click(move |_, _, cx| {
                    let path =
                        std::path::PathBuf::from(dir_for_start.read(cx).text().trim().to_string());
                    start_state.update(cx, |s, cx| s.connect(path, cx));
                }))
                .when_some(dead_reason, |this, reason| {
                    let reset_state = state.clone();
                    this.child(banner(
                        danger_fill,
                        danger_ink,
                        i18n::t(cx, "connection_failed"),
                        reason.into(),
                    ))
                    .child(
                        ely_gpui_component::buttons::Button::new(
                            "reconnect",
                            i18n::t(cx, "reconnect"),
                        )
                        .on_click(move |_, _, cx| {
                            reset_state.update(cx, |s, cx| s.disconnect(cx));
                        }),
                    )
                }),
        )
}

/// Environment status: pi install and provider auth.
fn render_environment(
    state: &gpui::Entity<AppState>,
    cx: &Context<crate::ui::app::PidoApp>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let s = state.read(cx);

    let (fill, ink, title, detail): (Hsla, Hsla, SharedString, SharedString) =
        if s.environment.pi_version.is_none() {
            (
                theme.colors.danger_subtle,
                theme.colors.danger,
                i18n::t(cx, "pi_missing"),
                i18n::t(cx, "pi_missing_desc"),
            )
        } else if s.environment.ready_providers.is_empty() {
            (
                theme.colors.warning_subtle,
                theme.colors.warning,
                i18n::t(cx, "auth_missing"),
                i18n::t(cx, "auth_missing_desc"),
            )
        } else {
            (
                theme.colors.success_subtle,
                theme.colors.success,
                format!(
                    "pi {} · {}",
                    s.environment.pi_version.as_deref().unwrap_or_default(),
                    s.environment.ready_providers.join(", ")
                )
                .into(),
                SharedString::default(),
            )
        };
    banner(fill, ink, title, detail)
}

fn banner(
    fill: Hsla,
    ink: Hsla,
    title: SharedString,
    detail: SharedString,
) -> impl IntoElement {
    let has_detail = !detail.as_ref().is_empty();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(fill)
        .text_color(ink)
        .child(Paragraph::new(title))
        .when(has_detail, |this| this.child(Caption::new(detail)))
}
