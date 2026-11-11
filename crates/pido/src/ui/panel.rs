//! The right panel: the session's diff review and a live shell, in tabs.
//!
//! Lives inside the window's split (not an overlay), so it is resizable with
//! the same divider drag as the sidebar, and closing it hands the width back
//! to the chat. The terminal entity outlives the panel: hiding it stops
//! nothing, and a shell that exits is replaced on the next open.

use ely_gpui_component::buttons::SegmentedControl;
use ely_gpui_component::buttons::{Button, ButtonVariant, IconButton};
use ely_gpui_component::primitives::IconName;
use ely_gpui_component::theme::{ActiveTheme, ControlSize, TextSize};
use ely_gpui_component::typography::Caption;
use gpui::{
    App, Entity, InteractiveElement, IntoElement, ParentElement, Styled, div,
    prelude::FluentBuilder,
};

use crate::i18n;
use crate::state::AppState;
use crate::ui::app::{PidoApp, RightTab};

/// The panel's chrome and the active tab's content.
pub fn render(
    app: &PidoApp,
    view: &Entity<PidoApp>,
    state: &Entity<AppState>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let changes = state.read(cx).changes.len();
    let tab = app.right_tab;

    let tabs = SegmentedControl::new("right-tabs", tab_key(tab))
        .size(ControlSize::Sm)
        .segment(
            "diff",
            i18n::t_arg(cx, "changes_files", "n", &changes.to_string()),
            None,
        )
        .segment("terminal", i18n::t(cx, "terminal"), None)
        .on_change({
            let view = view.clone();
            move |key, _, cx| {
                view.update(cx, |this, cx| {
                    let next = if key.as_ref() == "terminal" {
                        RightTab::Terminal
                    } else {
                        RightTab::Diff
                    };
                    if this.right_tab != next {
                        this.open_right(next, cx);
                    }
                    cx.notify();
                });
            }
        });

    div()
        .flex_none()
        .h_full()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .bg(colors.surface)
        .border_l_1()
        .border_color(colors.border)
        .child(
            // The chrome: tabs on the left, close on the right.
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(colors.border)
                .child(div().flex_1().min_w_0().child(tabs))
                .when(tab == RightTab::Terminal, |this| {
                    let view = view.clone();
                    this.child(
                        IconButton::new("terminal-new", IconName::Plus)
                            .variant(ButtonVariant::Ghost)
                            .size(ControlSize::Sm)
                            .tooltip(i18n::t(cx, "new_terminal"))
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| this.restart_terminal(cx));
                            }),
                    )
                })
                .child({
                    let view = view.clone();
                    IconButton::new("panel-close", IconName::X)
                        .variant(ButtonVariant::Ghost)
                        .size(ControlSize::Sm)
                        .tooltip(i18n::t(cx, "close_panel"))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.close_right(cx));
                        })
                }),
        )
        .child(match tab {
            RightTab::Diff => diff_tab(app, view, state, cx).into_any_element(),
            RightTab::Terminal => terminal_tab(app, view, cx).into_any_element(),
        })
        .into_any_element()
}

/// The session's change review, or a quiet word when nothing has changed.
fn diff_tab(
    app: &PidoApp,
    view: &Entity<PidoApp>,
    state: &Entity<AppState>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    if state.read(cx).changes.is_empty() {
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme.text_size(TextSize::Sm))
            .text_color(theme.colors.fg_subtle)
            .child(Caption::new(i18n::t(cx, "changes_empty")))
            .into_any_element();
    }
    crate::ui::changes::render(state, view, app.diff_selected.clone(), app.diff_layout, cx)
        .into_any_element()
}

/// The live shell, or the way back to a fresh one.
fn terminal_tab(app: &PidoApp, view: &Entity<PidoApp>, cx: &App) -> impl IntoElement + use<> {
    match &app.terminal {
        Some(terminal) => div()
            .id("terminal-pane")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(terminal.clone())
            .into_any_element(),
        None => div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Caption::new(i18n::t(cx, "terminal_gone")))
            .child(
                Button::new("terminal-again", i18n::t(cx, "new_terminal")).on_click({
                    let view = view.clone();
                    move |_, _, cx| view.update(cx, |this, cx| this.restart_terminal(cx))
                }),
            )
            .into_any_element(),
    }
}

/// Which segment matches `tab`.
fn tab_key(tab: RightTab) -> &'static str {
    match tab {
        RightTab::Diff => "diff",
        RightTab::Terminal => "terminal",
    }
}
