//! The sidebar: new-chat button plus past sessions grouped by project.

use ely_gpui_component::buttons::Button;
use ely_gpui_component::layout::ScrollArea;
use ely_gpui_component::theme::ActiveTheme;
use ely_gpui_component::typography::{Caption, Label};
use gpui::{
    App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};

use crate::i18n;
use crate::state::{AppState, Conn};

/// Render the whole sidebar. Session clicks switch the running pi process to
/// that session file.
pub fn render(state: &Entity<AppState>, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let app = state.clone();
    let s = state.read(cx);
    let live = s.conn == Conn::Live;

    let groups: Vec<(SharedString, Vec<SessionRow>)> = s
        .session_groups
        .iter()
        .map(|group| {
            (
                shorten_path(&group.cwd).into(),
                group
                    .sessions
                    .iter()
                    .map(|session| SessionRow {
                        path: session.path.to_string_lossy().into_owned(),
                        title: session
                            .name
                            .clone()
                            .or_else(|| session.preview.clone())
                            .unwrap_or_else(|| session.session_id.clone()),
                        meta: format!(
                            "{} · {}",
                            session
                                .timestamp
                                .as_deref()
                                .and_then(|t| t.get(..10))
                                .unwrap_or(""),
                            session.message_count
                        ),
                    })
                    .collect(),
            )
        })
        .collect();

    div()
        .w(px(264.))
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .p_3()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(Label::new(i18n::t(cx, "app_name")))
                        .child(Caption::new(pi_status_line(state, cx))),
                )
                .child(Button::new("new-chat", i18n::t(cx, "new_chat")).on_click({
                    let app = app.clone();
                    move |_, _, cx| app.update(cx, |s, cx| s.new_chat(cx))
                })),
        )
        .child(
            ScrollArea::new("sessions").vertical().child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(Caption::new(i18n::t(cx, "sessions")))
                    .children(groups.into_iter().map(|(cwd, sessions)| {
                        let app = app.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(Caption::new(cwd))
                            .children(sessions.into_iter().enumerate().map(move |(ix, row)| {
                                let app = app.clone();
                                div()
                                    .id(("session", ix as u64))
                                    .px_2()
                                    .py_1p5()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.colors.hover))
                                    .on_click({
                                        let app = app.clone();
                                        let path = row.path.clone();
                                        move |_, _, cx| {
                                            app.update(cx, |s, cx| {
                                                s.switch_session(
                                                    std::path::PathBuf::from(&path),
                                                    cx,
                                                )
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .min_w_0()
                                            .child(div().truncate().child(Label::new(row.title)))
                                            .child(div().truncate().child(Caption::new(row.meta))),
                                    )
                            }))
                    })),
            ),
        )
        .when(!live, |this| {
            // Sessions can only be opened while a pi process is live; the
            // welcome screen is where one is started.
            this
        })
}

struct SessionRow {
    path: String,
    title: String,
    meta: String,
}

/// One-line connection status for the sidebar header.
fn pi_status_line(state: &Entity<AppState>, cx: &App) -> SharedString {
    let s = state.read(cx);
    match &s.conn {
        Conn::Idle | Conn::Connecting => i18n::t(cx, "disconnected"),
        Conn::Live => match (&s.current_model, &s.session_name) {
            (Some(model), Some(name)) => format!("{name} · {}", model.name).into(),
            (Some(model), None) => model.name.clone().into(),
            (None, Some(name)) => name.clone().into(),
            (None, None) => "pi".into(),
        },
        Conn::Dead(_) => i18n::t(cx, "connection_failed"),
    }
}

/// `D:\develop\code\pido` → `code / pido`, to keep group headers short.
fn shorten_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    match parts.len() {
        0 => path.to_string(),
        1 => parts[0].to_string(),
        n => format!("{} / {}", parts[n - 2], parts[n - 1]),
    }
}
