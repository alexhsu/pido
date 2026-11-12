//! Application state: one Pi session, its transcript, and the bridge from
//! the RPC client's threads into GPUI's main thread.
//!
//! All blocking work (spawning pi, awaiting responses, scanning sessions)
//! runs on GPUI's background executor; results hop back through
//! `WeakEntity::update`, so the UI never blocks. The event pump subscribes to
//! the client and folds every record into the transcript.

use base64::Engine as _;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ely_gpui_component::git::{DiffLine, LineKind, Stretch, mark_words};
use gpui::{App, AppContext, Context, Entity};
use pi_rpc::client::{ClientEvent, PiClient, PiProcessConfig};
use pi_rpc::protocol::{
    AgentMessage, AssistantMessageEvent, ClearedQueue, Command, Event, Model, Response, RpcRecord,
    SessionEntry, SessionState, SessionStats, StreamingBehavior, ThinkingLevel,
};
use pi_rpc::sessions::{self, SessionGroup};

use crate::i18n;

/// Bound for the three initialization requests. A hung Pi must not leave the
/// window "connecting" for minutes.
const INIT_TIMEOUT: Duration = Duration::from_secs(20);

/// How the connection to pi stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    /// No session yet; the welcome screen owns the window.
    Idle,
    /// A pi process is being spawned and initialized.
    Connecting,
    /// A live session.
    Live,
    /// The connection died; `String` explains why.
    Dead(String),
}

/// Environment probe results for the welcome screen.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// `Some(version)` when the pi CLI was found.
    pub pi_version: Option<String>,
    /// Provider names that answer `pi auth check` with ready.
    pub ready_providers: Vec<String>,
}

/// A line in the transcript.
#[derive(Debug, Clone)]
pub enum TranscriptItem {
    User {
        text: gpui::SharedString,
        /// Wall-clock stamp, unix seconds; drives the hover footer.
        time: Option<i64>,
    },
    /// A turn's status line, sitting between its prompt and its content.
    /// `took` None means the turn is still running (the clock ticks).
    Turn {
        started: Instant,
        took: Option<Duration>,
    },
    Assistant(Box<AssistantView>),
    Tool(ToolView),
    Notice {
        kind: NoticeKind,
        text: String,
    },
}

/// Pi's message timestamps are unix **milliseconds** as f64; older builds
/// may send seconds. Returns seconds, or None for zero/absent stamps.
fn unix_secs(timestamp: f64) -> Option<i64> {
    if timestamp <= 0.0 {
        return None;
    }
    let secs = if timestamp >= 1e11 {
        // Milliseconds: a date after ~5138 CE cannot be a sane seconds value.
        (timestamp / 1000.0) as i64
    } else {
        timestamp as i64
    };
    (secs > 0).then_some(secs)
}

/// Tokens used by one assistant message, from its usage report.
fn message_tokens(usage: Option<&pi_rpc::protocol::Usage>) -> Option<u64> {
    let usage = usage?;
    Some(
        usage
            .total_tokens
            .unwrap_or(usage.input + usage.output + usage.cache_read + usage.cache_write),
    )
}

/// Now, as unix seconds.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct AssistantView {
    /// Text blocks keyed by their streaming `contentIndex`. A message can hold
    /// several blocks around tool calls; a single buffer would let `text_end`
    /// overwrite earlier text mid-stream.
    pub text_blocks: BTreeMap<u32, String>,
    pub thinking_blocks: BTreeMap<u32, String>,
    pub live_text: Option<u32>,
    pub live_thinking: Option<u32>,
    pub finished: bool,
    pub started: Instant,
    pub took: Option<Duration>,
    /// Provider error text for a failed assistant message, when any.
    pub error: Option<String>,
    /// Wall-clock stamp of the finished message, unix seconds.
    pub time: Option<i64>,
    /// Tokens the message's turn reported, from its usage.
    pub tokens: Option<u64>,
    /// Finished messages keep their joined text and parsed blocks, so a
    /// repaint does not re-join strings or re-parse Markdown.
    /// Filled the first time a finished message is drawn: parsing at load
    /// time would stall opening a long session on messages nobody looks at.
    geometry_cache: std::cell::RefCell<Option<CachedMarkdown>>,
    thinking_cache: std::cell::RefCell<Option<gpui::SharedString>>,
}

impl std::fmt::Debug for AssistantView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssistantView")
            .field("blocks", &self.text_blocks.len())
            .field("finished", &self.finished)
            .field("live", &self.live_text.is_some())
            .finish()
    }
}

impl AssistantView {
    fn streaming() -> Self {
        Self {
            text_blocks: BTreeMap::new(),
            thinking_blocks: BTreeMap::new(),
            live_text: None,
            live_thinking: None,
            finished: false,
            started: Instant::now(),
            took: None,
            error: None,
            time: None,
            tokens: None,
            geometry_cache: std::cell::RefCell::new(None),
            thinking_cache: std::cell::RefCell::new(None),
        }
    }

    fn from_message(message: &pi_rpc::protocol::AssistantMessage) -> Self {
        let mut view = Self::streaming();
        for (index, block) in message.content.iter().enumerate() {
            let index = index as u32;
            match block {
                pi_rpc::protocol::AssistantContent::Text { text } => {
                    view.text_blocks.insert(index, text.clone());
                }
                pi_rpc::protocol::AssistantContent::Thinking { thinking } => {
                    view.thinking_blocks.insert(index, thinking.clone());
                }
                pi_rpc::protocol::AssistantContent::ToolCall(_)
                | pi_rpc::protocol::AssistantContent::Unknown => {}
            }
        }
        view.finished = true;
        view.error = message.error_message.clone();
        view.time = unix_secs(message.timestamp);
        view.tokens = message_tokens(message.usage.as_ref());
        view
    }

    /// Rendered text, keeping blocks separated so two blocks never merge.
    pub fn text(&self) -> gpui::SharedString {
        let cached = self.geometry_cache.borrow().clone();
        match cached {
            Some(cached) => cached.text,
            None => join_blocks(&self.text_blocks).into(),
        }
    }

    /// The message's text and parsed blocks, computed on first use and kept.
    ///
    /// `None` while the message is still streaming: those redraw from the
    /// source, which is unavoidable for live text.
    pub fn cached(&self) -> Option<CachedMarkdown> {
        if !self.finished || self.live_text.is_some() {
            return None;
        }
        let mut cache = self.geometry_cache.borrow_mut();
        if cache.is_none() {
            let text: gpui::SharedString = join_blocks(&self.text_blocks).into();
            *cache = Some(CachedMarkdown {
                blocks: parse_markdown(&text),
                text,
            });
        }
        cache.clone()
    }

    pub fn thinking(&self) -> gpui::SharedString {
        let cached = self.thinking_cache.borrow().clone();
        match cached {
            Some(text) => text,
            None => {
                let text: gpui::SharedString = join_blocks(&self.thinking_blocks).into();
                *self.thinking_cache.borrow_mut() = Some(text.clone());
                text
            }
        }
    }
}

/// Fold `cwd` if it is open, unfold it if it is folded.
pub fn toggle_fold(collapsed: &mut std::collections::HashSet<String>, cwd: &str) {
    if !collapsed.remove(cwd) {
        collapsed.insert(cwd.to_string());
    }
}

/// Raw pi flags from `PIDO_EXTRA_ARGS` (e.g. `-e ./my-ext.ts`), for trying
/// extensions without a settings UI.
pub fn extra_args() -> Vec<String> {
    std::env::var("PIDO_EXTRA_ARGS")
        .map(|args| args.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// One row of the sidebar: a project header or a session.
#[derive(Debug, Clone)]
pub enum SidebarRow {
    /// A project directory: its short label, its full path (for the fold and
    /// for starting a session there) and whether it is folded. The archive
    /// fold has an empty `cwd` and its label comes from the locale.
    Group {
        label: gpui::SharedString,
        cwd: gpui::SharedString,
        folded: bool,
        sessions: usize,
        archived: bool,
    },
    /// A session file.
    Session {
        path: gpui::SharedString,
        session_id: gpui::SharedString,
        title: gpui::SharedString,
        /// Last activity, unix seconds: the age label is built at draw time,
        /// so it follows the locale without rebuilding the rows.
        updated: Option<i64>,
        pinned: bool,
        archived: bool,
    },
}

/// A session path as the pin and archive sets compare it.
pub fn session_key(path: &str) -> String {
    let key = path.replace('\\', "/");
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

/// Show a session's file in the platform's file manager, with the file
/// itself picked where the platform supports it.
pub fn reveal_in_folder(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// One session as a sidebar row.
fn session_row(session: &sessions::SessionSummary, pinned: bool, archived: bool) -> SidebarRow {
    let title = session
        .name
        .clone()
        .or_else(|| session.preview.clone())
        .unwrap_or_else(|| session.session_id.clone());
    SidebarRow::Session {
        path: session.path.to_string_lossy().into_owned().into(),
        session_id: session.session_id.clone().into(),
        title: title.into(),
        updated: session.updated_secs,
        pinned,
        archived,
    }
}

/// Flatten grouped sessions into the sidebar's row order: pinned sessions
/// first inside their project, archived ones collected into a fold at the
/// bottom. The archive fold sits under the empty-string key.
fn flatten_groups(
    groups: &[SessionGroup],
    collapsed: &std::collections::HashSet<String>,
    labels: &HashMap<String, String>,
    pinned: &HashSet<String>,
    archived: &HashSet<String>,
) -> Vec<SidebarRow> {
    let mut rows = Vec::with_capacity(
        groups
            .iter()
            .map(|group| group.sessions.len() + 1)
            .sum::<usize>(),
    );
    let mut archived_sessions: Vec<&sessions::SessionSummary> = Vec::new();
    for group in groups {
        let is_archived = |session: &sessions::SessionSummary| {
            archived.contains(&session_key(&session.path.to_string_lossy()))
        };
        // The user's label wins over the folder's tail.
        let label = labels
            .get(&session_key(&group.cwd))
            .cloned()
            .unwrap_or_else(|| shorten_project(&group.cwd));
        let active: Vec<&sessions::SessionSummary> = group
            .sessions
            .iter()
            .filter(|session| !is_archived(session))
            .collect();
        archived_sessions.extend(group.sessions.iter().filter(|session| is_archived(session)));
        let folded = collapsed.contains(&group.cwd);
        rows.push(SidebarRow::Group {
            label: label.into(),
            cwd: group.cwd.clone().into(),
            folded,
            sessions: active.len(),
            archived: false,
        });
        if folded {
            continue;
        }
        // Stable sort: pinned first, everything else in the scanner's order.
        let mut active = active;
        active
            .sort_by_key(|session| !pinned.contains(&session_key(&session.path.to_string_lossy())));
        for session in active {
            let pinned = pinned.contains(&session_key(&session.path.to_string_lossy()));
            rows.push(session_row(session, pinned, false));
        }
    }
    if !archived_sessions.is_empty() {
        archived_sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_secs));
        let folded = collapsed.contains("");
        rows.push(SidebarRow::Group {
            label: "".into(),
            cwd: "".into(),
            folded,
            sessions: archived_sessions.len(),
            archived: true,
        });
        if !folded {
            for session in archived_sessions {
                rows.push(session_row(
                    session,
                    pinned.contains(&session_key(&session.path.to_string_lossy())),
                    true,
                ));
            }
        }
    }
    rows
}

/// Retitle the sidebar row that points at `file`, if the list holds it. Lets
/// a rename show immediately instead of waiting for the next disk scan.
fn patch_session_title(rows: &mut [SidebarRow], file: &str, name: &str) {
    for row in rows {
        if let SidebarRow::Session { path, title, .. } = row
            && path.as_ref() == file
        {
            *title = name.to_string().into();
        }
    }
}

/// `/` separators throughout, so display and keys agree everywhere.
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// A unified patch's file path and its hunks, for the diff viewer. None when
/// the patch holds no hunks.
fn parse_patch(patch: &str) -> Option<(String, Vec<Stretch>)> {
    let (mut old_path, mut new_path) = (None, None);
    let mut hunks = Vec::new();
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("--- ") {
            old_path = patch_path(rest);
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            new_path = patch_path(rest);
        } else if line.starts_with("@@") {
            let (mut old_no, mut new_no) = hunk_starts(line)?;
            let mut body: Vec<DiffLine> = Vec::new();
            while let Some(next) = lines.peek() {
                let next: &str = next;
                // The next hunk, the next file, or the patch's end.
                if next.starts_with("@@") || next.starts_with("--- ") || next.starts_with("+++ ") {
                    break;
                }
                let next = lines.next().unwrap_or_default();
                if next.is_empty() {
                    break;
                }
                let (kind, text) = match next.as_bytes()[0] {
                    b' ' => (LineKind::Same, &next[1..]),
                    b'-' => (LineKind::Removed, &next[1..]),
                    b'+' => (LineKind::Added, &next[1..]),
                    // "\ No newline at end of file" and anything else.
                    _ => continue,
                };
                let (old, new) = match kind {
                    LineKind::Same => {
                        old_no += 1;
                        new_no += 1;
                        (Some(old_no - 1), Some(new_no - 1))
                    }
                    LineKind::Removed => {
                        old_no += 1;
                        (Some(old_no - 1), None)
                    }
                    LineKind::Added => {
                        new_no += 1;
                        (None, Some(new_no - 1))
                    }
                };
                body.push(DiffLine {
                    kind,
                    old,
                    new,
                    text: text.trim_end_matches('\r').to_string().into(),
                    words: Vec::new(),
                });
            }
            if !body.is_empty() {
                mark_words(&mut body);
                hunks.push(Stretch::Hunk {
                    header: line.to_string().into(),
                    lines: body,
                });
            }
        }
    }
    if hunks.is_empty() {
        return None;
    }
    let path = match (old_path, new_path) {
        // Git-style patches prefix the two sides with `a/` and `b/`; Pi's own
        // patches carry the same path on both sides.
        (Some(old), Some(new)) => match (old.strip_prefix("a/"), new.strip_prefix("b/")) {
            (Some(old), Some(new)) if old == new => new.to_string(),
            _ => new,
        },
        (_, Some(new)) => new,
        (Some(old), None) => old,
        (None, None) => return None,
    };
    Some((path, hunks))
}

/// The path a patch header line names, without its timestamp.
fn patch_path(raw: &str) -> Option<String> {
    let raw = raw.split('\t').next().unwrap_or(raw).trim();
    (!raw.is_empty() && raw != "/dev/null").then(|| raw.to_string())
}

/// The old and new start lines of a `@@ -a,b +c,d @@` header.
fn hunk_starts(header: &str) -> Option<(usize, usize)> {
    let rest = header.strip_prefix("@@")?;
    let spec = &rest[..rest.find("@@")?];
    let mut parts = spec.split_whitespace();
    let start = |side: &str| {
        side.split(',')
            .next()
            .and_then(|line| line.parse::<usize>().ok())
    };
    Some((
        start(parts.next()?.strip_prefix('-')?)?,
        start(parts.next()?.strip_prefix('+')?)?,
    ))
}

/// A `write` call's path and content, when its arguments carry both.
fn write_args(args: Option<&serde_json::Value>) -> Option<(String, String)> {
    let args = args?;
    let path = args.get("path")?.as_str()?;
    let content = args.get("content")?.as_str()?;
    Some((path.to_string(), content.to_string()))
}

/// A whole file as added lines: what a `write` shows, since its old text is
/// gone by the time the result arrives.
fn added_stretches(content: &str) -> Vec<Stretch> {
    let lines: Vec<DiffLine> = content
        .lines()
        .enumerate()
        .map(|(ix, text)| DiffLine {
            kind: LineKind::Added,
            old: None,
            new: Some(ix + 1),
            text: text.trim_end_matches('\r').to_string().into(),
            words: Vec::new(),
        })
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    vec![Stretch::Hunk {
        header: format!("@@ -0,0 +1,{} @@", lines.len()).into(),
        lines,
    }]
}

/// Lines added and removed across hunks.
fn stretches_stat(stretches: &[Stretch]) -> (usize, usize) {
    stretches
        .iter()
        .flat_map(|stretch| match stretch {
            Stretch::Hunk { lines, .. } | Stretch::Folded(lines) => lines.as_slice(),
        })
        .fold((0, 0), |(added, removed), line| match line.kind {
            LineKind::Added => (added + 1, removed),
            LineKind::Removed => (added, removed + 1),
            LineKind::Same => (added, removed),
        })
}

/// `D:\develop\code\pido` → `code / pido`, so a project header stays short.
/// The dialog messages reuse it under a display-oriented alias.
pub fn shorten_project_display(path: &str) -> String {
    shorten_project(path)
}
pub fn shorten_project(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    match parts.len() {
        0 => path.to_string(),
        1 => parts[0].to_string(),
        n => format!("{} / {}", parts[n - 2], parts[n - 1]),
    }
}

/// A finished message's text and its parsed blocks.
#[derive(Clone)]
pub struct CachedMarkdown {
    pub text: gpui::SharedString,
    pub blocks: std::rc::Rc<[ely_gpui_component::documents::MarkdownBlock]>,
}

/// Parse Markdown once into the blocks the renderer draws.
pub fn parse_markdown(source: &str) -> std::rc::Rc<[ely_gpui_component::documents::MarkdownBlock]> {
    use ely_gpui_component::documents::{MarkdownReader, markdown_tree, mdast};
    let root = markdown_tree(source);
    let reader = MarkdownReader::joined(source, &root);
    let mut blocks = Vec::new();
    if let mdast::Node::Root(parent) = &root {
        for child in &parent.children {
            reader.block(child, &mut blocks);
        }
    }
    std::rc::Rc::from(blocks.as_slice())
}

fn join_blocks(blocks: &BTreeMap<u32, String>) -> String {
    blocks
        .values()
        .filter(|text| !text.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Debug, Clone)]
pub struct ToolView {
    pub call_id: String,
    pub name: String,
    /// Pretty-printed arguments, when known.
    pub args: Option<gpui::SharedString>,
    pub running: bool,
    pub failed: bool,
    pub result: Option<gpui::SharedString>,
    pub started: Instant,
    pub took: Option<Duration>,
}

/// One file the agent changed in this session, with every edit to it parsed
/// for the diff viewer.
#[derive(Clone)]
pub struct ChangeFile {
    /// Normalized identity: the absolute path with `/` separators, lowercased
    /// on Windows, so relative and absolute reports of one file group up.
    pub key: String,
    /// Shown to the user: relative to the project when it lives inside it.
    pub path: gpui::SharedString,
    pub added: usize,
    pub removed: usize,
    /// Its edits, oldest first, as hunks the diff viewer draws.
    pub stretches: std::rc::Rc<Vec<Stretch>>,
    /// Sequence number of the last edit; files list newest first.
    touched: u64,
}

/// Arguments of a `write` call seen at start, waiting for its result.
struct PendingWrite {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Warn,
    Error,
}

pub struct AppState {
    pub conn: Conn,
    pub environment: Environment,
    pub cwd: Option<PathBuf>,
    pub transcript: Vec<TranscriptItem>,
    /// True between `agent_start` and `agent_settled`.
    pub streaming: bool,
    pub models: Vec<Model>,
    pub current_model: Option<Model>,
    pub thinking_levels: Vec<ThinkingLevel>,
    pub thinking_level: Option<ThinkingLevel>,
    /// Sessions per project, as last scanned. The sidebar draws
    /// `sidebar_rows`, which is this flattened once per scan.
    pub session_groups: Vec<SessionGroup>,
    /// Projects the user folded shut, keyed by working directory. The
    /// archive fold uses the empty string.
    pub collapsed_projects: std::collections::HashSet<String>,
    /// Project folders the user added; they list even without sessions.
    pub user_projects: Vec<String>,
    /// Sidebar labels the user gave projects, keyed like pin/archive sets.
    pub project_labels: HashMap<String, String>,
    /// Sessions the user pinned, as normalized keys.
    pub pinned_sessions: HashSet<String>,
    /// Sessions the user archived, as normalized keys.
    pub archived_sessions: HashSet<String>,
    /// The sidebar's rows, flattened once per scan: a header per project and
    /// a row per session. Rendering reads these instead of rebuilding strings
    /// per frame, and the list virtualises over the same order.
    pub sidebar_rows: Vec<SidebarRow>,
    pub session_name: Option<String>,
    /// The active session's file path, as reported by `get_state`.
    pub session_file: Option<String>,
    /// The active session's id, as reported by `get_state`.
    pub session_id: Option<String>,
    /// The session `new_chat` just started. Pi assigns the file path in
    /// `new_session` but only writes the file with the first assistant
    /// reply, so until a scan finds it the sidebar draws a draft row for it
    /// instead of dropping the new session from the list.
    draft_session_file: Option<String>,
    /// Set by `new_chat` until `get_state` reports the new session's file.
    awaiting_new_session_file: bool,
    /// Set while a startup resume is in flight: once live, load its history.
    last_session_loaded: bool,
    pub stats: Option<SessionStats>,
    /// A dismissible banner over the transcript.
    pub notice: Option<(NoticeKind, String)>,
    /// The last-used project directory, to prefill the welcome picker.
    pub last_project: Option<String>,
    /// Counter for staging image keys.
    next_image_key: u64,
    /// Extension commands, prompt templates and skills pi offers at `/`.
    pub commands: Vec<pi_rpc::protocol::CommandInfo>,
    /// Workspace files offered at `@`, as project-relative paths.
    pub workspace_files: Vec<String>,
    /// Images staged for the next prompt/steer (paste, picker or drop).
    pub pending_images: Vec<PendingImage>,
    /// An extension dialog waiting for the user (select/confirm/input/
    /// editor). None while nothing asks.
    pub pending_dialog: Option<pi_rpc::protocol::ExtensionUiRequest>,
    /// Bumped whenever the conversation is replaced, so the message list gets
    /// a fresh scroll/away state instead of reusing the old session's.
    pub session_epoch: u64,
    /// Steer messages queued by the UI that pi has not delivered yet.
    pending_steers: Vec<String>,
    /// Messages waiting in pi's queues, as `queue_update` reports them.
    pub queued_steers: Vec<gpui::SharedString>,
    pub queued_follow_ups: Vec<gpui::SharedString>,
    /// Turn status lines the user re-opened after completion, keyed by the
    /// turn item's transcript index.
    pub expanded_turns: std::collections::HashSet<u64>,
    /// True between a prompt and its `agent_settled`; drives the one-second
    /// ticker that keeps the working clock moving during silent stretches.
    turn_running: bool,
    /// Files the agent changed in this session, newest first.
    pub changes: Vec<ChangeFile>,
    /// Counter stamped on each recorded change.
    changes_seq: u64,
    /// `write` calls whose arguments arrived but whose result has not.
    pending_writes: HashMap<String, PendingWrite>,
    /// Text Esc or a failed send wants put back into the composer. The root
    /// view drains this into the draft field.
    pub pending_composer_text: Option<String>,
    /// Cached session scan keyed by file metadata.
    session_scan: Arc<Mutex<sessions::SessionScanner>>,
    client: Option<PiClient>,
}

impl AppState {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let persisted = crate::settings::Settings::load();
        let last_project = persisted.last_project;
        let user_projects = persisted.projects.clone();
        let pinned_sessions = persisted
            .pinned_sessions
            .iter()
            .map(|path| session_key(path))
            .collect();
        let archived_sessions = persisted
            .archived_sessions
            .iter()
            .map(|path| session_key(path))
            .collect();
        cx.new(|cx| {
            // The archive fold starts shut; it only lists when something is in it.
            let mut collapsed = HashSet::new();
            collapsed.insert(String::new());
            let state = Self {
                conn: Conn::Idle,
                environment: Environment::default(),
                cwd: None,
                transcript: Vec::new(),
                streaming: false,
                models: Vec::new(),
                current_model: None,
                thinking_levels: Vec::new(),
                thinking_level: None,
                session_groups: Vec::new(),
                sidebar_rows: Vec::new(),
                collapsed_projects: collapsed,
                user_projects,
                project_labels: persisted.project_labels.iter().cloned().collect(),
                pinned_sessions,
                archived_sessions,
                session_name: None,
                session_file: None,
                session_id: None,
                draft_session_file: None,
                awaiting_new_session_file: false,
                last_session_loaded: false,
                stats: None,
                notice: None,
                last_project,
                next_image_key: 1,
                commands: Vec::new(),
                workspace_files: Vec::new(),
                pending_images: Vec::new(),
                pending_dialog: None,
                session_epoch: 0,
                pending_steers: Vec::new(),
                queued_steers: Vec::new(),
                queued_follow_ups: Vec::new(),
                expanded_turns: std::collections::HashSet::new(),
                turn_running: false,
                changes: Vec::new(),
                changes_seq: 0,
                pending_writes: HashMap::new(),
                pending_composer_text: None,
                session_scan: Arc::new(Mutex::new(sessions::SessionScanner::default())),
                client: None,
            };
            state.probe_environment(cx);
            state
        })
    }

    fn client(&self) -> Option<PiClient> {
        self.client.clone()
    }

    /// Take text a cancelled/failed action wants restored to the composer.
    pub fn take_pending_composer_text(&mut self) -> Option<String> {
        self.pending_composer_text.take()
    }

    // -- environment --------------------------------------------------------

    /// Probe the pi CLI and provider auth in the background for the welcome
    /// screen. Only providers present in `auth.json` (plus the common ones
    /// for env-var auth) are checked, in parallel so startup is not serialized
    /// behind several Node processes.
    fn probe_environment(&self, cx: &Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let Some(pi_version) = pi_rpc::client::probe_pi_install(None) else {
                        return Environment::default();
                    };
                    let providers = probe_providers();
                    let ready_providers = std::thread::scope(|scope| {
                        let handles = providers
                            .into_iter()
                            .map(|provider| {
                                scope.spawn(move || {
                                    let ready = pi_rpc::client::probe_auth(&provider, None)
                                        .unwrap_or(false);
                                    (provider, ready)
                                })
                            })
                            .collect::<Vec<_>>();
                        handles
                            .into_iter()
                            .filter_map(|handle| handle.join().ok())
                            .filter_map(|(provider, ready)| ready.then_some(provider))
                            .collect()
                    });
                    Environment {
                        pi_version: Some(pi_version),
                        ready_providers,
                    }
                })
                .await;
            this.update(cx, |s, cx| {
                s.environment = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -- session lifecycle --------------------------------------------------

    /// Spawn pi in `cwd` and initialize the session. When `resume_session`
    /// names an existing session file it is loaded into pi at startup and its
    /// transcript is restored, so the app reopens where the user left off.
    pub fn connect(
        &mut self,
        cwd: PathBuf,
        resume_session: Option<String>,
        extra_args: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if !cwd.is_dir() {
            self.notice = Some((NoticeKind::Error, i18n::t(cx, "dir_invalid").to_string()));
            cx.notify();
            return;
        }
        self.conn = Conn::Connecting;
        self.cwd = Some(cwd.clone());
        self.transcript.clear();
        self.expanded_turns.clear();
        self.turn_running = false;
        self.clear_changes();
        self.notice = None;
        self.streaming = false;
        self.stats = None;
        self.session_name = None;
        self.session_epoch += 1;
        self.pending_steers.clear();
        self.queued_steers.clear();
        self.queued_follow_ups.clear();
        self.pending_composer_text = None;
        self.last_project = Some(cwd.to_string_lossy().into_owned());
        crate::settings::save_setting(|s| {
            s.last_project = self.last_project.clone();
        });
        cx.notify();

        // A stale session file must not break the spawn; fall back to fresh.
        let mut args: Vec<String> = Vec::new();
        self.last_session_loaded = false;
        self.session_file = None;
        if let Some(session) = resume_session
            && std::path::Path::new(&session).is_file()
        {
            log::info!("state: resuming session {session}");
            args.push("--session".into());
            args.push(session.clone());
            self.session_file = Some(session);
            self.last_session_loaded = true;
        }
        args.extend(extra_args);
        let config = PiProcessConfig::new(&cwd).with_args(args);
        log::info!("state: connecting in {}", config.cwd.display());
        cx.spawn(async move |this, cx| {
            let spawned = cx
                .background_executor()
                .spawn(async move { PiClient::spawn(&config) })
                .await;
            match spawned {
                Ok(client) => {
                    log::info!("state: pi spawned, running init requests");
                    // Run the three independent round trips in parallel, each
                    // with a short timeout. Sequentially they could stall the
                    // UI behind 3 x 120s if Pi is wedged.
                    let (state_result, models_result, levels_result) =
                        std::thread::scope(|scope| {
                            let state_client = client.clone();
                            let models_client = client.clone();
                            let levels_client = client.clone();
                            let state = scope.spawn(move || {
                                state_client.request_with_timeout(Command::GetState, INIT_TIMEOUT)
                            });
                            let models = scope.spawn(move || {
                                models_client
                                    .request_with_timeout(Command::GetAvailableModels, INIT_TIMEOUT)
                            });
                            let levels = scope.spawn(move || {
                                levels_client.request_with_timeout(
                                    Command::GetAvailableThinkingLevels,
                                    INIT_TIMEOUT,
                                )
                            });
                            (
                                state
                                    .join()
                                    .unwrap_or_else(|_| Err(anyhow::anyhow!("get_state panicked"))),
                                models.join().unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("get_available_models panicked"))
                                }),
                                levels.join().unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("get_available_thinking_levels panicked"))
                                }),
                            )
                        });
                    log::info!(
                        "state: init done, get_state ok={}",
                        state_result.as_ref().map(|r| r.success).unwrap_or(false)
                    );
                    this.update(cx, |s, cx| {
                        let failed = state_result.as_ref().err().map(|e| e.to_string());
                        log::info!("state: applying connect result, failed={failed:?}");
                        s.client = Some(client.clone());
                        match failed {
                            Some(error) => {
                                let stderr = client.last_stderr();
                                s.conn = Conn::Dead(if stderr.is_empty() {
                                    error
                                } else {
                                    format!("{error}\n{stderr}")
                                });
                            }
                            None => {
                                s.conn = Conn::Live;
                                if let Ok(Ok(Some(payload))) =
                                    state_result.map(|r| r.data_as::<SessionState>())
                                {
                                    s.current_model = payload.model;
                                    s.thinking_level = payload.thinking_level;
                                    s.session_name = payload.session_name;
                                    s.session_id = payload.session_id;
                                    s.streaming = payload.is_streaming;
                                    s.remember_session_file(payload.session_file.as_deref());
                                }
                                // A fresh pi process owns no draft session;
                                // the previous one died with its file.
                                s.draft_session_file = None;
                                s.awaiting_new_session_file = false;
                                if let Ok(Ok(Some(payload))) = models_result
                                    .map(|r| r.data_as::<pi_rpc::protocol::AvailableModels>())
                                {
                                    s.models = payload.models;
                                }
                                if let Ok(Ok(Some(payload))) = levels_result.map(|r| {
                                    r.data_as::<pi_rpc::protocol::AvailableThinkingLevels>()
                                }) {
                                    s.thinking_levels = payload.levels;
                                    s.thinking_levels
                                        .retain(|level| *level != ThinkingLevel::Unknown);
                                }
                                s.start_pump(client.clone(), cx);
                                // A resumed session opens with its history;
                                // a fresh one starts empty.
                                if s.last_session_loaded {
                                    s.last_session_loaded = false;
                                    s.load_history(cx);
                                }
                                s.load_commands(cx);
                                s.scan_workspace_files(cx);
                                s.refresh_stats(cx);
                            }
                        }
                        s.refresh_sessions(cx);
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => {
                    this.update(cx, |s, cx| {
                        s.conn = Conn::Dead(error.to_string());
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Persist the active session file so the next launch resumes it.
    ///
    /// Pi reports a session path as soon as a session exists, but only creates
    /// the file on the first message. Persist just paths that are on disk; an
    /// empty session must not become a resume target that fails the file
    /// existence check at startup.
    fn remember_session_file(&mut self, session_file: Option<&str>) {
        self.session_file = session_file.map(str::to_string);
        let persisted = session_file.filter(|path| std::path::Path::new(path).is_file());
        crate::settings::save_setting(|s| {
            s.last_session = persisted.map(str::to_string);
        });
    }

    /// The sidebar's rows from the last scan and the current pin/archive sets.
    /// The unsaved session `new_chat` just opened is merged in as a draft so
    /// it shows before pi writes its file.
    fn sidebar_rows_now(&self) -> Vec<SidebarRow> {
        let Some(draft) = self.draft_summary() else {
            return flatten_groups(
                &self.session_groups,
                &self.collapsed_projects,
                &self.project_labels,
                &self.pinned_sessions,
                &self.archived_sessions,
            );
        };
        // New activity leads its project, and that project leads the list;
        // the scanner orders things the same way once the file exists.
        let project = session_key(&draft.cwd);
        let mut groups = self.session_groups.clone();
        match groups
            .iter()
            .position(|group| session_key(&group.cwd) == project)
        {
            Some(ix) => {
                let mut group = groups.remove(ix);
                group.sessions.insert(0, draft);
                groups.insert(0, group);
            }
            None => groups.insert(
                0,
                SessionGroup {
                    cwd: draft.cwd.clone(),
                    sessions: vec![draft],
                },
            ),
        }
        flatten_groups(
            &groups,
            &self.collapsed_projects,
            &self.project_labels,
            &self.pinned_sessions,
            &self.archived_sessions,
        )
    }

    /// The draft row for the session `new_chat` just opened: pi already knows
    /// the file path but has not written the file, so a scan cannot list it.
    /// None once the scan holds that path.
    fn draft_summary(&self) -> Option<sessions::SessionSummary> {
        let path = self.draft_session_file.as_deref()?;
        let cwd = self.cwd.as_ref()?.to_string_lossy().into_owned();
        let key = session_key(path);
        let listed = self.session_groups.iter().any(|group| {
            group
                .sessions
                .iter()
                .any(|session| session_key(&session.path.to_string_lossy()) == key)
        });
        if listed {
            return None;
        }
        Some(sessions::SessionSummary {
            path: PathBuf::from(path),
            session_id: self.session_id.clone().unwrap_or_default(),
            cwd,
            timestamp: None,
            updated_secs: Some(now_secs()),
            name: self.session_name.clone(),
            preview: None,
            message_count: 0,
        })
    }

    /// Whether `path` is the unsaved session `new_chat` just opened: its row
    /// keeps a draft title until pi writes the file.
    pub fn is_draft_session(&self, path: &str) -> bool {
        self.draft_session_file
            .as_deref()
            .is_some_and(|draft| session_key(draft) == session_key(path))
    }

    /// One clock per second while a turn runs, so the working time advances
    /// even when the model is quietly thinking and no deltas arrive.
    fn spawn_turn_ticker(&self, cx: &Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let awake = this.update(cx, |s, cx| {
                    if s.turn_running {
                        cx.notify();
                        true
                    } else {
                        false
                    }
                });
                if !awake.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Freeze the running turn's clock; a no-op when nothing is running.
    fn finish_running_turn(&mut self) {
        self.turn_running = false;
        for item in self.transcript.iter_mut().rev() {
            if let TranscriptItem::Turn { started, took } = item {
                if took.is_none() {
                    *took = Some(started.elapsed());
                }
                break;
            }
        }
    }

    /// Fold or unfold a project in the sidebar. An empty `cwd` folds the
    /// archive list.
    pub fn toggle_project(&mut self, cwd: &str, cx: &mut Context<Self>) {
        toggle_fold(&mut self.collapsed_projects, cwd);
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Add a project folder to the sidebar. It keeps its place across
    /// restarts and shows even before it has any sessions; the folder's own
    /// name is its label, as `shorten_project` renders it.
    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let cwd = match path.canonicalize() {
            Ok(cwd) => cwd.to_string_lossy().into_owned(),
            Err(error) => {
                self.push_notice(NoticeKind::Error, format!("add project: {error}"));
                cx.notify();
                return;
            }
        };
        let known = self.user_projects.iter().any(|project| project == &cwd)
            || self.session_groups.iter().any(|group| group.cwd == cwd);
        if !known {
            self.user_projects.push(cwd.clone());
            crate::settings::save_setting(|s| s.projects = self.user_projects.clone());
        }
        // The scan merges it in; if it is already there the list just unfolds
        // to it.
        self.collapsed_projects.remove(&cwd);
        self.refresh_sessions(cx);
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Drop a project from the sidebar: forget the user-added entry, and
    /// fold bookkeeping for it. The sessions stay on disk; only the added
    /// project row goes away.
    pub fn remove_project(&mut self, cwd: &str, cx: &mut Context<Self>) {
        self.user_projects
            .retain(|project| session_key(project) != session_key(cwd));
        crate::settings::save_setting(|s| s.projects = self.user_projects.clone());
        self.collapsed_projects.remove(cwd);
        self.refresh_sessions(cx);
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Rename a project's sidebar label. Projects take their name from the
    /// folder, so this is an alias: stored per project, shown instead of the
    /// folder's tail.
    pub fn rename_project(&mut self, cwd: &str, name: &str, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.project_labels.insert(session_key(cwd), name.clone());
        let labels: Vec<(String, String)> = self
            .project_labels
            .iter()
            .map(|(key, label)| (key.clone(), label.clone()))
            .collect();
        crate::settings::save_setting(|s| s.project_labels = labels);
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Fold or unfold every project group at once (the header's chevron).
    pub fn set_all_projects_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        self.collapsed_projects.clear();
        if folded {
            for group in &self.session_groups {
                if group.cwd.is_empty() {
                    continue; // the archive fold keeps its own state
                }
                self.collapsed_projects.insert(group.cwd.clone());
            }
            self.collapsed_projects.insert(String::new());
        }
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Whether every project group is currently folded (drives the header's
    /// chevron).
    pub fn all_projects_folded(&self) -> bool {
        self.session_groups
            .iter()
            .all(|group| group.cwd.is_empty() || self.collapsed_projects.contains(&group.cwd))
    }

    /// Pin or unpin a session; pinned sessions lead their project group.
    pub fn toggle_pin(&mut self, path: &str, cx: &mut Context<Self>) {
        let key = session_key(path);
        if !self.pinned_sessions.remove(&key) {
            self.pinned_sessions.insert(key);
        }
        let mut pinned: Vec<String> = self.pinned_sessions.iter().cloned().collect();
        pinned.sort();
        crate::settings::save_setting(|s| s.pinned_sessions = pinned);
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Move a session into or out of the archive fold.
    pub fn toggle_archive(&mut self, path: &str, cx: &mut Context<Self>) {
        let key = session_key(path);
        // Remove returns whether it was there; absent means it now archives.
        if !self.archived_sessions.remove(&key) {
            self.archived_sessions.insert(key);
        }
        let mut list: Vec<String> = self.archived_sessions.iter().cloned().collect();
        list.sort();
        crate::settings::save_setting(|s| s.archived_sessions = list);
        self.sidebar_rows = self.sidebar_rows_now();
        cx.notify();
    }

    /// Rename a session by path: the open one through Pi, any other by
    /// appending a `session_info` entry to its file.
    pub fn rename_session_at(&mut self, path: &str, name: String, cx: &mut Context<Self>) {
        let open = self
            .session_file
            .as_deref()
            .is_some_and(|current| session_key(current) == session_key(path));
        if open {
            self.rename_session(name, cx);
            return;
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        match sessions::append_session_name(std::path::Path::new(path), &name) {
            Ok(()) => {
                patch_session_title(&mut self.sidebar_rows, path, &name);
                self.refresh_sessions(cx);
            }
            Err(error) => self.push_notice(NoticeKind::Error, format!("rename: {error}")),
        }
        cx.notify();
    }

    /// Ask pi to drop the queued steering and follow-up messages, leaving the
    /// run itself alone.
    pub fn clear_queue(&mut self, cx: &mut Context<Self>) {
        self.check_command(Command::ClearQueue, "clear_queue", cx);
        self.queued_steers.clear();
        self.queued_follow_ups.clear();
        cx.notify();
    }

    /// Rename the session. Pi keeps the name in the session file and reports
    /// it in `get_state`, so the header and the sidebar both follow.
    pub fn rename_session(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.session_name = Some(name.clone());
        // The sidebar is rebuilt from disk later; patch the open session's row
        // now so the rename shows up without waiting for pi to write it.
        if let Some(file) = self.session_file.as_deref() {
            patch_session_title(&mut self.sidebar_rows, file, &name);
        }
        self.check_command(Command::SetSessionName { name }, "set_session_name", cx);
        self.refresh_sessions(cx);
        cx.notify();
    }

    /// Fold a restored tool result into the changes panel. Assistant tool
    /// calls are seen before their results on the branch, so `write` content
    /// is available by the time its result arrives.
    fn record_history_change(
        &mut self,
        result: &pi_rpc::protocol::ToolResultMessage,
        calls: &HashMap<String, pi_rpc::protocol::ToolCall>,
    ) {
        match result.tool_name.as_str() {
            "edit" => {
                let patch = result
                    .details
                    .as_ref()
                    .and_then(|details| details.get("patch"))
                    .and_then(|patch| patch.as_str());
                match patch {
                    Some(patch) => self.record_patch(patch),
                    None => log::debug!("restored edit held no patch"),
                }
            }
            "write" => {
                if let Some(call) = calls.get(&result.tool_call_id)
                    && let (Some(path), Some(content)) = (
                        call.arguments.get("path").and_then(|value| value.as_str()),
                        call.arguments
                            .get("content")
                            .and_then(|value| value.as_str()),
                    )
                {
                    self.record_write(path, content);
                }
            }
            _ => {}
        }
    }

    /// Record a finished `edit` tool call from the unified patch Pi reports.
    fn record_patch(&mut self, patch: &str) {
        match parse_patch(patch) {
            Some((path, hunks)) => self.add_change(&path, hunks),
            None => log::debug!("edit result held no patch to show"),
        }
    }

    /// Record a finished `write` tool call; its new text is all that is left.
    fn record_write(&mut self, path: &str, content: &str) {
        let hunks = added_stretches(content);
        if !hunks.is_empty() {
            self.add_change(path, hunks);
        }
    }

    /// Merge one file's hunks into the panel's list, newest file first.
    fn add_change(&mut self, path: &str, hunks: Vec<Stretch>) {
        let key = self.change_key(path);
        let display = self.display_path(path);
        let (added, removed) = stretches_stat(&hunks);
        self.changes_seq += 1;
        let touched = self.changes_seq;
        if let Some(existing) = self.changes.iter_mut().find(|change| change.key == key) {
            existing.added += added;
            existing.removed += removed;
            let mut all = (*existing.stretches).clone();
            all.extend(hunks);
            existing.stretches = std::rc::Rc::new(all);
            existing.touched = touched;
        } else {
            self.changes.push(ChangeFile {
                key,
                path: display.into(),
                added,
                removed,
                stretches: std::rc::Rc::new(hunks),
                touched,
            });
        }
        self.changes
            .sort_by_key(|change| std::cmp::Reverse(change.touched));
    }

    /// A file's identity: absolute, `/`-separated, lowercased on Windows.
    fn change_key(&self, path: &str) -> String {
        let absolute = if Path::new(path).is_absolute() {
            path.to_string()
        } else if let Some(cwd) = &self.cwd {
            cwd.join(path).to_string_lossy().into_owned()
        } else {
            path.to_string()
        };
        let key = normalize_path(&absolute);
        if cfg!(windows) {
            key.to_lowercase()
        } else {
            key
        }
    }

    /// The path to show: relative to the project when the file lives inside
    /// it, `/`-separated either way.
    fn display_path(&self, path: &str) -> String {
        let file = PathBuf::from(path);
        if file.is_absolute() {
            if let Some(cwd) = &self.cwd
                && let Ok(rest) = file.strip_prefix(cwd)
            {
                return normalize_path(&rest.to_string_lossy());
            }
            // Outside the project, the tail is what tells files apart.
            let normalized = normalize_path(path);
            let mut parts = normalized.rsplit('/');
            let name = parts.next().unwrap_or(&normalized);
            return match parts.next() {
                Some(dir) => format!("…/{dir}/{name}"),
                None => normalized,
            };
        }
        path.trim_start_matches("./").replace('\\', "/")
    }

    /// Forget the session's file changes as its transcript goes.
    fn clear_changes(&mut self) {
        self.changes.clear();
        self.changes_seq = 0;
        self.pending_writes.clear();
    }

    /// Start a fresh session in `project`: a new chat when pi is already there,
    /// a reconnect to that directory otherwise.
    pub fn new_session_in(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        if self.conn == Conn::Live && self.cwd.as_deref() == Some(project.as_path()) {
            self.new_chat(cx);
        } else {
            self.connect(project, None, extra_args(), cx);
        }
    }

    /// Stage an image for the next prompt. Returns false when the bytes are
    /// not a format pi accepts or the file is unreasonably large.
    pub fn stage_image(&mut self, name: Option<String>, bytes: Vec<u8>) -> bool {
        let Some((mime, ext)) = sniff_image(&bytes) else {
            self.push_notice(NoticeKind::Error, "unsupported image format".into());
            return false;
        };
        if bytes.len() > MAX_IMAGE_BYTES {
            self.push_notice(
                NoticeKind::Error,
                format!(
                    "image too large ({} MB > {} MB)",
                    bytes.len() / 1024 / 1024,
                    MAX_IMAGE_BYTES / 1024 / 1024
                ),
            );
            return false;
        }
        let key = format!("img-{}", self.next_image_key);
        self.next_image_key += 1;
        let name = name.unwrap_or_else(|| format!("{key}.{ext}"));

        // The bytes go to a temp file so the chip can show a real thumbnail;
        // providers cap images well below this anyway.
        let preview = preview_dir().join(format!("{key}.{ext}"));
        if std::fs::create_dir_all(preview.parent().unwrap_or(std::path::Path::new("."))).is_ok()
            && let Err(error) = std::fs::write(&preview, &bytes)
        {
            log::warn!("attachment preview write failed: {error}");
        }

        self.pending_images.push(PendingImage {
            key: key.clone(),
            name,
            mime: mime.into(),
            data: bytes,
            preview,
        });
        true
    }

    pub fn remove_image(&mut self, key: &str) {
        if let Some(index) = self
            .pending_images
            .iter()
            .position(|image| image.key == key)
        {
            let removed = self.pending_images.remove(index);
            let _ = std::fs::remove_file(&removed.preview);
        }
    }

    /// Drop every staged image and its preview files.
    pub fn clear_images(&mut self) {
        for image in self.pending_images.drain(..) {
            let _ = std::fs::remove_file(&image.preview);
        }
    }

    /// Send an extension-dialog response and close the dialog. A no-op when
    /// a different dialog (or none) is pending, e.g. after pi's own timeout.
    fn answer_dialog(&mut self, id: &str, response: Command, cx: &mut Context<Self>) {
        let matches = self
            .pending_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.id == id);
        if !matches {
            return;
        }
        if let Some(client) = self.client()
            && let Err(error) = client.send_uncorrelated(response)
        {
            log::warn!("state: failed to answer extension dialog: {error}");
        }
        self.pending_dialog = None;
        cx.notify();
    }

    /// The user picked `value` in a select, or submitted input/editor text.
    pub fn answer_dialog_value(&mut self, id: &str, value: String, cx: &mut Context<Self>) {
        self.answer_dialog(
            id,
            Command::ExtensionUiResponse {
                id: id.to_string(),
                value: Some(value),
                confirmed: None,
                cancelled: None,
            },
            cx,
        );
    }

    /// The user answered a confirm dialog.
    pub fn answer_dialog_confirm(&mut self, id: &str, confirmed: bool, cx: &mut Context<Self>) {
        self.answer_dialog(
            id,
            Command::ExtensionUiResponse {
                id: id.to_string(),
                value: None,
                confirmed: Some(confirmed),
                cancelled: None,
            },
            cx,
        );
    }

    /// The user dismissed the dialog, or its timeout elapsed.
    pub fn cancel_dialog(&mut self, id: &str, cx: &mut Context<Self>) {
        self.answer_dialog(
            id,
            Command::ExtensionUiResponse {
                id: id.to_string(),
                value: None,
                confirmed: None,
                cancelled: Some(true),
            },
            cx,
        );
    }

    /// Mirror pi's dialog timeout client-side.
    pub fn expire_dialog(&mut self, id: &str, cx: &mut Context<Self>) {
        let still_pending = self
            .pending_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.id == id);
        if still_pending {
            log::info!("state: extension dialog {id} timed out on our side");
            self.cancel_dialog(id, cx);
        }
    }

    /// Fetch the `/` command list pi offers (extensions, templates, skills).
    fn load_commands(&self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let response = client.request(Command::GetCommands)?;
                    if !response.success {
                        anyhow::bail!("get_commands failed: {:?}", response.error);
                    }
                    response.data_as::<pi_rpc::protocol::AvailableCommands>()
                })
                .await;
            this.update(cx, |s, cx| {
                if let Ok(Some(payload)) = result {
                    log::info!("state: {} commands available", payload.commands.len());
                    s.commands = payload.commands;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Walk the project for the `@` mention list. A convenience index, not a
    /// git-aware one: it skips the heavy directories most projects have.
    fn scan_workspace_files(&self, cx: &Context<Self>) {
        let Some(root) = self.cwd.clone() else { return };
        cx.spawn(async move |this, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { workspace_files(&root) })
                .await;
            this.update(cx, |s, cx| {
                log::info!("state: {} files indexed for @", files.len());
                s.workspace_files = files;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_history(&self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let entries = client.request(Command::GetEntries { since: None })?;
                    if !entries.success {
                        anyhow::bail!("get_entries failed: {:?}", entries.error);
                    }
                    entries.data_as::<pi_rpc::protocol::SessionEntries>()
                })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(Some(payload)) => {
                        log::info!(
                            "state: history loaded, {} entries (leaf {:?})",
                            payload.entries.len(),
                            payload.leaf_id
                        );
                        s.load_entries(&payload.entries, payload.leaf_id.as_deref());
                    }
                    Ok(None) => {}
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fork the open session from the user prompt that started the turn the
    /// pointer is on: pi branches there and rebinds, so the transcript is
    /// rebuilt from the new leaf afterwards.
    pub fn fork_turn(&mut self, user_text: &str, occurrence: usize, cx: &mut Context<Self>) {
        if self.conn != Conn::Live {
            return;
        }
        if self.streaming {
            self.push_notice(NoticeKind::Warn, i18n::t(cx, "agent_busy").to_string());
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        let user_text = user_text.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let user_text = user_text.clone();
                    async move {
                        let listing = client.request(Command::GetForkMessages)?;
                        if !listing.success {
                            anyhow::bail!("get_fork_messages failed: {:?}", listing.error);
                        }
                        let messages = listing
                            .data_as::<pi_rpc::protocol::ForkMessages>()?
                            .unwrap_or_default()
                            .messages;
                        let entry_id = messages
                            .iter()
                            .filter(|message| message.text == user_text)
                            .nth(occurrence)
                            .map(|message| message.entry_id.clone())
                            .ok_or_else(|| anyhow::anyhow!("fork point not found"))?;
                        let forked = client.request(Command::Fork { entry_id })?;
                        if !forked.success {
                            anyhow::bail!("fork failed: {:?}", forked.error);
                        }
                        Ok(session_action_cancelled(&forked))
                    }
                })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(cancelled) => {
                        if cancelled {
                            s.push_notice(
                                NoticeKind::Warn,
                                i18n::t(cx, "session_cancelled").to_string(),
                            );
                        } else {
                            // Pi rebound to the fork; rebuild around it.
                            s.transcript.clear();
                            s.expanded_turns.clear();
                            s.turn_running = false;
                            s.clear_changes();
                            s.session_name = None;
                            s.stats = None;
                            s.session_epoch += 1;
                            s.pending_steers.clear();
                            s.query_state(cx);
                            s.load_history(cx);
                            s.refresh_stats(cx);
                            s.refresh_sessions(cx);
                        }
                    }
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Duplicate a session's active branch into a new session. Forking a
    /// session that is not open switches to it for the clone and then back,
    /// so the user's place survives; forking the open one leaves the clone
    /// active, as the fork is the continuation.
    pub fn fork_session(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.conn != Conn::Live {
            return;
        }
        if self.streaming {
            self.push_notice(NoticeKind::Warn, i18n::t(cx, "agent_busy").to_string());
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        let target = path.to_string();
        let original = self.session_file.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let target = target.clone();
                    let original = original.clone();
                    async move {
                        let same = |a: &str, b: &str| session_key(a) == session_key(b);
                        let forking_elsewhere =
                            original.as_deref().is_none_or(|open| !same(open, &target));
                        if forking_elsewhere {
                            let switched = client.request(Command::SwitchSession {
                                session_path: target.clone(),
                            })?;
                            if !switched.success {
                                anyhow::bail!("switch_session failed: {:?}", switched.error);
                            }
                            if session_action_cancelled(&switched) {
                                return Ok(None);
                            }
                        }
                        let cloned = client.request(Command::Clone)?;
                        if !cloned.success {
                            anyhow::bail!("clone failed: {:?}", cloned.error);
                        }
                        if session_action_cancelled(&cloned) {
                            return Ok(None);
                        }
                        let fork_cwd = sessions::read_session_cwd(std::path::Path::new(&target));
                        let mut restored = !forking_elsewhere;
                        if forking_elsewhere && let Some(back) = original.as_deref() {
                            let switched = client.request(Command::SwitchSession {
                                session_path: back.to_string(),
                            })?;
                            restored = switched.success && !session_action_cancelled(&switched);
                        }
                        // True when the open session is the one we started on.
                        Ok(Some((restored, fork_cwd)))
                    }
                })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(Some((active_is_original, fork_cwd))) => {
                        if active_is_original {
                            // Back where the user was: only the list grew.
                            s.query_state(cx);
                        } else {
                            // The clone is active now; rebuild around it.
                            s.transcript.clear();
                            s.expanded_turns.clear();
                            s.turn_running = false;
                            s.clear_changes();
                            s.session_name = None;
                            s.stats = None;
                            s.session_epoch += 1;
                            s.pending_steers.clear();
                            s.apply_session_cwd(fork_cwd, cx);
                            s.query_state(cx);
                            s.load_history(cx);
                            s.refresh_stats(cx);
                        }
                        s.refresh_sessions(cx);
                    }
                    Ok(None) => s.push_notice(
                        NoticeKind::Warn,
                        i18n::t(cx, "session_cancelled").to_string(),
                    ),
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop the session and return to the welcome screen.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
        self.conn = Conn::Idle;
        self.transcript.clear();
        self.expanded_turns.clear();
        self.turn_running = false;
        self.clear_changes();
        self.streaming = false;
        self.stats = None;
        self.session_name = None;
        self.draft_session_file = None;
        self.awaiting_new_session_file = false;
        self.sidebar_rows = self.sidebar_rows_now();
        self.pending_steers.clear();
        self.queued_steers.clear();
        self.queued_follow_ups.clear();
        self.pending_composer_text = None;
        cx.notify();
    }

    /// Reconnect to the last project/session after a crash or failed start.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        let directory = self
            .cwd
            .clone()
            .or_else(|| self.last_project.clone().map(PathBuf::from));
        if let Some(directory) = directory {
            self.connect(directory, self.session_file.clone(), Vec::new(), cx);
        }
    }

    /// Consume the client's broadcast into the transcript until it ends.
    fn start_pump(&self, client: PiClient, cx: &Context<Self>) {
        let events = Arc::new(client.subscribe());
        cx.spawn(async move |this, cx| {
            loop {
                let rx = Arc::clone(&events);
                let next = cx
                    .background_executor()
                    .spawn(async move { rx.recv() })
                    .await;
                match next {
                    Ok(event) => {
                        let keep_going = this.update(cx, |s, cx| {
                            s.on_client_event(event, cx);
                            cx.notify();
                        });
                        if keep_going.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    // -- sidebar ------------------------------------------------------------

    /// Rescan `~/.pi/agent/sessions` in the background, reusing cached
    /// summaries for files that have not changed.
    pub fn refresh_sessions(&self, cx: &Context<Self>) {
        let scanner = Arc::clone(&self.session_scan);
        cx.spawn(async move |this, cx| {
            let groups = cx
                .background_executor()
                .spawn(async move {
                    let Some(dir) = sessions::default_session_dir() else {
                        return Vec::new();
                    };
                    match scanner.lock() {
                        Ok(mut scanner) => scanner.scan(&dir).unwrap_or_else(|error| {
                            log::warn!("session scan failed: {error}");
                            Vec::new()
                        }),
                        Err(error) => {
                            log::warn!("session scan cache poisoned: {error}");
                            Vec::new()
                        }
                    }
                })
                .await;
            this.update(cx, |s, cx| {
                s.session_groups = merge_user_projects(groups, &s.user_projects);
                // The file exists now, so the scan took the draft row over.
                let draft_listed = s.draft_session_file.as_deref().is_some_and(|draft| {
                    let key = session_key(draft);
                    s.session_groups.iter().any(|group| {
                        group
                            .sessions
                            .iter()
                            .any(|session| session_key(&session.path.to_string_lossy()) == key)
                    })
                });
                if draft_listed {
                    s.draft_session_file = None;
                }
                s.sidebar_rows = s.sidebar_rows_now();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -- actions ------------------------------------------------------------

    /// Send a prompt, or steer the running agent. The optimistic user bubble
    /// appears immediately; on rejection it is removed and its text restored
    /// to the composer.
    pub fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        if (text.trim().is_empty() && self.pending_images.is_empty()) || self.conn != Conn::Live {
            return;
        }
        let Some(client) = self.client() else { return };
        // Every staged image rides along; pi wants base64 in the JSON.
        let images = (!self.pending_images.is_empty()).then(|| {
            self.pending_images
                .iter()
                .map(|image| pi_rpc::protocol::ImageInput {
                    content_type: "image",
                    data: base64::engine::general_purpose::STANDARD.encode(&image.data),
                    mime_type: image.mime.clone(),
                })
                .collect::<Vec<_>>()
        });
        let steering = self.streaming;
        let command = if steering {
            // Extension/prompt-template commands are not allowed through
            // `steer`; pi expands them on a prompt with streamingBehavior.
            if text.trim_start().starts_with('/') {
                Command::Prompt {
                    message: text.clone(),
                    images,
                    streaming_behavior: Some(StreamingBehavior::Steer),
                }
            } else {
                Command::Steer {
                    message: text.clone(),
                    images,
                }
            }
        } else {
            Command::Prompt {
                message: text.clone(),
                images,
                streaming_behavior: None,
            }
        };
        // The staged images leave with the message, whatever its fate.
        self.clear_images();
        if steering {
            self.pending_steers.push(text.clone());
        } else {
            // A fresh turn: its status line sits between this prompt and the
            // content, clocking the work until the agent settles.
            self.transcript.push(TranscriptItem::Turn {
                started: Instant::now(),
                took: None,
            });
            self.turn_running = true;
            self.spawn_turn_ticker(cx);
        }
        self.transcript.push(TranscriptItem::User {
            text: text.clone().into(),
            time: Some(now_secs()),
        });
        cx.notify();

        let label = if steering { "steer" } else { "prompt" };
        let restore = text.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(command) })
                .await;
            this.update(cx, |s, cx| {
                let failure = match &result {
                    Ok(response) if response.success => None,
                    Ok(response) => Some(response.error.clone().unwrap_or_default()),
                    Err(error) => Some(error.to_string()),
                };
                if let Some(error) = failure {
                    s.remove_last_user_text(&text);
                    if steering {
                        s.pending_steers.retain(|pending| pending != &text);
                    }
                    s.restore_to_composer(&restore);
                    s.push_notice(NoticeKind::Error, format!("{label}: {error}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_last_user_text(&mut self, text: &str) {
        if let Some(index) = self
            .transcript
            .iter()
            .rposition(|item| matches!(item, TranscriptItem::User { text: t, .. } if t == text))
        {
            self.transcript.remove(index);
        }
    }

    fn restore_to_composer(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.pending_composer_text = Some(match self.pending_composer_text.take() {
            Some(existing) if !existing.is_empty() => format!("{existing}\n{text}"),
            _ => text.to_string(),
        });
    }

    /// Interactive Esc semantics: drop queued steering, then abort the run.
    ///
    /// pi returns the cleared messages' text, and the documented Esc behavior
    /// is to put it back in the editor, so the user does not lose what they
    /// typed while the agent was running.
    pub fn stop(&mut self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let cleared = cx
                .background_executor()
                .spawn(async move {
                    let cleared = client.request(Command::ClearQueue);
                    let _ = client.request(Command::Abort);
                    cleared
                })
                .await;
            this.update(cx, |s, cx| {
                match cleared {
                    Ok(response) if response.success => {
                        if let Ok(Some(queue)) = response.data_as::<ClearedQueue>() {
                            for text in queue.steering.iter().chain(queue.follow_up.iter()) {
                                s.remove_last_user_text(text);
                            }
                            let restored = queue
                                .steering
                                .iter()
                                .chain(queue.follow_up.iter())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("\n");
                            s.restore_to_composer(&restored);
                        }
                        s.pending_steers.clear();
                    }
                    Ok(response) => s.push_notice(
                        NoticeKind::Error,
                        format!("clear_queue: {}", response.error.unwrap_or_default()),
                    ),
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Start a fresh session in the same project. The transcript is only
    /// cleared after pi accepts the switch, so a cancelled or failed command
    /// leaves the current conversation intact.
    pub fn new_chat(&mut self, cx: &mut Context<Self>) {
        if self.conn != Conn::Live {
            return;
        }
        if self.streaming {
            self.push_notice(NoticeKind::Warn, i18n::t(cx, "agent_busy").to_string());
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    client.request(Command::NewSession {
                        parent_session: None,
                    })
                })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(response) if response.success => {
                        if session_action_cancelled(&response) {
                            s.push_notice(
                                NoticeKind::Warn,
                                i18n::t(cx, "session_cancelled").to_string(),
                            );
                        } else {
                            s.transcript.clear();
                            s.expanded_turns.clear();
                            s.turn_running = false;
                            s.clear_changes();
                            s.session_name = None;
                            s.stats = None;
                            s.session_epoch += 1;
                            s.pending_steers.clear();
                            // The new session starts unsaved: pi assigns its
                            // file path now and writes the file on the first
                            // reply. Follow the path so the sidebar can show
                            // the row before that happens.
                            s.draft_session_file = None;
                            s.awaiting_new_session_file = true;
                            if let Some(cwd) =
                                s.cwd.as_ref().map(|cwd| cwd.to_string_lossy().into_owned())
                            {
                                s.collapsed_projects.remove(&cwd);
                            }
                            s.sidebar_rows = s.sidebar_rows_now();
                            s.query_state(cx);
                            s.refresh_stats(cx);
                            s.refresh_sessions(cx);
                        }
                    }
                    Ok(response) => s.push_notice(
                        NoticeKind::Error,
                        format!("new_session: {}", response.error.unwrap_or_default()),
                    ),
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load a past session into the running pi process and rebuild the
    /// transcript from its entries. The transcript is cleared only once pi
    /// accepts the switch.
    pub fn switch_session(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.conn != Conn::Live {
            return;
        }
        if self.streaming {
            self.push_notice(NoticeKind::Warn, i18n::t(cx, "agent_busy").to_string());
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        let session_file = path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let switched = client.request(Command::SwitchSession {
                            session_path: path.to_string_lossy().into_owned(),
                        })?;
                        if !switched.success {
                            anyhow::bail!("switch_session failed: {:?}", switched.error);
                        }
                        let cancelled = session_action_cancelled(&switched);
                        // pi rebinds its own runtime to the session's cwd;
                        // pido must follow so `@` and last_project agree.
                        let cwd = sessions::read_session_cwd(&path);
                        Ok((cancelled, cwd))
                    }
                })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok((cancelled, cwd)) => {
                        if cancelled {
                            s.push_notice(
                                NoticeKind::Warn,
                                i18n::t(cx, "session_cancelled").to_string(),
                            );
                        } else {
                            s.transcript.clear();
                            s.expanded_turns.clear();
                            s.turn_running = false;
                            s.clear_changes();
                            s.session_name = None;
                            s.stats = None;
                            s.session_epoch += 1;
                            s.pending_steers.clear();
                            // Any draft belonged to the session being left.
                            s.draft_session_file = None;
                            s.awaiting_new_session_file = false;
                            s.remember_session_file(Some(&session_file));
                            s.apply_session_cwd(cwd, cx);
                            s.sidebar_rows = s.sidebar_rows_now();
                            // Refresh name/model/session-file, then rebuild
                            // the transcript from the switched session.
                            s.query_state(cx);
                            s.load_history(cx);
                            s.refresh_stats(cx);
                        }
                    }
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Move pido's project context (cwd, `@` index, slash commands,
    /// last_project) to follow a switched session.
    fn apply_session_cwd(&mut self, cwd: Option<String>, cx: &Context<Self>) {
        let Some(cwd) = cwd else { return };
        let path = PathBuf::from(&cwd);
        if self.cwd.as_deref() == Some(path.as_path()) {
            return;
        }
        self.cwd = Some(path);
        self.last_project = Some(cwd.clone());
        crate::settings::save_setting(|s| s.last_project = Some(cwd));
        self.scan_workspace_files(cx);
        self.load_commands(cx);
    }

    pub fn set_model(&mut self, provider: &str, model_id: &str, cx: &mut Context<Self>) {
        let Some(model) = self
            .models
            .iter()
            .find(|m| m.id == model_id && m.provider.as_deref().unwrap_or_default() == provider)
            .cloned()
        else {
            return;
        };
        let Some(client) = self.client() else { return };
        let previous_model = self.current_model.clone();
        let previous_levels = self.thinking_levels.clone();
        self.current_model = Some(model);
        let provider = provider.to_string();
        let model_id = model_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(Command::SetModel { provider, model_id }) })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(response) if response.success => {
                        // Thinking levels change with the model.
                        s.refresh_thinking_levels(cx);
                    }
                    Ok(response) => {
                        s.current_model = previous_model;
                        s.thinking_levels = previous_levels;
                        s.push_notice(
                            NoticeKind::Error,
                            format!("set_model: {}", response.error.unwrap_or_default()),
                        );
                    }
                    Err(error) => {
                        s.current_model = previous_model;
                        s.thinking_levels = previous_levels;
                        s.push_notice(NoticeKind::Error, error.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn set_thinking(&mut self, level: ThinkingLevel, cx: &mut Context<Self>) {
        if level == ThinkingLevel::Unknown {
            return;
        }
        let Some(client) = self.client() else { return };
        let previous = self.thinking_level;
        self.thinking_level = Some(level);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(Command::SetThinkingLevel { level }) })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(response) if response.success => {}
                    Ok(response) => {
                        s.thinking_level = previous;
                        s.push_notice(
                            NoticeKind::Error,
                            format!("set_thinking: {}", response.error.unwrap_or_default()),
                        );
                    }
                    Err(error) => {
                        s.thinking_level = previous;
                        s.push_notice(NoticeKind::Error, error.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn compact_now(&mut self, cx: &Context<Self>) {
        self.check_command(
            Command::Compact {
                custom_instructions: None,
            },
            "compact",
            cx,
        );
    }

    /// Run a command in the background and surface failures as notices.
    fn check_command(&self, command: Command, label: &str, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        let label = label.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(command) })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(response) if response.success => {}
                    Ok(response) => {
                        s.push_notice(
                            NoticeKind::Error,
                            format!("{label}: {}", response.error.unwrap_or_default()),
                        );
                    }
                    Err(error) => s.push_notice(NoticeKind::Error, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Refresh model/thinking/name from a `get_state` round trip.
    fn query_state(&self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(Command::GetState) })
                .await;
            this.update(cx, |s, cx| {
                if let Ok(Ok(Some(payload))) = result.map(|r| r.data_as::<SessionState>()) {
                    s.current_model = payload.model;
                    s.thinking_level = payload.thinking_level;
                    s.session_name = payload.session_name;
                    s.session_id = payload.session_id;
                    let file = payload.session_file;
                    s.remember_session_file(file.as_deref());
                    // `new_chat` waits for this round trip to learn the file
                    // path pi allocated for the fresh session.
                    if s.awaiting_new_session_file {
                        s.awaiting_new_session_file = false;
                        s.draft_session_file = file;
                    }
                    s.sidebar_rows = s.sidebar_rows_now();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Refresh thinking levels for the current model.
    fn refresh_thinking_levels(&self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(Command::GetAvailableThinkingLevels) })
                .await;
            this.update(cx, |s, cx| {
                if let Ok(Ok(Some(payload))) =
                    result.map(|r| r.data_as::<pi_rpc::protocol::AvailableThinkingLevels>())
                {
                    s.thinking_levels = payload.levels;
                    s.thinking_levels
                        .retain(|level| *level != ThinkingLevel::Unknown);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Refresh session stats after a run settles.
    fn refresh_stats(&self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.request(Command::GetSessionStats) })
                .await;
            this.update(cx, |s, cx| {
                if let Ok(Ok(Some(payload))) =
                    result.map(|r| r.data_as::<pi_rpc::protocol::SessionStats>())
                {
                    s.stats = Some(payload);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        cx.notify();
    }

    pub fn push_notice(&mut self, kind: NoticeKind, text: String) {
        log::info!("notice: {text}");
        self.notice = Some((kind, text));
    }

    // -- event reduction ----------------------------------------------------

    /// Fold one client event into the state. Runs on the main thread.
    fn on_client_event(&mut self, event: ClientEvent, cx: &mut Context<Self>) {
        match event {
            ClientEvent::Record(record) => self.on_record(record, cx),
            ClientEvent::Disconnected(reason) => {
                log::warn!(
                    "state: client disconnected ({reason:?}), conn was {:?}",
                    self.conn
                );
                if self.conn == Conn::Live {
                    let mut message = format!("{reason:?}");
                    if let Some(tail) = self.client.as_ref().map(PiClient::last_stderr)
                        && !tail.is_empty()
                    {
                        message.push('\n');
                        message.push_str(&tail);
                    }
                    self.conn = Conn::Dead(message);
                    self.streaming = false;
                }
                // No AgentSettled will ever arrive: freeze any half-streamed
                // view so its timer and live indicators stop.
                self.finalize_open_views();
            }
        }
    }

    /// Mark every unfinished assistant view as finished and freeze its
    /// thinking duration. Called on settle and on disconnect.
    fn finalize_open_views(&mut self) {
        for item in &mut self.transcript {
            if let TranscriptItem::Assistant(view) = item
                && !view.finished
            {
                view.finished = true;
                view.live_text = None;
                view.live_thinking = None;
                if view.took.is_none() {
                    view.took = Some(view.started.elapsed());
                }
            }
        }
    }

    fn on_record(&mut self, record: RpcRecord, cx: &mut Context<Self>) {
        match record {
            RpcRecord::Response(response) => self.on_unmatched_response(response),
            RpcRecord::ExtensionUiRequest(request) => {
                if request.method.is_dialog() {
                    // The extension blocks until we answer (or its own
                    // timeout fires). Show the native dialog; a client-side
                    // timer mirrors pi's timeout so a stale dialog cannot
                    // sit forever after pi has already moved on.
                    log::info!(
                        "state: extension dialog {} ({:?})",
                        request.id,
                        request.method
                    );
                    if let Some(timeout) = request.timeout {
                        let id = request.id.clone();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(timeout))
                                .await;
                            this.update(cx, |s, cx| s.expire_dialog(&id, cx)).ok();
                        })
                        .detach();
                    }
                    self.pending_dialog = Some(request.clone());
                    cx.notify();
                } else if request.method == pi_rpc::protocol::ExtensionUiMethod::SetEditorText {
                    // The extension wants its text in the composer; the root
                    // view drains this channel into the draft field.
                    if let Some(text) = request.text {
                        self.pending_composer_text = Some(text);
                    }
                    cx.notify();
                } else {
                    let text = request
                        .message
                        .or(request.title)
                        .unwrap_or_else(|| i18n::t(cx, "extension_notify").to_string());
                    self.push_notice(NoticeKind::Warn, text);
                }
            }
            RpcRecord::Event(event) => self.on_event(event, cx),
            RpcRecord::Unknown { record_type, raw } => {
                log::debug!("unknown record {record_type}: {raw}");
            }
        }
    }

    /// Responses nobody awaited, such as `parse` errors without an id.
    fn on_unmatched_response(&mut self, response: Response) {
        if !response.success {
            let text = format!(
                "{}: {}",
                response.command,
                response.error.unwrap_or_default()
            );
            self.push_notice(NoticeKind::Error, text);
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::AgentStart => {
                self.streaming = true;
            }
            Event::AgentSettled { aborted } => {
                self.streaming = false;
                self.finish_running_turn();
                self.pending_steers.clear();
                self.queued_steers.clear();
                self.queued_follow_ups.clear();
                // Any assistant view still live is finished now.
                self.finalize_open_views();
                if aborted {
                    self.push_notice(NoticeKind::Warn, i18n::t(cx, "aborted").to_string());
                }
                // The first message creates the session file; refresh so it
                // becomes the persisted resume target.
                self.query_state(cx);
                self.refresh_stats(cx);
                self.refresh_sessions(cx);
            }
            Event::MessageStart { message } => {
                // pi also emits message_start for user prompts, steering
                // messages, tool results and custom messages. Only an
                // assistant message may open a transcript view.
                if opens_assistant_view(&message) {
                    self.transcript.push(TranscriptItem::Assistant(Box::new(
                        AssistantView::streaming(),
                    )));
                }
            }
            Event::MessageUpdate {
                assistant_message_event,
                ..
            } => self.apply_delta(assistant_message_event),
            Event::MessageEnd { message } => {
                if let AgentMessage::Assistant(assistant) = &message {
                    let view = AssistantView::from_message(assistant);
                    // Replace the streaming view this message grew in, or
                    // append when none is open (e.g. restored history).
                    match self
                        .transcript
                        .iter_mut()
                        .rev()
                        .find(|item| matches!(item, TranscriptItem::Assistant(v) if !v.finished))
                    {
                        Some(TranscriptItem::Assistant(existing)) => {
                            // Keep a duration already frozen at ThinkingEnd;
                            // otherwise measure from the view's start.
                            let took = existing.took.or_else(|| Some(existing.started.elapsed()));
                            **existing = view;
                            existing.took = took;
                        }
                        _ => self
                            .transcript
                            .push(TranscriptItem::Assistant(Box::new(view))),
                    }
                    // Tool calls embedded in the finished message become
                    // cards if execution events have not made them already.
                    for block in &assistant.content {
                        if let pi_rpc::protocol::AssistantContent::ToolCall(call) = block {
                            self.ensure_tool_card(
                                call.id.clone(),
                                call.name.clone(),
                                Some(&call.arguments),
                            );
                        }
                    }
                }
            }
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let card =
                    self.ensure_tool_card(tool_call_id.clone(), tool_name.clone(), args.as_ref());
                card.running = true;
                card.started = Instant::now();
                // A write's content is only in its arguments; by the time its
                // result arrives the old file is gone, so keep them here.
                if tool_name == "write"
                    && let Some((path, content)) = write_args(args.as_ref())
                {
                    self.pending_writes
                        .insert(tool_call_id.clone(), PendingWrite { path, content });
                }
            }
            Event::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => {
                if let Some(card) = self.tool_card_mut(&tool_call_id)
                    && let Some(partial) = partial_result
                {
                    // Updates carry accumulated output; replace.
                    card.result = Some(truncate_for_display(&partial.text(), 2000).into());
                }
            }
            Event::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                is_error,
                result,
            } => {
                if let Some(card) = self.tool_card_mut(&tool_call_id) {
                    card.running = false;
                    card.failed = is_error;
                    card.took = Some(card.started.elapsed());
                    if let Some(result) = result.as_ref() {
                        card.result = Some(truncate_for_display(&result.text(), 2000).into());
                    }
                }
                if is_error {
                    self.pending_writes.remove(&tool_call_id);
                } else if tool_name == "edit" {
                    let patch = result
                        .as_ref()
                        .and_then(|result| result.details.as_ref())
                        .and_then(|details| details.get("patch"))
                        .and_then(|patch| patch.as_str());
                    match patch {
                        Some(patch) => self.record_patch(patch),
                        None => log::debug!("edit result without a patch"),
                    }
                } else if tool_name == "write"
                    && let Some(write) = self.pending_writes.remove(&tool_call_id)
                {
                    self.record_write(&write.path, &write.content);
                }
            }
            Event::CompactionStart { .. } => {}
            Event::CompactionEnd {
                aborted,
                result,
                error_message,
                ..
            } => {
                if aborted {
                    self.push_notice(
                        NoticeKind::Warn,
                        i18n::t(cx, "context_compacted_aborted").to_string(),
                    );
                } else if result.is_some() {
                    self.push_notice(
                        NoticeKind::Info,
                        i18n::t(cx, "context_compacted").to_string(),
                    );
                } else {
                    let detail = error_message
                        .unwrap_or_else(|| i18n::t(cx, "context_compacted_failed").to_string());
                    self.push_notice(NoticeKind::Error, detail);
                }
            }
            Event::AutoRetryStart { .. } => {
                self.push_notice(NoticeKind::Warn, i18n::t(cx, "retrying").to_string());
            }
            Event::AutoRetryEnd {
                success,
                final_error,
                ..
            } => {
                if success {
                    self.push_notice(NoticeKind::Info, i18n::t(cx, "retry_succeeded").to_string());
                } else {
                    let detail = final_error
                        .unwrap_or_else(|| i18n::t(cx, "retry_failed_unknown").to_string());
                    self.push_notice(
                        NoticeKind::Error,
                        i18n::t_arg(cx, "retry_failed", "e", &detail).to_string(),
                    );
                }
            }
            Event::ExtensionError { error, .. } => {
                if let Some(error) = error {
                    self.push_notice(NoticeKind::Error, error);
                }
            }
            Event::QueueUpdate {
                steering,
                follow_up,
            } => {
                self.pending_steers = steering.clone();
                self.queued_steers = steering.into_iter().map(Into::into).collect();
                self.queued_follow_ups = follow_up.into_iter().map(Into::into).collect();
            }
            Event::BashExecutionUpdate { .. } => {}
            other => {
                log::debug!("event not shown in MVP UI: {:?}", other);
            }
        }
    }

    /// Append one streaming delta to the open assistant view.
    fn apply_delta(&mut self, delta: AssistantMessageEvent) {
        let Some(TranscriptItem::Assistant(view)) = self
            .transcript
            .iter_mut()
            .rev()
            .find(|item| matches!(item, TranscriptItem::Assistant(v) if !v.finished))
        else {
            // Delta without an open view (should not happen in RPC mode).
            log::debug!("dropping delta with no open assistant view: {delta:?}");
            return;
        };
        match delta {
            AssistantMessageEvent::TextStart { content_index } => {
                view.live_text = Some(content_index);
                view.text_blocks.entry(content_index).or_default();
            }
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                view.live_text = Some(content_index);
                view.text_blocks
                    .entry(content_index)
                    .or_default()
                    .push_str(&delta);
            }
            AssistantMessageEvent::TextEnd {
                content_index,
                content,
            } => {
                view.text_blocks.insert(content_index, content);
                if view.live_text == Some(content_index) {
                    view.live_text = None;
                }
            }
            AssistantMessageEvent::ThinkingStart { content_index } => {
                view.live_thinking = Some(content_index);
                view.thinking_blocks.entry(content_index).or_default();
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                view.live_thinking = Some(content_index);
                view.thinking_blocks
                    .entry(content_index)
                    .or_default()
                    .push_str(&delta);
            }
            AssistantMessageEvent::ThinkingEnd { content_index } => {
                if view.live_thinking == Some(content_index) {
                    view.live_thinking = None;
                }
                // Freeze the thinking duration here: the header switches from
                // the live indicator to a static value at exactly this point.
                if view.took.is_none() {
                    view.took = Some(view.started.elapsed());
                }
            }
            AssistantMessageEvent::ToolCallStart {
                id,
                tool_name,
                content_index,
                ..
            } => {
                // Reserve the slot so later deltas for other content indexes
                // still land in this view; the card appears on execution.
                log::debug!(
                    "tool call {id} ({tool_name}) announced at {content_index}; awaiting execution"
                );
            }
            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                log::debug!(
                    "tool call args delta at {content_index}: {} bytes",
                    delta.len()
                );
            }
            AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                self.ensure_tool_card(tool_call.id, tool_call.name, Some(&tool_call.arguments));
            }
        }
    }

    /// Find a tool card by call id, or create one just before the first
    /// assistant view that follows it.
    fn ensure_tool_card(
        &mut self,
        call_id: String,
        name: String,
        args: Option<&serde_json::Value>,
    ) -> &mut ToolView {
        if let Some(index) = self
            .transcript
            .iter()
            .position(|item| matches!(item, TranscriptItem::Tool(card) if card.call_id == call_id))
        {
            let card = match &mut self.transcript[index] {
                TranscriptItem::Tool(card) => card,
                _ => unreachable!("position() matched a Tool"),
            };
            if card.args.is_none() {
                card.args = args.map(|value| pretty_json(value).into());
            }
            return card;
        }
        // Insert after the last transcript item that is not a notice.
        let insert_at = self
            .transcript
            .iter()
            .rposition(|item| !matches!(item, TranscriptItem::Notice { .. }))
            .map_or(0, |i| i + 1);
        self.transcript.insert(
            insert_at,
            TranscriptItem::Tool(ToolView {
                call_id: call_id.clone(),
                name,
                args: args.map(|value| pretty_json(value).into()),
                running: false,
                failed: false,
                result: None,
                started: Instant::now(),
                took: None,
            }),
        );
        match self
            .transcript
            .iter_mut()
            .find(|item| matches!(item, TranscriptItem::Tool(card) if card.call_id == call_id))
        {
            Some(TranscriptItem::Tool(card)) => card,
            _ => unreachable!("just inserted the card"),
        }
    }

    fn tool_card_mut(&mut self, call_id: &str) -> Option<&mut ToolView> {
        self.transcript
            .iter_mut()
            .rev()
            .find_map(|item| match item {
                TranscriptItem::Tool(card) if card.call_id == call_id => Some(card),
                _ => None,
            })
    }

    /// Rebuild the transcript from `get_entries` payload.
    fn load_entries(&mut self, entries: &[SessionEntry], leaf_id: Option<&str>) {
        self.transcript.clear();
        self.expanded_turns.clear();
        self.turn_running = false;
        self.clear_changes();

        // `write` calls carry their content in the call itself; the result
        // that follows only confirms it.
        let mut calls: HashMap<String, pi_rpc::protocol::ToolCall> = HashMap::new();
        for entry in active_branch(entries, leaf_id) {
            let Some(message) = &entry.message else {
                if entry.entry_type == "compaction" {
                    // The compacted summary is a checkpoint, not chat.
                }
                continue;
            };
            match message {
                AgentMessage::User(user) => {
                    let text = user.text();
                    if !text.is_empty() {
                        self.transcript.push(TranscriptItem::User {
                            text: text.into(),
                            time: unix_secs(user.timestamp),
                        });
                    }
                }
                AgentMessage::Assistant(assistant) => {
                    let mut view = AssistantView::from_message(assistant);
                    view.took = None;
                    self.transcript
                        .push(TranscriptItem::Assistant(Box::new(view)));
                    // Cards follow the message in both live and restored
                    // transcripts, so ordering stays consistent.
                    for block in &assistant.content {
                        if let pi_rpc::protocol::AssistantContent::ToolCall(call) = block {
                            self.ensure_tool_card(
                                call.id.clone(),
                                call.name.clone(),
                                Some(&call.arguments),
                            );
                            calls.insert(call.id.clone(), call.clone());
                        }
                    }
                }
                AgentMessage::ToolResult(result) => {
                    let text = result.text();
                    if let Some(card) = self.tool_card_mut(&result.tool_call_id) {
                        card.running = false;
                        card.failed = result.is_error;
                        card.result = Some(truncate_for_display(&text, 2000).into());
                    } else {
                        self.transcript.push(TranscriptItem::Tool(ToolView {
                            call_id: result.tool_call_id.clone(),
                            name: result.tool_name.clone(),
                            args: None,
                            running: false,
                            failed: result.is_error,
                            result: Some(truncate_for_display(&text, 2000).into()),
                            started: Instant::now(),
                            took: None,
                        }));
                    }
                    if !result.is_error {
                        self.record_history_change(result, &calls);
                    }
                }
                AgentMessage::BashExecution(bash) => {
                    self.transcript.push(TranscriptItem::Notice {
                        kind: NoticeKind::Info,
                        text: format!("$ {}", bash.command),
                    });
                }
                // The system-prompt snapshot and unknown roles are internal
                // bookkeeping: restored, but not part of the conversation.
                AgentMessage::System(_) | AgentMessage::Unknown => {}
            }
        }
    }
}

/// The fork point of the assistant turn at `ix`: the text of the user
/// prompt that started it, and which occurrence of that text it is, so
/// duplicate prompts land on the right entry.
pub(crate) fn fork_point_for(items: &[TranscriptItem], ix: usize) -> Option<(String, usize)> {
    let row = items.get(ix)?;
    if !matches!(row, TranscriptItem::Assistant(_)) {
        return None;
    }
    let mut at = None;
    for (j, item) in items[..ix].iter().enumerate().rev() {
        if let TranscriptItem::User { text, .. } = item {
            at = Some((j, text.to_string()));
            break;
        }
    }
    let (j, text) = at?;
    let occurrence = items[..j]
        .iter()
        .filter(|item| matches!(item, TranscriptItem::User { text: t, .. } if *t == text))
        .count();
    Some((text, occurrence))
}

/// Fold the user's added projects into the scan: a project without sessions
/// still gets its group, in the order the user added them.
fn merge_user_projects(mut groups: Vec<SessionGroup>, projects: &[String]) -> Vec<SessionGroup> {
    for project in projects {
        if !groups.iter().any(|group| group.cwd == *project) {
            groups.push(SessionGroup {
                cwd: project.clone(),
                sessions: Vec::new(),
            });
        }
    }
    groups
}

/// Whether the row at `ix` sits in a finished, still-folded turn: its tool
/// calls, notices and thinking are hidden until the turn is re-opened.
pub(crate) fn turn_folded_for(
    items: &[TranscriptItem],
    ix: usize,
    expanded: &std::collections::HashSet<u64>,
) -> bool {
    for (j, item) in items[..ix.min(items.len())].iter().enumerate().rev() {
        if let TranscriptItem::Turn { took, .. } = item {
            return took.is_some() && !expanded.contains(&(j as u64));
        }
    }
    false
}

/// Only assistant message starts open a transcript view. Pi emits the same
/// event for user prompts/steering, tool results and custom messages.
fn opens_assistant_view(message: &Option<AgentMessage>) -> bool {
    matches!(message, Some(AgentMessage::Assistant(_)))
}

/// Whether a `new_session` / `switch_session` response was cancelled by an
/// extension's `session_before_switch` handler.
fn session_action_cancelled(response: &Response) -> bool {
    response
        .data
        .as_ref()
        .and_then(|data| data.get("cancelled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Entries on the path from the root to `leaf_id`, in append order. Falls
/// back to every entry when the leaf is missing or the tree is malformed.
fn active_branch<'a>(entries: &'a [SessionEntry], leaf_id: Option<&str>) -> Vec<&'a SessionEntry> {
    let Some(leaf_id) = leaf_id else {
        return entries.iter().collect();
    };
    let by_id: HashMap<&str, &SessionEntry> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = leaf_id;
    while let Some(entry) = by_id.get(current).copied() {
        if !seen.insert(current) {
            log::warn!("session tree has a cycle at {current}");
            break;
        }
        chain.push(entry);
        match entry.parent_id.as_deref() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    if chain.is_empty() {
        log::warn!("leaf {leaf_id} not found in {} entries", entries.len());
        return entries.iter().collect();
    }
    chain.reverse();
    chain
}

/// Providers worth probing for onboarding: those in `auth.json` plus the
/// common env-var-backed ones. Only their names are read; credentials stay
/// with pi.
fn probe_providers() -> Vec<String> {
    let mut providers = vec![
        "deepseek".to_string(),
        "anthropic".to_string(),
        "openai".to_string(),
        "openrouter".to_string(),
        "google".to_string(),
    ];
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".pi").join("agent").join("auth.json");
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(&text)
        {
            providers.extend(map.into_iter().map(|(provider, _)| provider));
        }
    }
    providers.sort();
    providers.dedup();
    providers
}

/// Pretty-print JSON arguments for the tool card.
fn pretty_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

/// Keep display text within a sane bound; a full log can be huge.
fn truncate_for_display(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}\n…")
}

/// Project-relative file paths for `@` mentions.
///
/// A cheap index rather than a git-aware one: skips the directories nearly
/// every project wants ignored, caps depth and count so a huge tree cannot
/// stall startup.
fn workspace_files(root: &std::path::Path) -> Vec<String> {
    const SKIP: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        "out",
        ".venv",
        "venv",
        "__pycache__",
        ".next",
        ".nuxt",
        ".cache",
        ".idea",
        ".vscode",
        "vendor",
        ".gradle",
        ".dart_tool",
        "Pods",
    ];
    const MAX_FILES: usize = 3000;
    const MAX_DEPTH: usize = 6;

    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || files.len() >= MAX_FILES {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push((path, depth + 1));
                }
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().replace(char::from(92), "/"));
                if files.len() >= MAX_FILES {
                    break;
                }
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, parent: Option<&str>) -> SessionEntry {
        SessionEntry {
            entry_type: "message".into(),
            id: id.into(),
            parent_id: parent.map(str::to_string),
            timestamp: None,
            message: None,
        }
    }

    #[test]
    fn only_assistant_message_starts_open_a_view() {
        let user = AgentMessage::User(pi_rpc::protocol::UserMessage {
            content: pi_rpc::protocol::UserContent::Text("hi".into()),
            timestamp: 1.0,
            attachments: None,
        });
        let tool = AgentMessage::ToolResult(pi_rpc::protocol::ToolResultMessage {
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            content: Vec::new(),
            details: None,
            usage: None,
            is_error: false,
            timestamp: 2.0,
        });
        let assistant = AgentMessage::Assistant(pi_rpc::protocol::AssistantMessage {
            content: Vec::new(),
            api: None,
            provider: None,
            model: None,
            usage: None,
            stop_reason: None,
            error_message: None,
            timestamp: 3.0,
        });

        assert!(!opens_assistant_view(&Some(user)));
        assert!(!opens_assistant_view(&Some(tool)));
        assert!(!opens_assistant_view(&None));
        assert!(opens_assistant_view(&Some(assistant)));
    }

    #[test]
    fn active_branch_follows_parents_and_skips_abandoned_entries() {
        let entries = vec![
            entry("root", None),
            entry("a", Some("root")),
            entry("b", Some("a")),
            entry("abandoned", Some("root")),
        ];
        let ids = active_branch(&entries, Some("b"))
            .into_iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["root", "a", "b"]);
    }

    #[test]
    fn active_branch_falls_back_when_leaf_is_unknown() {
        let entries = vec![entry("root", None), entry("a", Some("root"))];
        assert_eq!(active_branch(&entries, Some("missing")).len(), 2);
        assert_eq!(active_branch(&entries, None).len(), 2);
    }

    pub(super) fn empty_state() -> AppState {
        AppState {
            conn: Conn::Idle,
            environment: Environment::default(),
            cwd: None,
            transcript: Vec::new(),
            streaming: false,
            models: Vec::new(),
            current_model: None,
            thinking_levels: Vec::new(),
            thinking_level: None,
            session_groups: Vec::new(),
            sidebar_rows: Vec::new(),
            collapsed_projects: std::collections::HashSet::new(),
            user_projects: Vec::new(),
            project_labels: HashMap::new(),
            pinned_sessions: HashSet::new(),
            archived_sessions: HashSet::new(),
            session_name: None,
            session_file: None,
            session_id: None,
            draft_session_file: None,
            awaiting_new_session_file: false,
            last_session_loaded: false,
            stats: None,
            notice: None,
            last_project: None,
            commands: Vec::new(),
            workspace_files: Vec::new(),
            pending_dialog: None,
            pending_images: Vec::new(),
            next_image_key: 1,
            session_epoch: 0,
            pending_steers: Vec::new(),
            queued_steers: Vec::new(),
            queued_follow_ups: Vec::new(),
            expanded_turns: std::collections::HashSet::new(),
            turn_running: false,
            changes: Vec::new(),
            changes_seq: 0,
            pending_writes: HashMap::new(),
            pending_composer_text: None,
            session_scan: Arc::new(Mutex::new(sessions::SessionScanner::default())),
            client: None,
        }
    }

    /// One scanned session summary for sidebar-row tests.
    pub(super) fn summary(cwd: &str, id: &str, path: &str) -> sessions::SessionSummary {
        sessions::SessionSummary {
            path: std::path::PathBuf::from(path),
            session_id: id.to_string(),
            cwd: cwd.to_string(),
            timestamp: None,
            updated_secs: Some(100),
            name: Some(format!("session {id}")),
            preview: None,
            message_count: 1,
        }
    }

    fn message_entry(id: &str, parent: Option<&str>, message: AgentMessage) -> SessionEntry {
        SessionEntry {
            entry_type: "message".into(),
            id: id.into(),
            parent_id: parent.map(str::to_string),
            timestamp: None,
            message: Some(message),
        }
    }

    #[test]
    fn load_entries_follows_the_branch_and_orders_cards_after_messages() {
        let mut state = empty_state();
        let user = AgentMessage::User(pi_rpc::protocol::UserMessage {
            content: pi_rpc::protocol::UserContent::Text("hi".into()),
            timestamp: 1.0,
            attachments: None,
        });
        let assistant = AgentMessage::Assistant(pi_rpc::protocol::AssistantMessage {
            content: vec![
                pi_rpc::protocol::AssistantContent::Text {
                    text: "hello".into(),
                },
                pi_rpc::protocol::AssistantContent::ToolCall(pi_rpc::protocol::ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                }),
            ],
            api: None,
            provider: None,
            model: None,
            usage: None,
            stop_reason: None,
            error_message: None,
            timestamp: 2.0,
        });
        let result = AgentMessage::ToolResult(pi_rpc::protocol::ToolResultMessage {
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            content: vec![pi_rpc::protocol::ToolResultBlock::Text {
                text: "a.txt".into(),
            }],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 3.0,
        });
        let abandoned = AgentMessage::User(pi_rpc::protocol::UserMessage {
            content: pi_rpc::protocol::UserContent::Text("abandoned".into()),
            timestamp: 4.0,
            attachments: None,
        });
        let entries = vec![
            message_entry("u1", None, user),
            message_entry("a1", Some("u1"), assistant),
            message_entry("t1", Some("a1"), result),
            message_entry("x1", Some("u1"), abandoned),
        ];

        state.load_entries(&entries, Some("t1"));

        let kinds = state
            .transcript
            .iter()
            .map(|item| match item {
                TranscriptItem::User { text, .. } => format!("user:{text}"),
                TranscriptItem::Turn { .. } => "turn".to_string(),
                TranscriptItem::Assistant(view) => format!("assistant:{}", view.text()),
                TranscriptItem::Tool(card) => {
                    format!("tool:{}:{:?}", card.name, card.result.as_deref())
                }
                TranscriptItem::Notice { text, .. } => format!("notice:{text}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["user:hi", "assistant:hello", "tool:bash:Some(\"a.txt\")"],
            "only the active branch is rendered and the card follows the message"
        );
    }

    #[test]
    fn cancelled_session_actions_are_recognized() {
        let response = Response {
            id: None,
            record_type: "response".into(),
            command: "switch_session".into(),
            success: true,
            error: None,
            data: Some(serde_json::json!({"cancelled": true})),
        };
        assert!(session_action_cancelled(&response));

        let response = Response {
            data: Some(serde_json::json!({"cancelled": false})),
            ..response
        };
        assert!(!session_action_cancelled(&response));
    }

    const PATCH: &str = "--- src/a.rs
+++ src/a.rs
@@ -1,3 +1,4 @@
 let a = 1;
-let b = 2;
+let b = 3;
+let c = 4;
 let d = 5;
";

    #[test]
    fn a_finished_turn_folds_its_content_until_reopened() {
        let user = || TranscriptItem::User {
            text: "hi".into(),
            time: None,
        };
        let assistant = || TranscriptItem::Assistant(Box::new(AssistantView::streaming()));
        let turn = |took: Option<Duration>| TranscriptItem::Turn {
            started: Instant::now(),
            took,
        };
        let items = vec![
            user(),
            turn(Some(Duration::from_secs(4))),
            assistant(),
            TranscriptItem::Tool(ToolView {
                call_id: "c".into(),
                name: "bash".into(),
                args: None,
                running: false,
                failed: false,
                result: None,
                started: Instant::now(),
                took: None,
            }),
            user(),
            turn(None),
            assistant(),
        ];
        let none = std::collections::HashSet::new();
        assert!(!turn_folded_for(&items, 1, &none), "the status row itself");
        assert!(
            turn_folded_for(&items, 2, &none),
            "text stays visible though"
        );
        assert!(turn_folded_for(&items, 3, &none), "tool hides while folded");
        assert!(
            !turn_folded_for(&items, 6, &none),
            "running turn stays open"
        );

        let mut open = std::collections::HashSet::new();
        open.insert(1u64);
        assert!(!turn_folded_for(&items, 3, &open), "re-opened shows tools");
    }

    #[test]
    fn the_fork_point_is_the_user_prompt_that_started_the_turn() {
        let user = |text: &str| TranscriptItem::User {
            text: text.into(),
            time: None,
        };
        let assistant = || TranscriptItem::Assistant(Box::new(AssistantView::streaming()));
        let items = vec![user("hi"), assistant(), user("hi"), assistant()];
        assert_eq!(fork_point_for(&items, 1), Some(("hi".to_string(), 0)));
        assert_eq!(fork_point_for(&items, 3), Some(("hi".to_string(), 1)));
        assert_eq!(fork_point_for(&items, 0), None, "not an assistant row");
    }

    #[test]
    fn timestamps_understand_seconds_and_milliseconds() {
        assert_eq!(unix_secs(0.0), None);
        assert_eq!(unix_secs(1_762_700_000.0), Some(1_762_700_000));
        assert_eq!(unix_secs(1_762_700_000_000.0), Some(1_762_700_000));
    }

    #[test]
    fn a_patch_becomes_hunks_with_both_sides_numbered() {
        let (path, hunks) = parse_patch(PATCH).expect("a patch with hunks parses");
        assert_eq!(path, "src/a.rs");
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Stretch::Hunk { header, lines } => {
                assert_eq!(header.as_ref(), "@@ -1,3 +1,4 @@");
                let kinds: Vec<LineKind> = lines.iter().map(|line| line.kind).collect();
                assert_eq!(
                    kinds,
                    [
                        LineKind::Same,
                        LineKind::Removed,
                        LineKind::Added,
                        LineKind::Added,
                        LineKind::Same
                    ]
                );
                assert_eq!(lines[0].old, Some(1));
                assert_eq!(lines[0].new, Some(1));
                assert_eq!(lines[2].new, Some(2), "the first added line lands at 2");
                assert_eq!(lines[4].old, Some(3));
                assert_eq!(lines[4].new, Some(4));
            }
            other => panic!("expected a hunk, got {other:?}"),
        }
        assert_eq!(stretches_stat(&hunks), (2, 1));
    }

    #[test]
    fn git_style_prefixes_are_stripped_from_the_path() {
        let patch = "--- a/src/x.rs
+++ b/src/x.rs
@@ -1 +1 @@
-a
+b
";
        let (path, _) = parse_patch(patch).expect("parses");
        assert_eq!(path, "src/x.rs");
    }

    #[test]
    fn a_patch_without_hunks_is_not_a_change() {
        assert!(
            parse_patch(
                "--- a
+++ b
"
            )
            .is_none()
        );
    }

    #[test]
    fn a_write_shows_its_whole_file_as_added() {
        let hunks = added_stretches(
            "one
two
",
        );
        assert_eq!(stretches_stat(&hunks), (2, 0));
        match &hunks[0] {
            Stretch::Hunk { header, lines } => {
                assert_eq!(header.as_ref(), "@@ -0,0 +1,2 @@");
                assert_eq!(lines[1].text.as_ref(), "two");
                assert_eq!(lines[1].new, Some(2));
            }
            other => panic!("expected a hunk, got {other:?}"),
        }
    }

    #[test]
    fn edits_to_one_file_merge_into_one_row() {
        let mut state = empty_state();
        state.record_patch(PATCH);
        state.record_patch(PATCH);
        assert_eq!(state.changes.len(), 1, "one file, one row");
        let change = &state.changes[0];
        assert_eq!(change.path.as_ref(), "src/a.rs");
        assert_eq!((change.added, change.removed), (4, 2));
        assert_eq!(change.stretches.len(), 2, "both edits stay viewable");
    }

    #[test]
    fn a_restored_session_rebuilds_its_changes() {
        let mut state = empty_state();
        let call = pi_rpc::protocol::ToolCall {
            id: "call-w".into(),
            name: "write".into(),
            arguments: serde_json::json!({"path": "notes.md", "content": "hello
"}),
        };
        let assistant = AgentMessage::Assistant(pi_rpc::protocol::AssistantMessage {
            content: vec![
                pi_rpc::protocol::AssistantContent::Text {
                    text: "writing".into(),
                },
                pi_rpc::protocol::AssistantContent::ToolCall(call),
            ],
            api: None,
            provider: None,
            model: None,
            usage: None,
            stop_reason: None,
            error_message: None,
            timestamp: 1.0,
        });
        let result = AgentMessage::ToolResult(pi_rpc::protocol::ToolResultMessage {
            tool_call_id: "call-w".into(),
            tool_name: "write".into(),
            content: Vec::new(),
            details: None,
            usage: None,
            is_error: false,
            timestamp: 2.0,
        });
        let edit = AgentMessage::ToolResult(pi_rpc::protocol::ToolResultMessage {
            tool_call_id: "call-e".into(),
            tool_name: "edit".into(),
            content: Vec::new(),
            details: Some(serde_json::json!({ "patch": PATCH })),
            usage: None,
            is_error: false,
            timestamp: 3.0,
        });
        let entries = vec![
            message_entry("a1", None, assistant),
            message_entry("t1", Some("a1"), result),
            message_entry("t2", Some("t1"), edit),
        ];
        state.load_entries(&entries, Some("t2"));

        let paths: Vec<&str> = state
            .changes
            .iter()
            .map(|change| change.path.as_ref())
            .collect();
        assert_eq!(paths, ["src/a.rs", "notes.md"], "newest edit first");
        assert_eq!(state.changes[1].added, 1);
    }
}

/// The largest image one message may carry, per attachment.
pub const MAX_IMAGE_BYTES: usize = 50 * 1024 * 1024;

/// Where staged images' thumbnails live.
fn preview_dir() -> PathBuf {
    std::env::temp_dir().join("pido-previews")
}

/// An image staged for the next message: raw bytes plus the mime pi needs.
#[derive(Debug, Clone)]
pub struct PendingImage {
    pub key: String,
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
    /// The staged bytes written to a temp file, for the chip's thumbnail.
    pub preview: PathBuf,
}

/// Sniff an image's format from its magic bytes: `(mime, extension)`.
fn sniff_image(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    const MIME_PNG: &str = "image/png";
    const MIME_JPEG: &str = "image/jpeg";
    const MIME_GIF: &str = "image/gif";
    const MIME_WEBP: &str = "image/webp";
    const MIME_BMP: &str = "image/bmp";
    let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let jpeg = [0xffu8, 0xd8, 0xff];
    let riff = b"RIFF";
    let webp = b"WEBP";
    if bytes.starts_with(&png) {
        Some((MIME_PNG, "png"))
    } else if bytes.starts_with(&jpeg) {
        Some((MIME_JPEG, "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some((MIME_GIF, "gif"))
    } else if bytes.len() >= 12 && bytes.starts_with(riff) && bytes[8..12] == *webp {
        Some((MIME_WEBP, "webp"))
    } else if bytes.starts_with(b"BM") {
        Some((MIME_BMP, "bmp"))
    } else {
        None
    }
}

/// Read the image files at `paths` into staged attachments; non-image files
/// are skipped with a notice.
pub fn stage_image_files(state: &Entity<AppState>, paths: &[std::path::PathBuf], cx: &mut App) {
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        state.update(cx, |s, cx| {
            if s.stage_image(Some(name), bytes) {
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod image_tests {
    use super::MAX_IMAGE_BYTES;
    use super::tests::empty_state;

    #[test]
    fn stage_sniffs_formats_and_rejects_junk() {
        let mut state = empty_state();
        let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        assert!(state.stage_image(None, png.to_vec()));
        assert_eq!(state.pending_images.len(), 1);
        assert_eq!(state.pending_images[0].mime, "image/png");
        assert_eq!(state.pending_images[0].name, "img-1.png");
        // A thumbnail file is staged beside the bytes for the chip.
        assert!(state.pending_images[0].preview.is_file());

        assert!(state.stage_image(Some("photo.jpg".into()), vec![0xff, 0xd8, 0xff, 0xaa]));
        assert_eq!(state.pending_images[1].mime, "image/jpeg");

        assert!(!state.stage_image(None, b"just text".to_vec()));
        assert_eq!(state.pending_images.len(), 2, "junk must not stage");

        let removed_preview = state.pending_images[0].preview.clone();
        state.remove_image("img-1");
        assert_eq!(state.pending_images.len(), 1);
        assert_eq!(state.pending_images[0].name, "photo.jpg");
        assert!(!removed_preview.is_file(), "preview file cleaned up");
    }

    #[test]
    fn oversized_images_are_refused() {
        let mut state = empty_state();
        let big = vec![0x89u8, b'P', b'N', b'G', 0, 0, 0, 0]
            .into_iter()
            .chain(std::iter::repeat_n(0u8, MAX_IMAGE_BYTES + 1))
            .collect::<Vec<_>>();
        assert!(!state.stage_image(None, big));
        assert!(state.pending_images.is_empty());
    }
}

#[cfg(test)]
mod sidebar_tests {
    use super::*;

    fn group(cwd: &str, sessions: usize) -> SessionGroup {
        SessionGroup {
            cwd: cwd.to_string(),
            sessions: (0..sessions)
                .map(|ix| pi_rpc::sessions::SessionSummary {
                    path: std::path::PathBuf::from(format!("{cwd}/s{ix}.jsonl")),
                    session_id: format!("s{ix}"),
                    cwd: cwd.to_string(),
                    timestamp: None,
                    updated_secs: Some(100),
                    name: Some(format!("session {ix}")),
                    preview: None,
                    message_count: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn folding_a_project_hides_its_sessions() {
        let groups = vec![group("D:/a", 3), group("D:/b", 2)];
        let open = flatten_groups(
            &groups,
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(open.len(), 2 + 3 + 2, "two headers and five sessions");

        let mut folded = std::collections::HashSet::new();
        folded.insert("D:/a".to_string());
        let rows = flatten_groups(
            &groups,
            &folded,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(
            rows.len(),
            2 + 2,
            "the folded project keeps only its header"
        );
        match &rows[0] {
            SidebarRow::Group {
                folded: is_folded,
                sessions,
                ..
            } => {
                assert!(is_folded);
                assert_eq!(*sessions, 3);
            }
            other => panic!("expected a group row, got {other:?}"),
        }
        match &rows[1] {
            SidebarRow::Group {
                folded: is_folded, ..
            } => assert!(!is_folded),
            other => panic!("expected the second group, got {other:?}"),
        }
    }

    #[test]
    fn pinned_sessions_lead_and_archived_ones_fold_to_the_bottom() {
        let groups = vec![group("D:/a", 3), group("D:/b", 1)];
        let mut pinned = HashSet::new();
        pinned.insert(session_key("D:/a/s2.jsonl"));

        let rows = flatten_groups(
            &groups,
            &Default::default(),
            &Default::default(),
            &pinned,
            &Default::default(),
        );
        match &rows[1] {
            SidebarRow::Session { path, pinned, .. } => {
                assert!(path.ends_with("s2.jsonl"), "the pinned session leads");
                assert!(pinned);
            }
            other => panic!("expected a session row, got {other:?}"),
        }
        assert_eq!(rows.len(), 2 + 4, "both projects unfolded");

        let mut archived = HashSet::new();
        archived.insert(session_key("D:/a/s1.jsonl"));
        let rows = flatten_groups(
            &groups,
            &Default::default(),
            &Default::default(),
            &HashSet::new(),
            &archived,
        );
        let group_a = match &rows[0] {
            SidebarRow::Group { sessions, .. } => *sessions,
            other => panic!("expected a group, got {other:?}"),
        };
        assert_eq!(group_a, 2, "the archived session leaves its project");
        match rows.last().unwrap() {
            SidebarRow::Session { path, archived, .. } => {
                assert!(path.ends_with("s1.jsonl"));
                assert!(archived);
            }
            other => panic!("expected the archived session last, got {other:?}"),
        }
        match &rows[rows.len() - 2] {
            SidebarRow::Group {
                archived: is_archive,
                ..
            } => assert!(is_archive, "the archive fold precedes its rows"),
            other => panic!("expected the archive fold, got {other:?}"),
        }
    }

    #[test]
    fn toggling_folds_and_unfolds() {
        let mut collapsed = std::collections::HashSet::new();
        toggle_fold(&mut collapsed, "D:/a");
        assert!(collapsed.contains("D:/a"));
        toggle_fold(&mut collapsed, "D:/a");
        assert!(!collapsed.contains("D:/a"));
    }

    #[test]
    fn an_unsaved_new_session_leads_its_project() {
        let mut state = super::tests::empty_state();
        state.session_groups = vec![group("D:/a", 2), group("D:/b", 1)];
        state.cwd = Some(std::path::PathBuf::from("D:/b"));
        state.session_file = Some("D:/b/new.jsonl".into());
        state.draft_session_file = Some("D:/b/new.jsonl".into());
        state.session_id = Some("fresh".into());

        let rows = state.sidebar_rows_now();
        match &rows[0] {
            SidebarRow::Group { cwd, sessions, .. } => {
                assert_eq!(cwd.as_ref(), "D:/b", "the draft's project leads the list");
                assert_eq!(*sessions, 2, "the draft counts as one of its sessions");
            }
            other => panic!("expected the draft's project first, got {other:?}"),
        }
        match &rows[1] {
            SidebarRow::Session { path, .. } => {
                assert!(path.ends_with("new.jsonl"), "the draft row sits under it");
                assert!(state.is_draft_session(path));
            }
            other => panic!("expected the draft row, got {other:?}"),
        }
    }

    #[test]
    fn a_draft_opens_a_project_with_no_sessions_on_disk() {
        let mut state = super::tests::empty_state();
        state.session_groups = vec![group("D:/a", 1)];
        state.cwd = Some(std::path::PathBuf::from("D:/fresh"));
        state.session_file = Some("D:/fresh/new.jsonl".into());
        state.draft_session_file = Some("D:/fresh/new.jsonl".into());

        let rows = state.sidebar_rows_now();
        assert_eq!(
            rows.len(),
            4,
            "a fresh header and row, then the old project"
        );
        match &rows[0] {
            SidebarRow::Group { cwd, sessions, .. } => {
                assert_eq!(cwd.as_ref(), "D:/fresh");
                assert_eq!(*sessions, 1);
            }
            other => panic!("expected the draft's project first, got {other:?}"),
        }
    }

    #[test]
    fn a_scanned_session_replaces_its_draft_row() {
        let mut state = super::tests::empty_state();
        state.session_groups = vec![group("D:/a", 1)];
        state.cwd = Some(std::path::PathBuf::from("D:/a"));
        state.session_file = Some("D:/a/new.jsonl".into());
        state.draft_session_file = Some("D:/a/new.jsonl".into());

        assert_eq!(state.sidebar_rows_now().len(), 3);

        // The first reply writes the file; the scan now lists it under the
        // same path, so the draft must not double the row.
        state.session_groups[0]
            .sessions
            .insert(0, super::tests::summary("D:/a", "new", "D:/a/new.jsonl"));
        let rows = state.sidebar_rows_now();
        assert_eq!(rows.len(), 3, "the scanned row takes the draft's place");
        match &rows[1] {
            SidebarRow::Session { path, .. } => assert!(path.ends_with("new.jsonl")),
            other => panic!("expected the scanned session, got {other:?}"),
        }
    }

    #[test]
    fn renaming_retitles_only_the_matching_row() {
        let mut rows = vec![
            SidebarRow::Group {
                label: "a".into(),
                cwd: "D:/a".into(),
                folded: false,
                sessions: 2,
                archived: false,
            },
            SidebarRow::Session {
                path: "D:/a/one.jsonl".into(),
                session_id: "one".into(),
                title: "old".into(),
                updated: Some(1),
                pinned: false,
                archived: false,
            },
            SidebarRow::Session {
                path: "D:/a/two.jsonl".into(),
                session_id: "two".into(),
                title: "keep".into(),
                updated: Some(2),
                pinned: false,
                archived: false,
            },
        ];
        patch_session_title(&mut rows, "D:/a/one.jsonl", "renamed");
        match (&rows[1], &rows[2]) {
            (
                SidebarRow::Session { title: renamed, .. },
                SidebarRow::Session { title: kept, .. },
            ) => {
                assert_eq!(renamed.as_ref(), "renamed");
                assert_eq!(kept.as_ref(), "keep");
            }
            _ => panic!("expected two session rows"),
        }
    }
}
