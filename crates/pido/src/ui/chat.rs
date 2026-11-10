//! The transcript: streaming assistant messages, thinking blocks, tool call
//! cards, and the session header (model / thinking / stats pickers).

use ely_gpui_component::agent::ToolCallCard;
use ely_gpui_component::chat::{
    MessageBubble, MessageHeader, MessageList, Role, StepState, ThinkingBlock,
};
use ely_gpui_component::forms::{Choice, Select};
use ely_gpui_component::theme::ActiveTheme;
use ely_gpui_component::typography::{Caption, Paragraph};
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use pi_rpc::protocol::ThinkingLevel;

use crate::i18n;
use crate::state::{AppState, NoticeKind, TranscriptItem};
use crate::ui::app::toggle_theme;

/// The header row above the transcript: session title, model and thinking
/// pickers, stats, theme and language toggles.
pub fn render_header(state: &Entity<AppState>, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let s = state.read(cx);
    let app = state.clone();

    let model_choices: Vec<Choice> = s
        .models
        .iter()
        .map(|model| {
            let note = model.context_window.map(|window| format!("{window} ctx"));
            Choice {
                note: note.map(Into::into),
                ..Choice::new(model.id.clone(), model.name.clone())
            }
        })
        .collect();
    let selected_model: SharedString = s
        .current_model
        .as_ref()
        .map(|m| m.id.clone().into())
        .unwrap_or_default();

    let level_choices: Vec<Choice> = s
        .thinking_levels
        .iter()
        .map(|&level| Choice::new(level_slug(level), level_slug(level)))
        .collect();
    let selected_level: SharedString = s.thinking_level.map(level_slug).unwrap_or_default().into();

    let stats_line: SharedString = match &s.stats {
        Some(stats) => {
            let tokens = stats
                .tokens
                .as_ref()
                .map(|t| t.total_tokens.unwrap_or(t.input + t.output))
                .unwrap_or(0);
            let cost = stats
                .cost
                .map(|c| format!(" · ${c:.3}"))
                .unwrap_or_default();
            format!("{tokens} {}{cost}", i18n::t(cx, "tokens")).into()
        }
        None => String::new().into(),
    };

    let next_locale = i18n::other_locale(cx);

    div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(theme.colors.border)
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().truncate().child(Paragraph::new(
                    s.session_name.clone().unwrap_or_else(|| {
                        s.cwd
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    }),
                )))
                .child(Caption::new(stats_line)),
        )
        .child(div().w(px(240.)).when(!model_choices.is_empty(), |this| {
            let app = app.clone();
            this.child(
                Select::new("model", model_choices)
                    .label(i18n::t(cx, "model"))
                    .selected(selected_model)
                    .on_change(move |next, _, cx| {
                        app.update(cx, |s, cx| s.set_model(next, cx));
                    }),
            )
        }))
        .child(div().w(px(130.)).when(!level_choices.is_empty(), |this| {
            let app = app.clone();
            this.child(
                Select::new("thinking", level_choices)
                    .label(i18n::t(cx, "thinking"))
                    .selected(selected_level)
                    .on_change(move |next, _, cx| {
                        if let Some(level) = parse_level_slug(next) {
                            app.update(cx, |s, cx| s.set_thinking(level, cx));
                        }
                    }),
            )
        }))
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
///
/// Parent is Ely's `ChatContainer`, which supplies the column height but no
/// horizontal padding for messages (it gives that treatment only to the
/// composer). The message column here mirrors the composer's: centered,
/// `px_4`, capped at the theme's medium container width.
pub fn render_transcript(
    state: &Entity<AppState>,
    _window: &mut Window,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let column = theme.container_width(ely_gpui_component::theme::ContainerSize::Md);
    let s = state.read(cx);
    let count = s.transcript.len();
    let empty = count == 0;
    let app = state.clone();
    let notice = s
        .notice
        .clone()
        .map(|(kind, text)| render_notice(kind, text, app.clone(), cx));

    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .when_some(notice, |this, banner| {
            this.child(div().px_4().pt_2().child(banner))
        })
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .justify_center()
                .px_4()
                .child(
                    div()
                        .w_full()
                        .max_w(column)
                        .flex()
                        .flex_col()
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
                                .min_h_0()
                                .child(MessageList::new(
                                    "transcript",
                                    count,
                                    move |ix, _window, cx| {
                                        let s = app.read(cx);
                                        render_item(&s.transcript[ix], ix, cx).into_any_element()
                                    },
                                ))
                                .into_any_element()
                        }),
                ),
        )
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
            let mut bubble = MessageBubble::new(Role::Assistant)
                .header(MessageHeader::new(("header", ix), i18n::t(cx, "pi")));
            if !view.thinking.is_empty() {
                // Only a still-thinking view ticks; a finished one shows the
                // frozen duration. Restored history records no durations, so
                // it falls back to "Thought for a moment" (Duration::ZERO)
                // instead of a value that would grow on every redraw.
                let took = if view.thinking_live {
                    view.took.unwrap_or_else(|| view.started.elapsed())
                } else {
                    view.took.unwrap_or_default()
                };
                bubble = bubble.child(ThinkingBlock::new(
                    ("thinking", ix),
                    view.thinking.clone(),
                    view.thinking_live,
                    took,
                ));
            }
            if !view.text.is_empty() || view.text_live {
                bubble = bubble.child(ely_gpui_component::chat::StreamingMarkdown::new(
                    ("text", ix),
                    view.text.clone(),
                    !view.finished,
                ));
            }
            if !view.finished && view.thinking.is_empty() && view.text.is_empty() {
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

/// The dismissible banner above the transcript.
fn render_notice(
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
    }
}

fn parse_level_slug(slug: &str) -> Option<ThinkingLevel> {
    Some(match slug {
        "off" => ThinkingLevel::Off,
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" => ThinkingLevel::Xhigh,
        "max" => ThinkingLevel::Max,
        _ => return None,
    })
}
