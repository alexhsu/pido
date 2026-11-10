//! The composer: Ely's PromptInput wired to prompt/steer/stop.

use ely_gpui_component::chat::PromptInput;
use ely_gpui_component::typography::Caption;
use gpui::{App, IntoElement, ParentElement, Styled, div};

use crate::i18n;
use crate::state::AppState;
use crate::ui::app::PidoApp;

/// Render the composer. While the agent runs, the send button becomes a stop
/// button and Enter sends a steering message instead of a new prompt.
///
/// Spacing and the centered column width come from Ely's `ChatContainer`,
/// which wraps this composer.
pub fn render(state: &gpui::Entity<AppState>, app: &mut PidoApp, cx: &App) -> impl IntoElement {
    let streaming = state.read(cx).streaming;
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

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(input)
        .child(div().flex().justify_end().child(Caption::new(hint)))
        .into_any_element()
}
