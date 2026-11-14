//! The transcript: streaming assistant messages, thinking blocks, tool call
//! cards, and the session header (model / thinking / stats pickers).

use ely_gpui_component::agent::ToolCallCard;
use ely_gpui_component::chat::{MessageBubble, Role, StepState, ThinkingBlock};

use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, TextSize};
use ely_gpui_component::typography::{Caption, Paragraph};
use gpui::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use pi_rpc::protocol::ThinkingLevel;

use crate::i18n;
use crate::state::{AppState, AssistantView, NoticeKind, TranscriptItem};
use crate::ui::app::{RightTab, toggle_theme};

/// The header row above the transcript: session title, theme and language
/// toggles, compaction. Model and thinking picks live in the composer's
/// tool row; run stats sit under the composer.
pub fn render_header(
    state: &Entity<AppState>,
    view: &Entity<crate::ui::app::PidoApp>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let s = state.read(cx);
    let app = state.clone();
    let next_locale = i18n::other_locale(cx);

    div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .h(gpui::px(crate::ui::app::HEADER_HEIGHT))
        .border_b_1()
        .border_color(theme.colors.border)
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(div().min_w_0().truncate().child(Paragraph::new(
                    s.session_name.clone().unwrap_or_else(|| {
                        s.cwd
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    }),
                )))
                .child({
                    let view = view.clone();
                    ely_gpui_component::buttons::IconButton::new(
                        "rename-session",
                        ely_gpui_component::primitives::IconName::Pencil,
                    )
                    .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
                    .tooltip(i18n::t(cx, "rename"))
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| this.open_rename(window, cx));
                    })
                }),
        )
        // The header's actions are icon buttons; what they do shows on hover.
        .when(!s.changes.is_empty(), |this| {
            let view = view.clone();
            let count = s.changes.len();
            let tooltip = format!(
                "{} ({})",
                i18n::t(cx, "changes"),
                i18n::t_arg(cx, "changes_files", "n", &count.to_string())
            );
            this.child(
                ely_gpui_component::buttons::IconButton::new(
                    "open-changes",
                    ely_gpui_component::primitives::IconName::MessageSquareDiff,
                )
                .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
                .tooltip(tooltip)
                .on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.open_right(RightTab::Diff, cx));
                }),
            )
        })
        .child({
            let view = view.clone();
            ely_gpui_component::buttons::IconButton::new(
                "open-terminal",
                ely_gpui_component::primitives::IconName::Terminal,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .tooltip(i18n::t(cx, "terminal"))
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.open_right(RightTab::Terminal, cx));
            })
        })
        .child({
            let dark = theme.is_dark();
            ely_gpui_component::buttons::IconButton::new(
                "theme",
                if dark {
                    ely_gpui_component::primitives::IconName::Sun
                } else {
                    ely_gpui_component::primitives::IconName::Moon
                },
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .tooltip(i18n::t(cx, "theme"))
            .on_click(|_, _, cx| toggle_theme(cx))
        })
        .child(
            ely_gpui_component::buttons::IconButton::new(
                "locale",
                ely_gpui_component::primitives::IconName::Languages,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .tooltip(next_locale)
            .on_click(move |_, _, cx| {
                crate::settings::save_setting(|s| {
                    s.locale = Some(next_locale.to_string());
                });
                ely_gpui_component::i18n::I18n::set_locale(next_locale, cx);
            }),
        )
        .child({
            let app = app.clone();
            ely_gpui_component::buttons::IconButton::new(
                "export-html",
                ely_gpui_component::primitives::IconName::ExternalLink,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .tooltip(i18n::t(cx, "export_html"))
            .on_click(move |_, _, cx| app.update(cx, |s, cx| s.export_html(cx)))
        })
        .child({
            let app = app.clone();
            let auto = s.auto_compaction.unwrap_or(true);
            let compact_label = i18n::t(cx, "compact_now").to_string();
            let auto_label = i18n::t(cx, "auto_compact").to_string();
            let tooltip = i18n::t(cx, "compact_now");
            let compact = app.clone();
            let menu = ely_gpui_component::menus::Menu::new()
                .item(
                    ely_gpui_component::menus::MenuItem::new(compact_label)
                        .icon(ely_gpui_component::primitives::IconName::Minimize2)
                        .on_click(move |_, cx| compact.update(cx, |s, cx| s.compact_now(cx))),
                )
                .item({
                    let app = app.clone();
                    ely_gpui_component::menus::MenuItem::check(auto_label, auto).on_click(
                        move |_, cx| {
                            app.update(cx, |s, cx| s.set_auto_compaction(!auto, cx));
                        },
                    )
                });
            ely_gpui_component::menus::menu_under(
                "compact".into(),
                &menu,
                move |click| {
                    ely_gpui_component::buttons::IconButton::new(
                        "compact-button",
                        ely_gpui_component::primitives::IconName::Minimize2,
                    )
                    .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
                    .tooltip(tooltip)
                    .on_click(move |event, window, cx| click(event, window, cx))
                    .into_any_element()
                },
                window,
                cx,
            )
        })
}

/// The scrollable transcript area with the notice banner and empty state.
/// The message column mirrors the composer: centered, `px_4`, capped at the
/// theme's medium container width. The scroll state is owned by the view so
/// search can jump to a row and the tail-follow badge survives re-renders.
pub fn render_transcript(
    state: &Entity<AppState>,
    view: &Entity<crate::ui::app::PidoApp>,
    app: &mut crate::ui::app::PidoApp,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let s = state.read(cx);
    let empty = s.transcript.is_empty();
    let notice = s
        .notice
        .clone()
        .map(|(kind, text)| render_notice(kind, text, state.clone(), cx));

    // Folded content must not exist in the list at all: an empty row would
    // still hold its slot, so the hidden tool calls would keep their space.
    let visible: Vec<usize> = s.visible_rows();
    let count = visible.len();

    // Own the scroll state: rebuilt when the conversation was replaced, kept
    // (with its measured heights and away badge) across re-renders.
    if app.list_epoch != s.session_epoch || app.transcript_list.is_none() {
        let overdraw = cx.theme().list_overdraw().to_pixels(window.rem_size());
        let list = gpui::ListState::new(count, gpui::ListAlignment::Bottom, overdraw);
        let watcher = view.downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            let away = event.is_scrolled;
            watcher
                .update(cx, |app, cx| {
                    if app.transcript_away != away {
                        app.transcript_away = away;
                        if away {
                            app.transcript_fresh = 0;
                        }
                        cx.notify();
                    }
                })
                .ok();
        });
        app.transcript_list = Some(list);
        app.list_epoch = s.session_epoch;
        app.transcript_away = false;
        app.transcript_fresh = 0;
    }
    let list = app.transcript_list.clone().expect("transcript list");
    let feed_count = list.item_count();
    if feed_count != count {
        // Rows arriving while scrolled away count for the jump badge; at the
        // bottom, Bottom alignment pins to the newest row for free.
        if count > feed_count && app.transcript_away {
            app.transcript_fresh += count - feed_count;
        }
        if count >= feed_count {
            list.splice(feed_count..feed_count, count - feed_count);
        } else {
            list.splice(count..feed_count, 0);
        }
    }

    // The list is the direct child of the transcript's flex slot: it needs
    // that height to resolve its 100%. The centered column is applied per
    // row instead, so the list stays full-bleed and scrolls correctly.
    div()
        .relative()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .when_some(notice, |this, banner| {
            this.child(div().flex_none().px_4().pt_2().child(banner))
        })
        .child(if empty {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(Caption::new(i18n::t(cx, "empty_transcript")))
                .into_any_element()
        } else {
            let visible = std::rc::Rc::new(visible);
            // The current find match's transcript index, for the row tint.
            let hit_ix = app
                .find
                .as_ref()
                .and_then(|find| find.matches.get(find.current).copied());
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .overflow_hidden()
                .relative()
                .child(
                    gpui::list(list.clone(), {
                        let visible = visible.clone();
                        let app = state.clone();
                        let hit_ix = hit_ix;
                        move |pos, _window, cx| {
                            let s = app.read(cx);
                            let ix = visible[pos];
                            let column = cx
                                .theme()
                                .container_width(ely_gpui_component::theme::ContainerSize::Md);
                            // A finished turn folds its tool calls and thinking
                            // away until its status line is clicked.
                            let folded =
                                crate::state::turn_folded_for(&s.transcript, ix, &s.expanded_turns);
                            let hit = hit_ix == Some(ix);
                            div()
                                .w_full()
                                .flex()
                                .justify_center()
                                .px_4()
                                .child(
                                    div()
                                        .w_full()
                                        .max_w(column)
                                        .min_w_0()
                                        .when(hit, |row| {
                                            row.rounded_sm().bg(cx.theme().colors.info_subtle)
                                        })
                                        .child(render_item(
                                            &app,
                                            &s.transcript[ix],
                                            folded,
                                            ix,
                                            cx,
                                        )),
                                )
                                .into_any_element()
                        }
                    })
                    .size_full(),
                )
                // Scrollbar drags to the bottom never fire the scroll handler;
                // a paint pass re-checks engagement so the badge retires itself.
                .child(
                    gpui::canvas(
                        {
                            let view = view.downgrade();
                            let list = list.clone();
                            move |_, window, cx| {
                                let held = list.logical_scroll_top().item_ix >= list.item_count();
                                let away = view
                                    .upgrade()
                                    .is_some_and(|app| app.read(cx).transcript_away);
                                if away && held {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |app, cx| {
                                            app.transcript_away = false;
                                            app.transcript_fresh = 0;
                                            cx.notify();
                                        });
                                    }
                                    window.request_animation_frame();
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
                .when(app.transcript_away, |this| {
                    this.child(
                        div()
                            .absolute()
                            .bottom_4()
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child({
                                let view = view.downgrade();
                                ely_gpui_component::chat::ScrollToBottomButton::new(
                                    ("transcript-jump", app.list_epoch),
                                    app.transcript_fresh,
                                    move |_, cx| {
                                        if let Some(view) = view.upgrade() {
                                            view.update(cx, |app, cx| {
                                                if let Some(list) = &app.transcript_list {
                                                    let end = list.item_count();
                                                    list.scroll_to(gpui::ListOffset {
                                                        item_ix: end,
                                                        offset_in_item: gpui::px(0.),
                                                    });
                                                }
                                                app.transcript_away = false;
                                                app.transcript_fresh = 0;
                                                cx.notify();
                                            });
                                        }
                                    },
                                )
                            }),
                    )
                })
                .when_some(app.find.clone(), |this, find| {
                    this.child(render_find_bar(view, &find, cx))
                })
                .into_any_element()
        })
}

/// The floating find bar: query, match counter, stepping and close. The
/// field's Enter steps forward (subscribed at open); the arrows step either
/// way and the jump reveals the match's row.
fn render_find_bar(
    view: &Entity<crate::ui::app::PidoApp>,
    find: &crate::ui::app::FindState,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let query = find.field.read(cx).text().to_string();
    let counter = if query.trim().is_empty() {
        None
    } else if find.matches.is_empty() {
        Some(i18n::t(cx, "find_none").to_string())
    } else {
        Some(format!("{}/{}", find.current + 1, find.matches.len()))
    };
    let (prev_view, next_view, close_view) = (view.clone(), view.clone(), view.clone());
    div()
        .absolute()
        .top_2()
        .right_4()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .child(div().w(gpui::px(220.)).child(find.field.clone()))
        .when_some(counter, |this, counter| {
            this.child(
                div()
                    .text_size(theme.text_size(TextSize::Xs))
                    .text_color(theme.colors.fg_subtle)
                    .child(counter),
            )
        })
        .child({
            let empty = find.matches.is_empty();
            ely_gpui_component::buttons::IconButton::new(
                "find-prev",
                ely_gpui_component::primitives::IconName::ArrowUp,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .size(ely_gpui_component::theme::ControlSize::Sm)
            .when(empty, |button| button.disabled(true))
            .on_click(move |_, _, cx| {
                prev_view.update(cx, |this, cx| this.find_step(false, cx));
            })
        })
        .child({
            let empty = find.matches.is_empty();
            ely_gpui_component::buttons::IconButton::new(
                "find-next",
                ely_gpui_component::primitives::IconName::ArrowDown,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .size(ely_gpui_component::theme::ControlSize::Sm)
            .when(empty, |button| button.disabled(true))
            .on_click(move |_, _, cx| {
                next_view.update(cx, |this, cx| this.find_step(true, cx));
            })
        })
        .child({
            ely_gpui_component::buttons::IconButton::new(
                "find-close",
                ely_gpui_component::primitives::IconName::X,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .size(ely_gpui_component::theme::ControlSize::Sm)
            .on_click(move |_, _, cx| {
                close_view.update(cx, |this, cx| this.close_find(cx));
            })
        })
}
fn render_item(
    app: &Entity<AppState>,
    item: &TranscriptItem,
    folded: bool,
    ix: usize,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    match item {
        // A turn's status line: the working clock while it runs, then what
        // the turn cost, with a chevron that re-opens what was folded away.
        TranscriptItem::Turn { started, took } => {
            let (text, running) = match took {
                Some(took) => (
                    i18n::t_arg(cx, "turn_done", "t", &i18n::format_duration(cx, *took)),
                    false,
                ),
                None => (
                    i18n::t_arg(
                        cx,
                        "turn_working",
                        "t",
                        &i18n::format_duration(cx, started.elapsed()),
                    ),
                    true,
                ),
            };
            let expanded = app.read(cx).expanded_turns.contains(&(ix as u64));
            let app = app.clone();
            div()
                .id(("turn", ix))
                .flex_none()
                .flex()
                .items_center()
                .gap_1p5()
                .py_1()
                .mb_1()
                .border_b_1()
                .border_color(theme.colors.border)
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(theme.colors.fg_subtle)
                .when(!running, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.text_color(theme.colors.fg))
                        .on_click(move |_, _, cx| {
                            app.update(cx, |s, cx| {
                                if s.expanded_turns.contains(&(ix as u64)) {
                                    s.expanded_turns.remove(&(ix as u64));
                                } else {
                                    s.expanded_turns.insert(ix as u64);
                                }
                                cx.notify();
                            });
                        })
                })
                .child(text)
                .when(!running, |this| {
                    this.child(
                        // The reference shows the down chevron while folded.
                        ely_gpui_component::primitives::Disclosure::new(
                            ("turn-chevron", ix),
                            !expanded,
                        )
                        .size(ely_gpui_component::theme::IconSize::Xs),
                    )
                })
                .into_any_element()
        }
        TranscriptItem::User { text, time } => {
            let group = SharedString::from(format!("user-{ix}"));
            let footer = div()
                .flex_none()
                .invisible()
                .group_hover(group.clone(), |style| style.visible())
                .flex()
                .items_center()
                .justify_end()
                .gap_1p5()
                .pt_0p5()
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Xs))
                        .text_color(theme.colors.fg_subtle)
                        .child(i18n::format_time(cx, *time)),
                )
                .child({
                    let app = app.clone();
                    ely_gpui_component::buttons::IconButton::new(
                        ("fork-user", ix),
                        ely_gpui_component::primitives::IconName::GitBranch,
                    )
                    .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
                    .size(ControlSize::Sm)
                    .tooltip(i18n::t(cx, "fork_turn"))
                    .on_click(move |_, _, cx| {
                        let Some((text, occurrence)) =
                            crate::state::fork_point_for(&app.read(cx).transcript, ix)
                        else {
                            return;
                        };
                        app.update(cx, |s, cx| s.fork_turn(&text, occurrence, cx));
                    })
                })
                .child({
                    let copy = text.clone();
                    ely_gpui_component::buttons::IconButton::new(
                        ("copy-user", ix),
                        ely_gpui_component::primitives::IconName::Copy,
                    )
                    .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
                    .size(ControlSize::Sm)
                    .tooltip(i18n::t(cx, "copy_message"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.to_string()));
                    })
                });
            div()
                .group(group)
                .flex()
                .flex_col()
                .child(MessageBubble::new(Role::User).child(
                    ely_gpui_component::typography::SelectableText::new(
                        ("user-text", ix),
                        text.clone(),
                    ),
                ))
                .child(footer)
                .into_any_element()
        }
        TranscriptItem::Assistant(view) => {
            let thinking = view.thinking();
            let text = view.text();
            let empty = thinking.is_empty() && text.is_empty();
            // An assistant message with no text, thinking or error (for
            // example an aborted turn) should not leave a bare "Pi" header.
            if view.finished && view.error.is_none() && empty {
                return div().into_any_element();
            }
            let mut bubble = MessageBubble::new(Role::Assistant);
            if !thinking.is_empty() && !folded {
                // Only a still-thinking view ticks; a finished one shows the
                // frozen duration. Restored history records no durations, so
                // it falls back to "Thought for a moment" (Duration::ZERO)
                // instead of a value that would grow on every redraw.
                let took = if view.live_thinking.is_some() {
                    view.took.unwrap_or_else(|| view.started.elapsed())
                } else {
                    view.took.unwrap_or_default()
                };
                bubble = bubble.child(ThinkingBlock::new(
                    ("thinking", ix),
                    thinking,
                    view.live_thinking.is_some(),
                    took,
                ));
            }
            if !text.is_empty() || view.live_text.is_some() {
                // A finished message draws from blocks parsed once; a live one
                // streams, which has to re-read its source each frame.
                let cached = view.cached();
                bubble = bubble.child(match cached {
                    Some(cached) => ely_gpui_component::documents::MarkdownRenderer::blocks(
                        ("text", ix),
                        cached.blocks.clone(),
                    )
                    // Finished text picks up pointer selection: drag to
                    // select, Ctrl-C copies, A selects all. A right press
                    // copies the selection straight away when there is one.
                    .selectable(|press, _, cx| {
                        if let Some(selected) = press.selected {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                        }
                    })
                    .into_any_element(),
                    None => ely_gpui_component::chat::StreamingMarkdown::new(
                        ("text", ix),
                        text,
                        view.live_text.is_some() && !view.finished,
                    )
                    .into_any_element(),
                });
            }
            if let Some(error) = &view.error {
                bubble = bubble.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.colors.danger_subtle)
                        .text_color(theme.colors.danger)
                        .child(Paragraph::new(error.clone())),
                );
            }
            if !view.finished && empty {
                // Message announced, first delta still in flight.
                bubble = bubble.child(ely_gpui_component::chat::ThinkingIndicator::new((
                    "wait", ix,
                )));
            }
            let wrapper = div().group(group(ix)).flex().flex_col().child(bubble);
            // A finished message shows its actions on hover; a streaming one
            // has nothing stable to copy yet.
            if view.finished {
                wrapper
                    .child(assistant_footer(ix, view, cx))
                    .into_any_element()
            } else {
                wrapper.into_any_element()
            }
        }
        TranscriptItem::Tool(card) if folded => div().into_any_element(),
        TranscriptItem::Tool(card) => {
            let status = if card.running {
                StepState::Working
            } else if card.failed {
                StepState::Failed
            } else if card.result.is_some() || card.took.is_some() {
                StepState::Done
            } else {
                StepState::Waiting
            };
            let mut rendered = ToolCallCard::new(("tool", ix), card.name.clone(), status);
            if let Some(args) = &card.args {
                rendered = rendered.arguments(args.clone());
            }
            if let Some(result) = &card.result {
                rendered = rendered.result(result.clone());
            }
            if let Some(took) = card.took {
                rendered = rendered.took(took);
            }
            div().px_1().child(rendered).into_any_element()
        }
        TranscriptItem::Notice { .. } if folded => div().into_any_element(),
        TranscriptItem::Notice { kind, text } => {
            let (fill, ink) = match kind {
                NoticeKind::Info => (theme.colors.info_subtle, theme.colors.info),
                NoticeKind::Warn => (theme.colors.warning_subtle, theme.colors.warning),
                NoticeKind::Error => (theme.colors.danger_subtle, theme.colors.danger),
            };
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(fill)
                .text_color(ink)
                .child(Caption::new(text.clone()))
                .into_any_element()
        }
    }
}

/// Hover group id for a row's wrapper, shared by both message kinds.
fn group(ix: usize) -> SharedString {
    SharedString::from(format!("msg-{ix}"))
}

/// The action row under a finished assistant message: copy, fork the turn,
/// what it cost in tokens, and when it landed.
fn assistant_footer(ix: usize, view: &AssistantView, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let copy = view.text();
    let usage = view
        .tokens
        .map(|tokens| format!("{} {}", i18n::t(cx, "usage"), compact_tokens(tokens)));
    div()
        .flex_none()
        .invisible()
        .group_hover(group(ix), |style| style.visible())
        .flex()
        .items_center()
        .gap_2()
        .pt_0p5()
        .child({
            ely_gpui_component::buttons::IconButton::new(
                ("copy-assistant", ix),
                ely_gpui_component::primitives::IconName::Copy,
            )
            .variant(ely_gpui_component::buttons::ButtonVariant::Ghost)
            .size(ControlSize::Sm)
            .tooltip(i18n::t(cx, "copy_message"))
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.to_string()));
            })
        })
        .when_some(usage, |this, usage| {
            this.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(theme.text_size(TextSize::Xs))
                    .text_color(theme.colors.fg_subtle)
                    .child(
                        ely_gpui_component::primitives::Icon::new(
                            ely_gpui_component::primitives::IconName::Database,
                        )
                        .size(IconSize::Xs),
                    )
                    .child(usage),
            )
        })
        .child(
            div()
                .text_size(theme.text_size(TextSize::Xs))
                .text_color(theme.colors.fg_subtle)
                .child(i18n::format_time(cx, view.time)),
        )
        .into_any_element()
}

/// Tokens in review notation: 5.1M, 512k, 999.
fn compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1e6)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// The dismissible banner above the transcript (also used by the welcome
/// screen for connection errors).
pub fn render_notice(
    kind: NoticeKind,
    text: String,
    app: Entity<AppState>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (fill, ink) = match kind {
        NoticeKind::Info => (theme.colors.info_subtle, theme.colors.info),
        NoticeKind::Warn => (theme.colors.warning_subtle, theme.colors.warning),
        NoticeKind::Error => (theme.colors.danger_subtle, theme.colors.danger),
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(fill)
        .text_color(ink)
        .child(Paragraph::new(text))
        .child(
            ely_gpui_component::buttons::Button::new("dismiss", i18n::t(cx, "dismiss"))
                .on_click(move |_, _, cx| app.update(cx, |s, cx| s.dismiss_notice(cx))),
        )
}

/// Wire name for a thinking level.
pub fn level_slug(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
        ThinkingLevel::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_read_in_review_notation() {
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(5_120), "5k");
        assert_eq!(compact_tokens(5_100_000), "5.1M");
    }
}
