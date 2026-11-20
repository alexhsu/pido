//! The right panel's branch tree: the open session's fork structure as an
//! indented list. Read-only — pi's RPC cannot move the leaf, so in-session
//! navigation stays out of reach — but every branch is visible with its
//! summaries, and the active branch's user prompts can fork (pi's one
//! branching move) straight from a node.
//!
//! The tree draws **branches, not the chain**: a real session is a linear
//! run of entries (each entry's parent is the one before it), so rows are
//! indented by the number of *forked* ancestors and entries without
//! displayable content — tool results, model switches, thinking-only
//! assistants, structural markers — are skipped. Drawing every entry at
//! its parent-chain depth produced a staircase that ran off the panel.

use ely_gpui_component::buttons::{ButtonVariant, IconButton};
use ely_gpui_component::primitives::IconName;
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, TextSize};
use ely_gpui_component::typography::{Caption, Ellipsis, Paragraph};
use gpui::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::i18n;
use crate::state::{AppState, TreeRow};

/// What a row draws as, decided once from the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    User,
    Assistant,
    Compaction,
    BranchSummary,
}

/// The owned snapshot of one row — full entries carry whole messages, and
/// cloning those per frame is not worth it.
struct RowView {
    depth: usize,
    kind: RowKind,
    on_active: bool,
    /// How many branches grow from this node; more than one draws a fork
    /// marker with the count.
    children: usize,
    /// Entry id, for the fork affordance.
    id: SharedString,
    text: SharedString,
}

impl RowView {
    /// `None` for entries the tree does not draw — anything without a
    /// prompt, an answer, or a summary is the transcript's job.
    fn from(row: &TreeRow) -> Option<Self> {
        let entry = &row.entry;
        let (kind, text) = match entry.entry_type.as_str() {
            "message" => match entry.message.as_ref() {
                Some(pi_rpc::protocol::AgentMessage::User(message)) => {
                    (RowKind::User, user_text(message))
                }
                Some(pi_rpc::protocol::AgentMessage::Assistant(message)) => {
                    (RowKind::Assistant, assistant_text(message))
                }
                _ => return None,
            },
            "compaction" => (RowKind::Compaction, excerpt(entry.summary.as_deref())),
            "branch_summary" => (RowKind::BranchSummary, excerpt(entry.summary.as_deref())),
            _ => return None,
        };
        // An empty prompt or a thinking-only answer draws nothing but a
        // blank line, and a blank line here reads as a bug (they clustered
        // into what looked like gaps in the first cut).
        if text.is_empty() {
            return None;
        }
        Some(Self {
            depth: row.depth,
            kind,
            on_active: row.on_active,
            children: row.children,
            id: entry.id.clone().into(),
            text,
        })
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

/// One line's worth, enough to recognize the node. Newlines MUST go: the
/// drawing path (Ellipsis's measurement copy, then gpui's text shaping)
/// asserts on them and took the whole app down once.
fn excerpt(text: Option<&str>) -> SharedString {
    let flat = text
        .unwrap_or_default()
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let mut out: String = flat.chars().take(160).collect();
    if flat.chars().count() > 160 {
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
            .filter_map(RowView::from)
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

    // The leaf badge sits on the last drawn row pi would run — the tree's
    // "you are here", even though that entry itself may not be drawn.
    let leaf_ix = rows
        .iter()
        .rposition(|row| row.on_active)
        .unwrap_or(usize::MAX);
    let list = rows
        .into_iter()
        .enumerate()
        .map(|(ix, row)| tree_row(ix, row, ix == leaf_ix, state, cx))
        .collect::<Vec<_>>();
    div()
        .id("tree-scroll")
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .py_2()
        .children(list)
        .into_any_element()
}

/// One branch node: a 2px accent bar marks the active path, the indent
/// steps once per real fork, and forked nodes carry a branch marker with
/// their count. The leaf gets a dot — where pi is working right now.
fn tree_row(
    ix: usize,
    row: RowView,
    is_leaf: bool,
    state: &Entity<AppState>,
    cx: &App,
) -> AnyElement {
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
        RowKind::Compaction | RowKind::BranchSummary => ("≡", colors.fg_subtle),
    };

    let group = SharedString::from(format!("tree-row-{ix}"));
    let mut line = div()
        .group(group.clone())
        .flex()
        .items_center()
        .gap_1()
        // The bar keeps its slot on every row so text alignment never
        // shifts between active and inactive ones.
        .border_l_2()
        .border_color(if row.on_active {
            colors.info
        } else {
            gpui::transparent_black()
        })
        .pl(px(10.0 + row.depth as f32 * 16.0))
        .pr_3()
        .py_0p5()
        .w_full()
        .min_w_0()
        .child(
            div()
                .flex_none()
                .text_size(theme.text_size(if row.kind == RowKind::Assistant {
                    TextSize::Xs
                } else {
                    TextSize::Sm
                }))
                .text_color(ink)
                .child(glyph),
        );

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
        RowKind::User | RowKind::Assistant => div()
            .flex_1()
            .min_w_0()
            .text_size(theme.text_size(if row.kind == RowKind::Assistant {
                TextSize::Xs
            } else {
                TextSize::Sm
            }))
            .text_color(ink)
            .child(Ellipsis::new(row.text.clone())),
    };
    line = line.child(body);

    // A node the user has branched from shows how many ways it forks.
    if row.children > 1 {
        line = line.child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_0p5()
                .text_size(theme.text_size(TextSize::Xs))
                .text_color(colors.fg_subtle)
                .child(
                    ely_gpui_component::primitives::Icon::new(IconName::GitBranch)
                        .size(IconSize::Xs)
                        .color(colors.fg_subtle),
                )
                .child(row.children.to_string()),
        );
    }

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

    if is_leaf {
        line = line.child(
            div()
                .flex_none()
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(colors.info)
                .child("●"),
        );
    }

    line.into_any_element()
}
