//! The sidebar: new-chat button plus past sessions grouped by project.
//!
//! Row styling mirrors ely-chat's `ConversationItem` (control-height row,
//! rounded hover/active background, ellipsized title, muted ink) with one
//! addition the component has no slot for: a relative age on the right.
//! Grouping is by project, matching pido's model, rather than the
//! component's built-in day buckets.

use ely_gpui_component::buttons::Button;
use ely_gpui_component::layout::ScrollArea;
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize};
use ely_gpui_component::typography::{Caption, Ellipsis, Label};
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
    let colors = theme.colors.clone();
    let (control_h, radius_md, text_sm, text_xs) = (
        theme.control_height(ControlSize::Md),
        theme.radius(Radius::Md),
        theme.text_size(TextSize::Sm),
        theme.text_size(TextSize::Xs),
    );
    let app = state.clone();
    let s = state.read(cx);
    let active_file = s.session_file.clone();

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
                        age: i18n::relative_time(cx, session.updated_secs),
                        active: active_file.as_deref().is_some_and(|active| {
                            same_file(active, &session.path.to_string_lossy())
                        }),
                    })
                    .collect(),
            )
        })
        .collect();
    let empty = groups.is_empty();

    div()
        .w(px(264.))
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(colors.border)
        .bg(colors.surface)
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
            // The list must be the only growing part and stay inside the
            // window; without a bound its content stretches the whole row and
            // the session pane's composer lands below the window.
            div().flex_1().min_h_0().min_w_0().child(
                ScrollArea::new("sessions").vertical().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .when(empty, |this| {
                            this.child(Caption::new(i18n::t(cx, "no_sessions")))
                        })
                        .children(groups.into_iter().enumerate().map(
                            |(group_ix, (cwd, sessions))| {
                                let app = app.clone();
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    // Project header: a folder mark and the
                                    // directory's last two segments.
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .px_2()
                                            .pb_1()
                                            .child(
                                                Icon::new(IconName::Folder)
                                                    .size(IconSize::Sm)
                                                    .color(colors.fg_subtle),
                                            )
                                            .child(Label::new(cwd)),
                                    )
                                    .children(sessions.into_iter().enumerate().map(
                                        move |(row_ix, row)| {
                                            let SessionRow {
                                                path,
                                                title,
                                                age,
                                                active,
                                            } = row;
                                            let app = app.clone();
                                            div()
                                                .id((
                                                    "session",
                                                    (group_ix as u64) * 1_000_000 + row_ix as u64,
                                                ))
                                                .role(gpui::Role::ListItem)
                                                .aria_label(title.clone())
                                                .aria_selected(active)
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .h(control_h)
                                                .pl_2()
                                                .pr_0p5()
                                                .rounded(radius_md)
                                                .text_size(text_sm)
                                                .text_color(if active {
                                                    colors.fg
                                                } else {
                                                    colors.fg_muted
                                                })
                                                .when(active, |row| row.bg(colors.active))
                                                .when(!active, |row| {
                                                    row.cursor_pointer()
                                                        .hover(|style| style.bg(colors.hover))
                                                })
                                                .on_click(move |_, _, cx| {
                                                    app.update(cx, |s, cx| {
                                                        s.switch_session(
                                                            std::path::PathBuf::from(&path),
                                                            cx,
                                                        )
                                                    });
                                                })
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .child(Ellipsis::new(title.clone())),
                                                )
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .text_size(text_xs)
                                                        .text_color(colors.fg_subtle)
                                                        .child(age),
                                                )
                                        },
                                    ))
                            },
                        )),
                ),
            ),
        )
}

struct SessionRow {
    path: String,
    title: String,
    age: SharedString,
    active: bool,
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

/// Compare two session paths without caring about separator style.
fn same_file(a: &str, b: &str) -> bool {
    std::path::Path::new(a) == std::path::Path::new(b)
        || a.replace('\\', "/") == b.replace('\\', "/")
}
