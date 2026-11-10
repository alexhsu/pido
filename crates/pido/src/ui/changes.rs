//! The changes panel: the files the agent edited this session, and their
//! hunks, beside the chat rather than over it.
//!
//! The state side parses each finished `edit` patch (and each `write`'s new
//! text) into stretches, so this view only lays out what it is given: a
//! virtualised file list on the left, Ely's diff viewer on the right.

use ely_gpui_component::buttons::{ButtonVariant, IconButton};
use ely_gpui_component::git::{DiffLayout, DiffStat, DiffViewer};
use ely_gpui_component::primitives::{Icon, IconName, file_icon};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, Elevation, IconSize, TextSize};
use ely_gpui_component::typography::MiddleEllipsis;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
    relative,
};

use crate::i18n;
use crate::state::{AppState, ChangeFile};
use crate::ui::app::PidoApp;

/// The sliding review panel, while open.
pub fn render(
    state: &Entity<AppState>,
    view: &Entity<PidoApp>,
    selected_key: Option<String>,
    layout: DiffLayout,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let files = state.read(cx).changes.clone();
    let count = files.len();

    // The picked file, or the newest one when nothing is picked yet; the row
    // highlight and the diff must agree on which it is.
    let selected = files
        .iter()
        .find(|file| selected_key.as_deref() == Some(file.key.as_str()))
        .or_else(|| files.first());
    let selected = selected.map(|file| file.key.clone());

    // A session's edited files number in the tens, so the list is plain rows
    // in a scroll box rather than a virtualised list: fewer layout pitfalls,
    // and every row is cheap.
    let rows: Vec<AnyElement> = files
        .iter()
        .enumerate()
        .map(|(ix, file)| {
            let is_selected = selected.as_deref() == Some(file.key.as_str());
            file_row(cx, ix, file, is_selected, view.clone())
        })
        .collect();

    let diff = match files
        .iter()
        .find(|file| Some(&file.key) == selected.as_ref())
    {
        Some(file) => {
            let layout_view = view.clone();
            DiffViewer::from_stretches(
                ElementId::Name(format!("change-diff:{}", file.key).into()),
                file.path.clone(),
                file.stretches.clone(),
            )
            .layout(layout)
            .on_layout(move |layout: DiffLayout, _, cx| {
                layout_view.update(cx, |this, cx| {
                    this.diff_layout = layout;
                    cx.notify();
                });
            })
            .into_any_element()
        }
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme.text_size(TextSize::Sm))
            .text_color(colors.fg_subtle)
            .child(i18n::t(cx, "changes_empty"))
            .into_any_element(),
    };

    let close = {
        let view = view.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.diff_open = false;
                cx.notify();
            });
        }
    };

    div()
        .id("changes-panel")
        .absolute()
        .top_0()
        .right_0()
        .h_full()
        .w(px(880.))
        .max_w(relative(0.94))
        .flex()
        .flex_col()
        .bg(colors.overlay)
        .border_l_1()
        .border_color(colors.border)
        .shadow(theme.elevation(Elevation::Modal))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .h(px(crate::ui::app::HEADER_HEIGHT))
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(i18n::t(cx, "changes")),
                )
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Sm))
                        .text_color(colors.fg_subtle)
                        .child(i18n::t_arg(cx, "changes_files", "n", &count.to_string())),
                )
                .child(div().flex_1())
                .child(
                    IconButton::new("changes-close", IconName::X)
                        .variant(ButtonVariant::Ghost)
                        .tooltip(i18n::t(cx, "changes_close"))
                        .on_click(close),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_none()
                        .w(px(260.))
                        .h_full()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(colors.border)
                        .child(
                            div()
                                .id("change-file-list")
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .children(rows),
                        ),
                )
                .child(div().flex_1().min_w_0().min_h_0().child(diff)),
        )
}

/// One row of the file list: file mark, path, and how much moved.
fn file_row(
    cx: &App,
    ix: usize,
    file: &ChangeFile,
    selected: bool,
    view: Entity<PidoApp>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let key = file.key.clone();
    let height = theme.control_height(ControlSize::Md);
    div()
        .id(("change-file", ix))
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .h(height)
        .px_3()
        .cursor_pointer()
        .when(selected, |this| this.bg(colors.selection))
        .hover(|this| this.bg(colors.hover))
        .on_click(move |_, _, cx| {
            view.update(cx, |this, cx| {
                this.diff_selected = Some(key.clone());
                cx.notify();
            });
        })
        .child(
            Icon::new(file_icon(&file.path, cx))
                .size(IconSize::Sm)
                .color(colors.fg_muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(theme.mono_family.clone())
                .text_size(theme.text_size(TextSize::Xs))
                .child(MiddleEllipsis::new(file.path.clone())),
        )
        .child(DiffStat::new(file.added, file.removed))
        .into_any_element()
}
