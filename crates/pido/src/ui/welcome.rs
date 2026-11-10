//! The welcome screen on Ely's `WelcomeScreen`: a time-of-day greeting over
//! environment checks, a project-directory picker, and the start button.
//!
//! This pane only appears on first run, when the last project directory has
//! vanished, or after a failed connection — normally the app reconnects to
//! the last project automatically at startup.

use ely_gpui_component::buttons::Button;
use ely_gpui_component::forms::{PathInput, TextInput};
use ely_gpui_component::theme::ActiveTheme;
use ely_gpui_component::typography::format::{now, system_zone};
use ely_gpui_component::typography::{Caption, Paragraph};
use gpui::{
    App, AppContext, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use crate::i18n;
use crate::state::{AppState, Conn};
use crate::ui::chat;

/// Render the welcome pane shown before a session starts.
pub fn render(
    state: &gpui::Entity<AppState>,
    app: &mut crate::ui::app::PidoApp,
    window: &mut Window,
    cx: &mut Context<crate::ui::app::PidoApp>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (danger_fill, danger_ink) = (theme.colors.danger_subtle, theme.colors.danger);
    let (connecting, dead_reason, last_project, notice) = {
        let s = state.read(cx);
        (
            s.conn == Conn::Connecting,
            match &s.conn {
                Conn::Dead(reason) => Some(reason.clone()),
                _ => None,
            },
            s.last_project.clone(),
            s.notice.clone(),
        )
    };

    // The path field needs a Window, so it is created lazily on first render.
    let dir_field = app.welcome_dir.clone().unwrap_or_else(|| {
        let cwd_guess = last_project
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let field = cx.new(|cx| {
            let mut input = TextInput::new(window, cx).placeholder("D:\\develop\\my-project");
            input.set_text(cwd_guess, cx);
            input
        });
        // Re-render the welcome pane as the path changes so the start button
        // and validation stay in sync with the field.
        cx.observe(&field, |_, _, cx| cx.notify()).detach();
        app.welcome_dir = Some(field.clone());
        field
    });
    let typed = dir_field.read(cx).text().trim().to_string();
    let typed_invalid = !typed.is_empty() && !std::path::Path::new(&typed).is_dir();
    let next_locale = i18n::other_locale(cx);

    let start_state = state.clone();
    let dir_for_start = dir_field.clone();

    // Ely's WelcomeScreen greets in hardcoded English with no locale hook,
    // so the same layout (centered container column, sparkles mark, Xxl
    // greeting) is composed here with the greeting from our i18n catalog.
    let theme = cx.theme();
    let column = theme.container_width(ely_gpui_component::theme::ContainerSize::Md);

    div()
        .id("welcome")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .items_center()
        .p_6()
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .w_full()
                .max_w(column)
                .flex()
                .flex_col()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            ely_gpui_component::buttons::Button::new(
                                "welcome-theme",
                                i18n::t(cx, "theme"),
                            )
                            .on_click(|_, _, cx| crate::ui::app::toggle_theme(cx)),
                        )
                        .child(
                            ely_gpui_component::buttons::Button::new("welcome-locale", next_locale)
                                .on_click(move |_, _, cx| {
                                    crate::settings::save_setting(|s| {
                                        s.locale = Some(next_locale.to_string());
                                    });
                                    ely_gpui_component::i18n::I18n::set_locale(next_locale, cx);
                                }),
                        ),
                )
                .child(
                    // The wordmark says what this is; the time of day greets
                    // underneath it.
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .w_full()
                        .child(
                            ely_gpui_component::primitives::Image::new(
                                "pido-wordmark",
                                crate::brand::wordmark(2),
                            )
                            .fit(gpui::ObjectFit::Contain)
                            .w(gpui::px(220.))
                            .h(gpui::px(101.)),
                        )
                        .child(
                            div()
                                .text_size(theme.text_size(ely_gpui_component::theme::TextSize::Lg))
                                .text_color(theme.colors.fg_muted)
                                .child(greeting(cx)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_5()
                        .w_full()
                        .child(Caption::new(i18n::t(cx, "welcome_desc")))
                        .child(render_environment(state, cx))
                        .when_some(notice, |this, (kind, text)| {
                            this.child(chat::render_notice(kind, text, state.clone(), cx))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(Caption::new(i18n::t(cx, "project_dir")))
                                .child(PathInput::new("project-dir", &dir_field).directories())
                                .when(typed_invalid, |this| {
                                    this.child(
                                        div()
                                            .text_color(danger_ink)
                                            .child(Caption::new(i18n::t(cx, "dir_invalid"))),
                                    )
                                }),
                        )
                        .child(
                            Button::new(
                                "start",
                                if connecting {
                                    i18n::t(cx, "starting")
                                } else {
                                    i18n::t(cx, "start")
                                },
                            )
                            .disabled(connecting || typed.is_empty() || typed_invalid)
                            .on_click(move |_, _, cx| {
                                let path = std::path::PathBuf::from(
                                    dir_for_start.read(cx).text().trim().to_string(),
                                );
                                start_state
                                    .update(cx, |s, cx| s.connect(path, None, Vec::new(), cx));
                            }),
                        )
                        .when_some(dead_reason, |this, reason| {
                            let reset_state = state.clone();
                            let retry_state = state.clone();
                            this.child(banner(
                                danger_fill,
                                danger_ink,
                                i18n::t(cx, "connection_failed"),
                                reason.into(),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(Button::new("retry", i18n::t(cx, "retry")).on_click(
                                        move |_, _, cx| retry_state.update(cx, |s, cx| s.retry(cx)),
                                    ))
                                    .child(
                                        Button::new("reconnect", i18n::t(cx, "reconnect"))
                                            .on_click(move |_, _, cx| {
                                                reset_state.update(cx, |s, cx| s.disconnect(cx));
                                            }),
                                    ),
                            )
                        }),
                ),
        )
        .child(div().flex_1())
}

/// The time-of-day greeting in the active locale.
fn greeting(cx: &App) -> SharedString {
    let hour = now().to_zoned(system_zone("pido")).hour();
    let key = match hour {
        5..=11 => "greeting_morning",
        12..=17 => "greeting_afternoon",
        _ => "greeting_evening",
    };
    i18n::t(cx, key)
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

fn banner(fill: Hsla, ink: Hsla, title: SharedString, detail: SharedString) -> impl IntoElement {
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
