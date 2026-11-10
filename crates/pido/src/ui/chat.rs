//! The transcript: streaming assistant messages, thinking blocks, tool call
//! cards, and the session header (model / thinking / stats pickers).

use ely_gpui_component::agent::ToolCallCard;
use ely_gpui_component::chat::{
    MessageBubble, MessageHeader, MessageList, Role, StepState, ThinkingBlock,
};

use ely_gpui_component::theme::ActiveTheme;
use ely_gpui_component::typography::{Caption, Paragraph};
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder,
};
use pi_rpc::protocol::ThinkingLevel;

use crate::i18n;
use crate::state::{AppState, NoticeKind, TranscriptItem};
use crate::ui::app::toggle_theme;

/// The header row above the transcript: session title, theme and language
/// toggles, compaction. Model and thinking picks live in the composer's
/// tool row; run stats sit under the composer.
pub fn render_header(state: &Entity<AppState>, cx: &App) -> impl IntoElement + use<> {
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
                .child(div().truncate().child(Paragraph::new(
                    s.session_name.clone().unwrap_or_else(|| {
                        s.cwd
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    }),
                ))),
        )
        .child(
            ely_gpui_component::buttons::Button::new("theme", i18n::t(cx, "theme"))
                .on_click(|_, _, cx| toggle_theme(cx)),
        )
        .child(
            ely_gpui_component::buttons::Button::new("locale", next_locale).on_click(
                move |_, _, cx| {
                    crate::settings::save_setting(|s| {
                        s.locale = Some(next_locale.to_string());
                    });
                    ely_gpui_component::i18n::I18n::set_locale(next_locale, cx);
                },
            ),
        )
        .child({
            let app = app.clone();
            ely_gpui_component::buttons::Button::new("compact", i18n::t(cx, "compact_now"))
                .on_click(move |_, _, cx| app.update(cx, |s, cx| s.compact_now(cx)))
        })
}

/// The scrollable transcript area with the notice banner and empty state.
/// The message column mirrors the composer: centered, `px_4`, capped at the
/// theme's medium container width.
pub fn render_transcript(state: &Entity<AppState>, cx: &App) -> impl IntoElement + use<> {
    let s = state.read(cx);
    let count = s.transcript.len();
    let empty = count == 0;
    let app = state.clone();
    let list_epoch = s.session_epoch;
    let notice = s
        .notice
        .clone()
        .map(|(kind, text)| render_notice(kind, text, app.clone(), cx));

    // MessageList is rendered as the direct child of ChatContainer's
    // messages slot: it needs that flex height to resolve its 100%. The
    // centered column is applied per row instead, so the list itself stays
    // full-bleed and scrolls correctly.
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
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .overflow_hidden()
                .child(MessageList::new(
                    ("transcript", list_epoch),
                    count,
                    move |ix, _window, cx| {
                        let s = app.read(cx);
                        let column = cx
                            .theme()
                            .container_width(ely_gpui_component::theme::ContainerSize::Md);
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px_4()
                            .child(div().w_full().max_w(column).min_w_0().child(render_item(
                                &s.transcript[ix],
                                ix,
                                cx,
                            )))
                            .into_any_element()
                    },
                ))
                .into_any_element()
        })
}

/// One transcript row. `ix` keys element ids so streaming text is stable.
fn render_item(item: &TranscriptItem, ix: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match item {
        TranscriptItem::User { text } => MessageBubble::new(Role::User)
            .header(MessageHeader::new(("header", ix), i18n::t(cx, "you")))
            .child(Paragraph::new(text.clone()))
            .into_any_element(),
        TranscriptItem::Assistant(view) => {
            let thinking = view.thinking();
            let text = view.text();
            let empty = thinking.is_empty() && text.is_empty();
            // An assistant message with no text, thinking or error (for
            // example an aborted turn) should not leave a bare "Pi" header.
            if view.finished && view.error.is_none() && empty {
                return div().into_any_element();
            }
            let mut bubble = MessageBubble::new(Role::Assistant)
                .header(MessageHeader::new(("header", ix), i18n::t(cx, "pi")));
            if !thinking.is_empty() {
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
            bubble.into_any_element()
        }
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
