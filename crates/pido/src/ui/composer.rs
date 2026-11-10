//! The composer: Ely's PromptInput wired to prompt/steer/stop, offering
//! `/` commands and `@` file mentions, with a single model pill on the right
//! of the tool row and a stats line beneath.
//!
//! Spacing and the centered column width come from Ely's `ChatContainer`.

use ely_gpui_component::buttons::{ButtonVariant, IconButton};
use ely_gpui_component::chat::PromptInput;
use ely_gpui_component::forms::Choice;
use ely_gpui_component::menus::{DropdownMenu, Menu, MenuItem};
use ely_gpui_component::primitives::IconName;
use ely_gpui_component::theme::ActiveTheme;
use gpui::{
    App, Entity, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder, px,
};

use crate::i18n;
use crate::state::AppState;
use crate::ui::app::PidoApp;
use crate::ui::chat::level_slug;

/// Render the composer block. While the agent runs, Enter sends a steering
/// message and a separate stop button aborts the run.
pub fn render(state: &Entity<AppState>, app: &mut PidoApp, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let s = state.read(cx);
    let streaming = s.streaming;
    let Some(draft) = app.draft.clone() else {
        return div().into_any_element();
    };

    let send_state = state.clone();
    let owned_draft = draft.clone();
    let input = PromptInput::new("composer", &draft, move |_, cx| {
        let text = owned_draft.read(cx).text().to_string();
        if !text.trim().is_empty() {
            owned_draft.update(cx, |input, cx| input.set_text("", cx));
            send_state.update(cx, |s, cx| s.send_text(text, cx));
        }
    });

    // Do not use PromptInput::busy here: it also swallows Enter, which is
    // exactly how a running agent should be steered. The send button stays a
    // send button while streaming (it sends `steer`), and a separate Stop
    // tool aborts the run.

    // `/` offers pi's extension commands, prompt templates and skills. The
    // component strips the typed `/query` and hands us the value; commands
    // run by sending `/name`, so the text goes back at the message start.
    let command_choices: Vec<Choice> = s
        .commands
        .iter()
        .map(|command| {
            let slash = format!("/{}", command.name);
            let label = match command.description.as_deref() {
                Some(note) if !note.is_empty() => format!("{slash} — {note}"),
                _ => slash.clone(),
            };
            Choice::new(slash, label)
        })
        .collect();
    let command_draft = draft.clone();
    let input = input.commands(command_choices, move |command, _, cx| {
        let command = command.to_string();
        command_draft.update(cx, |field, cx| {
            let rest = field.text().trim_start().to_string();
            field.set_text(format!("{command} {rest}"), cx);
        });
    });

    // `@` inserts a project-relative path. Pi expands `@path` only as CLI
    // arguments, never inside a prompt, so the path goes in as plain text and
    // the agent reads it with its own tools.
    let file_choices: Vec<Choice> = s
        .workspace_files
        .iter()
        .map(|path| Choice::new(path.as_str(), path.as_str()))
        .collect();
    let mention_draft = draft.clone();
    let input = input.context(file_choices, move |path, _, cx| {
        let path = format!("{path} ");
        mention_draft.update(cx, |field, cx| field.insert(&path, cx));
    });

    // The model pill sits at the right of the tool row, beside send; a
    // separate stop button appears while the agent runs.
    let pill = model_pill(s, state.clone(), cx);
    let stop = if streaming {
        let stop_state = state.clone();
        Some(
            IconButton::new("composer-stop", IconName::Square)
                .variant(ButtonVariant::Secondary)
                .tooltip(i18n::t(cx, "stop"))
                .on_click(move |_, _, cx| stop_state.update(cx, |s, cx| s.stop(cx))),
        )
    } else {
        None
    };

    let stats_line = stats_caption(s, cx);

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            input.tool(
                // A growable spacer with the controls pinned to its right edge:
                // the component's own spacer then gets no space to expand.
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .children(stop)
                    .child(pill),
            ),
        )
        .when_some(stats_line, |this, line| {
            this.child(
                div()
                    .flex()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(colors.fg_subtle)
                    .child(line),
            )
        })
        .into_any_element()
}

/// One pill reading `Model Level`, opening a two-row menu: the model list and
/// the thinking levels, each as a submenu with the active value on the right.
fn model_pill(s: &AppState, picker: Entity<AppState>, cx: &App) -> DropdownMenu {
    let model_name = s
        .current_model
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| i18n::t(cx, "model").to_string());
    let level = s
        .thinking_level
        .map(|level| level_slug(level).to_string())
        .unwrap_or_default();
    let label = if level.is_empty() {
        model_name.clone()
    } else {
        format!("{model_name} {level}")
    };

    // 模型 ▸ every configured model, the active one checked. Identity is
    // (provider, id): two providers may expose the same model id.
    let current = s
        .current_model
        .as_ref()
        .map(|m| (m.provider.clone().unwrap_or_default(), m.id.clone()));
    let mut models = Menu::new();
    for model in &s.models {
        let picker = picker.clone();
        let id = model.id.clone();
        let provider = model.provider.clone().unwrap_or_default();
        let checked = current
            .as_ref()
            .is_some_and(|(current_provider, current_id)| {
                current_provider == &provider && current_id == &id
            });
        models = models.item(MenuItem::radio(model.name.clone(), checked).on_click(
            move |_, cx| {
                let id = id.clone();
                let provider = provider.clone();
                picker.update(cx, |s, cx| s.set_model(&provider, &id, cx));
            },
        ));
    }

    // 推理等级 ▸ the levels the current model supports.
    let mut levels = Menu::new();
    for &level in &s.thinking_levels {
        let picker = picker.clone();
        let checked = s.thinking_level == Some(level);
        levels = levels.item(
            MenuItem::radio(level_slug(level), checked).on_click(move |_, cx| {
                picker.update(cx, |s, cx| s.set_thinking(level, cx));
            }),
        );
    }

    let menu = Menu::new()
        .item(MenuItem::submenu(i18n::t(cx, "model"), models).keys(model_name))
        .item(
            MenuItem::submenu(i18n::t(cx, "thinking"), levels)
                .keys(level_default(s, i18n::t(cx, "thinking"))),
        );

    DropdownMenu::new("composer-model", label, menu)
}

/// The row's trailing value, falling back to a dash when unset.
fn level_default(s: &AppState, fallback: SharedString) -> SharedString {
    match s.thinking_level {
        Some(level) => SharedString::from(level_slug(level)),
        None => fallback,
    }
}

/// `12 条消息 · 3.4M tok · 缓存命中 99% · 上下文 61% · 成本 $0.0123`,
/// or nothing before a run.
fn stats_caption(s: &AppState, cx: &App) -> Option<SharedString> {
    let stats = s.stats.as_ref()?;
    let mut parts: Vec<String> = Vec::new();
    if stats.total_messages > 0 {
        parts.push(
            i18n::t_arg(
                cx,
                "status_messages",
                "n",
                &stats.total_messages.to_string(),
            )
            .to_string(),
        );
    }
    if let Some(tokens) = stats.tokens.as_ref() {
        let total = tokens.total_tokens.unwrap_or(tokens.input + tokens.output);
        if total > 0 {
            parts.push(format!("{} tok", compact_count(total)));
            if let Some(hit) = cache_hit_percent(tokens.input, tokens.cache_read) {
                parts.push(i18n::t_arg(cx, "status_cache", "p", &hit).to_string());
            }
        }
    }
    if let Some(ctx) = stats.context_usage.as_ref()
        && let Some(percent) = ctx.percent
    {
        parts.push(i18n::t_arg(cx, "status_ctx", "p", &format!("{percent:.0}")).to_string());
    }
    let cost = stats.cost.or_else(|| {
        stats
            .tokens
            .as_ref()
            .and_then(|tokens| tokens.cost.as_ref().and_then(|cost| cost.total))
    });
    if let Some(cost) = cost
        && cost > 0.0
    {
        parts.push(i18n::t_arg(cx, "status_cost", "c", &format_cost(cost)).to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · ").into())
    }
}

fn format_cost(cost: f64) -> String {
    if cost >= 1.0 {
        format!("${cost:.2}")
    } else if cost >= 0.01 {
        format!("${cost:.3}")
    } else {
        format!("${cost:.4}")
    }
}

/// `12_345_678` → `12.3M`, `45_678` → `45.7k`.
fn compact_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Cache hit percent, rounded; None when nothing to divide.
fn cache_hit_percent(input: u64, cache_read: u64) -> Option<String> {
    (cache_read * 100)
        .checked_div(input + cache_read)
        .map(|p| p.to_string())
}
