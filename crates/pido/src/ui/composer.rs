//! The composer: Ely's PromptInput wired to prompt/steer/stop, with the
//! model and thinking pickers in its tool row and a stats line beneath —
//! matching the reference layout (picks bottom-left, send bottom-right,
//! run stats centered under the box).
//!
//! Spacing and the centered column width come from Ely's `ChatContainer`.

use ely_gpui_component::chat::PromptInput;
use ely_gpui_component::menus::{DropdownMenu, Menu, MenuItem};
use ely_gpui_component::theme::ActiveTheme;
use ely_gpui_component::typography::Caption;
use gpui::{
    App, Entity, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder, px,
};

use crate::i18n;
use crate::state::AppState;
use crate::ui::app::PidoApp;
use crate::ui::chat::level_slug;

/// Render the composer block. While the agent runs, the send button becomes a
/// stop button and Enter sends a steering message instead of a new prompt.
pub fn render(state: &Entity<AppState>, app: &mut PidoApp, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let s = state.read(cx);
    let streaming = s.streaming;
    let hint = if streaming {
        i18n::t(cx, "steer_hint")
    } else {
        i18n::t(cx, "idle_hint")
    };
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

    let input = if streaming {
        let stop_state = state.clone();
        input.busy(move |_, cx| stop_state.update(cx, |s, cx| s.stop(cx)))
    } else {
        input
    };

    // Tool-row picks, bottom-left: the model, then the thinking level.
    let picker_state = state.clone();
    let model_menu = model_menu(s, picker_state);
    let thinking_menu = thinking_menu(s, state.clone());

    // Run stats under the box: turns, tokens, cache hit, context fill.
    let stats_line = stats_caption(s, cx);

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(input.tool(model_menu).tool(thinking_menu))
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
        .child(div().flex().justify_end().child(Caption::new(hint)))
        .into_any_element()
}

/// The model picker as a dropdown button reading the current model's name.
fn model_menu(s: &AppState, picker: Entity<AppState>) -> DropdownMenu {
    let current = s.current_model.clone();
    let label = current
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| "model".into());
    let current_id = current.as_ref().map(|m| m.id.clone());
    let mut menu = Menu::new();
    for model in &s.models {
        let picker = picker.clone();
        let model_id = model.id.clone();
        let checked = current_id.as_deref() == Some(model.id.as_str());
        menu = menu.item(
            MenuItem::check(model.name.clone(), checked).on_click(move |_, cx| {
                let model_id = model_id.clone();
                picker.update(cx, |s, cx| s.set_model(&model_id, cx));
            }),
        );
    }
    DropdownMenu::new("composer-model", label, menu)
}

/// The thinking-level picker, beside the model's.
fn thinking_menu(s: &AppState, picker: Entity<AppState>) -> DropdownMenu {
    let label = s
        .thinking_level
        .map(|level| SharedString::from(level_slug(level)))
        .unwrap_or_else(|| "-".into());
    let mut menu = Menu::new();
    for level in s.thinking_levels.iter().copied() {
        let picker = picker.clone();
        let checked = s.thinking_level == Some(level);
        menu = menu.item(
            MenuItem::check(level_slug(level), checked).on_click(move |_, cx| {
                picker.update(cx, |s, cx| s.set_thinking(level, cx));
            }),
        );
    }
    DropdownMenu::new("composer-thinking", label, menu)
}

/// `12 轮 · 3.4M tok · 缓存命中 99.6% · 上下文 61%`, or nothing before a run.
fn stats_caption(s: &AppState, cx: &App) -> Option<SharedString> {
    let stats = s.stats.as_ref()?;
    let turns = stats.user_messages + stats.assistant_messages;
    let mut parts: Vec<String> = Vec::new();
    if turns > 0 {
        parts.push(i18n::t_arg(cx, "status_turns", "n", &turns.to_string()).to_string());
    }
    if let Some(tokens) = stats.tokens.as_ref() {
        let total = tokens.total_tokens.unwrap_or(tokens.input + tokens.output);
        if total > 0 {
            parts.push(format!("{} tok", compact_count(total)));
            let hit = cache_hit_percent(tokens.input, tokens.cache_read);
            if let Some(hit) = hit {
                parts.push(i18n::t_arg(cx, "status_cache", "p", &hit).to_string());
            }
        }
    }
    if let Some(ctx) = stats.context_usage.as_ref()
        && let Some(percent) = ctx.percent
    {
        parts.push(i18n::t_arg(cx, "status_ctx", "p", &format!("{percent:.0}")).to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · ").into())
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
