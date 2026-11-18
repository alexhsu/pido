//! The right panel's branch tree: the open session's fork structure as an
//! indented list. Read-only — pi's RPC cannot move the leaf, so in-session
//! navigation stays out of reach — but every branch is visible with its
//! summaries, and the active branch's user prompts can fork (pi's one
//! branching move) straight from a node.

use ely_gpui_component::buttons::{ButtonVariant, IconButton};
use ely_gpui_component::primitives::IconName;
use ely_gpui_component::theme::{ActiveTheme, ControlSize, TextSize};
use ely_gpui_component::typography::{Caption, Ellipsis, Paragraph};
use gpui::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div,
};

use crate::i18n;
use crate::state::{AppState, TreeRow};

/// What a row draws as, decided once from the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    User,
    Assistant,
    Tool,
    Bash,
    /// A `session_info` rename marker or an unknown entry type: structural,
    /// kept so the branch picture stays whole, but shown barely.
    Marker,
    Compaction,
    BranchSummary,
}

/// The owned snapshot of one row — full entries carry whole messages, and
/// cloning those per frame is not worth it.
struct RowView {
    depth: usize,
    kind: RowKind,
    on_active: bool,
    /// Entry id, for the fork affordance.
    id: SharedString,
    text: SharedString,
}

impl RowView {
    fn from(row: &TreeRow) -> Self {
        let entry = &row.entry;
        let (kind, text) = match entry.entry_type.as_str() {
            "message" => match entry.message.as_ref() {
                Some(pi_rpc::protocol::AgentMessage::User(message)) => {
                    (RowKind::User, user_text(message))
                }
                Some(pi_rpc::protocol::AgentMessage::Assistant(message)) => {
                    (RowKind::Assistant, assistant_text(message))
                }
                Some(pi_rpc::protocol::AgentMessage::ToolResult(message)) => {
                    (RowKind::Tool, message.tool_name.clone().into())
                }
                Some(pi_rpc::protocol::AgentMessage::BashExecution(message)) => {
                    (RowKind::Bash, message.command.clone().into())
                }
                _ => (RowKind::Marker, SharedString::from("")),
            },
            "compaction" => (RowKind::Compaction, excerpt(entry.summary.as_deref())),
            "branch_summary" => (RowKind::BranchSummary, excerpt(entry.summary.as_deref())),
            _ => (
                RowKind::Marker,
                SharedString::from(entry.entry_type.as_str()),
            ),
        };
        Self {
            depth: row.depth,
            kind,
            on_active: row.on_active,
            id: entry.id.clone().into(),
            text,
        }
    }
}

/// A user prompt's text: plain, or the text blocks of a multipart one.
fn user_text(message: &pi_rpc::protocol::UserMessage) -> SharedString {
    match &message.content {
        pi_rpc::protocol::UserContent::Text(text) => excerpt(Some(text)),
        pi_rpc::protocol::UserContent::Blocks(blocks) => {
            let joined = blocks
                .iter()
                .filter_map(|block| match block {
                    pi_rpc::protocol::UserContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            excerpt(Some(&joined))
        }
        pi_rpc::protocol::UserContent::Other(_) => SharedString::from(""),
    }
}

/// An assistant answer's text blocks; thinking and tool calls stay out.
fn assistant_text(message: &pi_rpc::protocol::AssistantMessage) -> SharedString {
    let joined = message
        .content
        .iter()
        .filter_map(|block| match block {
            pi_rpc::protocol::AssistantContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    excerpt(Some(&joined))
}

/// One or two lines' worth, enough to recognize the node.
fn excerpt(text: Option<&str>) -> SharedString {
    let text = text.unwrap_or_default();
    let mut out: String = text.chars().take(160).collect();
    if text.chars().count() > 160 {
        out.push('…');
    }
    out.into()
}

/// The tab's body: the indented tree, or a quiet word when there is no
/// session (or nothing to draw).
pub fn render(state: &Entity<AppState>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let has_session = state.read(cx).session_file.is_some();
    let rows: Vec<RowView> = {
        let s = state.read(cx);
        s.session_tree
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(RowView::from)
            .collect()
    };

    if rows.is_empty() {
        let label = if has_session {
            "tree_empty"
        } else {
            "tree_no_session"
        };
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme.text_size(TextSize::Sm))
            .text_color(colors.fg_subtle)
            .child(Caption::new(i18n::t(cx, label)))
            .into_any_element();
    }

    let list = rows
        .into_iter()
        .enumerate()
        .map(|(ix, row)| tree_row(ix, row, state, cx))
        .collect::<Vec<_>>();
    div()
        .id("tree-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .overflow_x_hidden()
        .flex()
        .flex_col()
        .py_2()
        .children(list)
        .into_any_element()
}

/// One branch node, indented by depth; active-branch rows keep full color
/// and the user ones carry the fork affordance on hover.
fn tree_row(ix: usize, row: RowView, state: &Entity<AppState>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors.clone();

    let (glyph, ink) = match row.kind {
        RowKind::User => (
            "›",
            if row.on_active {
                colors.fg
            } else {
                colors.fg_muted
            },
        ),
        RowKind::Assistant => (
            "·",
            if row.on_active {
                colors.fg_muted
            } else {
                colors.fg_subtle
            },
        ),
        RowKind::Tool | RowKind::Bash => ("◇", colors.fg_subtle),
        RowKind::Marker => ("", colors.fg_subtle),
        RowKind::Compaction | RowKind::BranchSummary => ("≡", colors.fg_subtle),
    };

    let group = SharedString::from(format!("tree-row-{ix}"));
    let indent = row.depth * 14;
    let mut line = div()
        .group(group.clone())
        .flex()
        .items_center()
        .gap_1()
        .pl(gpui::px(indent as f32 + 12.0))
        .pr_3()
        .py_0p5()
        .min_w_0();

    if !glyph.is_empty() {
        line = line.child(
            div()
                .flex_none()
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(ink)
                .child(glyph),
        );
    }

    let body = match row.kind {
        RowKind::BranchSummary => div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(theme.text_size(TextSize::Xs))
                    .text_color(colors.fg_subtle)
                    .child(Caption::new(i18n::t(cx, "branch_left"))),
            )
            .child(
                div()
                    .text_size(theme.text_size(TextSize::Sm))
                    .text_color(colors.fg_muted)
                    .child(Paragraph::new(row.text.clone())),
            ),
        RowKind::Compaction => div()
            .flex_1()
            .min_w_0()
            .text_size(theme.text_size(TextSize::Sm))
            .text_color(colors.fg_subtle)
            .child(Paragraph::new(format!(
                "{} {}",
                i18n::t(cx, "context_compacted"),
                row.text
            ))),
        _ => div()
            .flex_1()
            .min_w_0()
            .text_size(theme.text_size(if row.kind == RowKind::Marker {
                TextSize::Xs
            } else {
                TextSize::Sm
            }))
            .text_color(ink)
            .child(Ellipsis::new(row.text.clone())),
    };
    line = line.child(body);

    // Forking is pi's one branch move and only reaches user prompts on the
    // active branch; other nodes inform, but cannot branch.
    if row.kind == RowKind::User && row.on_active {
        let state = state.clone();
        line = line.child(
            div()
                .flex_none()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(
                    IconButton::new(("tree-fork", ix), IconName::GitBranch)
                        .variant(ButtonVariant::Ghost)
                        .size(ControlSize::Sm)
                        .tooltip(i18n::t(cx, "tree_fork_here"))
                        .on_click(move |_, _, cx| {
                            state.update(cx, |s, cx| s.fork_entry(&row.id, cx));
                        }),
                ),
        );
    }

    line.into_any_element()
}
