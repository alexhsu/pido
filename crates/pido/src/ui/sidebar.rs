//! The sidebar: new-chat button plus past sessions grouped by project.
//!
//! Row styling mirrors ely-chat's `ConversationItem` (control-height row,
//! rounded hover/active background, ellipsized title, muted ink) with one
//! addition the component has no slot for: a relative age on the right.
//! Grouping is by project, matching pido's model, rather than the
//! component's built-in day buckets.

use ely_gpui_component::buttons::Button;
use ely_gpui_component::lists::VirtualList;
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize};
use ely_gpui_component::typography::{Caption, Ellipsis, Label};
use gpui::{
    App, Entity, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div, prelude::FluentBuilder, px,
};

use crate::i18n;
use crate::state::{AppState, Conn, SidebarRow};

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
    let can_start = s.conn == Conn::Live && !s.streaming;
    let empty = s.sidebar_rows.is_empty();

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
                    // The wordmark carries the brand; connection and model
                    // details live in the session header instead.
                    ely_gpui_component::primitives::Image::new(
                        "pido-wordmark",
                        crate::brand::wordmark(theme.is_dark(), 1),
                    )
                    .fit(gpui::ObjectFit::Contain)
                    .w(px(72.))
                    .h(px(34.)),
                )
                .child(
                    Button::new("new-chat", i18n::t(cx, "new_chat"))
                        .disabled(!can_start)
                        .on_click({
                            let app = app.clone();
                            move |_, _, cx| app.update(cx, |s, cx| s.new_chat(cx))
                        }),
                ),
        )
        .child(
            // The list must be the only growing part and stay inside the
            // window; without a bound its content stretches the whole row and
            // the session pane's composer lands below the window.
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .when(empty, |this| {
                    this.child(Caption::new(i18n::t(cx, "no_sessions")))
                })
                .when(!empty, |this| {
                    // Only the rows in view are built: a project's whole
                    // history can be long, and this list redraws whenever the
                    // transcript does.
                    let rows = s.sidebar_rows.clone();
                    let app = app.clone();
                    let state = state.clone();
                    this.child(
                        VirtualList::new(
                            ("sessions", rows.len() as u64),
                            rows.len(),
                            move |ix, _, cx| {
                                let row = &rows[ix];
                                let colors = cx.theme().colors.clone();
                                match row {
                                    SidebarRow::Group { label } => div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .px_2()
                                        .pt_3()
                                        .pb_1()
                                        .child(
                                            Icon::new(IconName::Folder)
                                                .size(IconSize::Sm)
                                                .color(colors.fg_subtle),
                                        )
                                        .child(Label::new(label.clone()))
                                        .into_any_element(),
                                    SidebarRow::Session {
                                        path,
                                        title,
                                        updated,
                                    } => {
                                        let active = state
                                            .read(cx)
                                            .session_file
                                            .as_deref()
                                            .is_some_and(|open| same_file(open, path));
                                        let age = i18n::relative_time(cx, *updated);
                                        let app = app.clone();
                                        let path = path.clone();
                                        div()
                                            .id(("session", ix as u64))
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
                                                        std::path::PathBuf::from(path.to_string()),
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
                                            .into_any_element()
                                    }
                                }
                            },
                        )
                        .size_full(),
                    )
                }),
        )
}

/// Compare two session paths without caring about separator style; Windows
/// paths are also case-insensitive.
fn same_file(a: &str, b: &str) -> bool {
    let normalize = |path: &str| {
        let path = path.replace('\\', "/");
        if cfg!(windows) {
            path.to_lowercase()
        } else {
            path
        }
    };
    std::path::Path::new(a) == std::path::Path::new(b) || normalize(a) == normalize(b)
}
