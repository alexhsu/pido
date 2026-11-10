//! Application state: one Pi session, its transcript, and the bridge from
//! the RPC client's threads into GPUI's main thread.
//!
//! All blocking work (spawning pi, awaiting responses, scanning sessions)
//! runs on GPUI's background executor; results hop back through
//! `WeakEntity::update`, so the UI never blocks. The event pump subscribes to
//! the client and folds every record into the transcript.

use base64::Engine as _;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    User { text: gpui::SharedString },
    Assistant(Box<AssistantView>),
    Tool(ToolView),
    Notice { kind: NoticeKind, text: String },
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
    /// for starting a session there) and whether it is folded.
    Group {
        label: gpui::SharedString,
        cwd: gpui::SharedString,
        folded: bool,
        sessions: usize,
    },
    /// A session file.
    Session {
        path: gpui::SharedString,
        title: gpui::SharedString,
        /// Last activity, unix seconds: the age label is built at draw time,
        /// so it follows the locale without rebuilding the rows.
        updated: Option<i64>,
    },
}

/// Flatten grouped sessions into the sidebar's row order.
fn flatten_groups(
    groups: &[SessionGroup],
    collapsed: &std::collections::HashSet<String>,
) -> Vec<SidebarRow> {
    let mut rows = Vec::with_capacity(
        groups
            .iter()
            .map(|group| group.sessions.len() + 1)
            .sum::<usize>(),
    );
    for group in groups {
        let folded = collapsed.contains(&group.cwd);
        rows.push(SidebarRow::Group {
            label: shorten_project(&group.cwd).into(),
            cwd: group.cwd.clone().into(),
            folded,
            sessions: group.sessions.len(),
        });
        if folded {
            continue;
        }
        for session in &group.sessions {
            let title = session
                .name
                .clone()
                .or_else(|| session.preview.clone())
                .unwrap_or_else(|| session.session_id.clone());
            rows.push(SidebarRow::Session {
                path: session.path.to_string_lossy().into_owned().into(),
                title: title.into(),
                updated: session.updated_secs,
            });
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

/// `D:\develop\code\pido` → `code / pido`, so a project header stays short.
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
    /// Projects the user folded shut, keyed by working directory.
    pub collapsed_projects: std::collections::HashSet<String>,
    /// The sidebar's rows, flattened once per scan: a header per project and
    /// a row per session. Rendering reads these instead of rebuilding strings
    /// per frame, and the list virtualises over the same order.
    pub sidebar_rows: Vec<SidebarRow>,
    pub session_name: Option<String>,
    /// The active session's file path, as reported by `get_state`.
    pub session_file: Option<String>,
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
    /// Text Esc or a failed send wants put back into the composer. The root
    /// view drains this into the draft field.
    pub pending_composer_text: Option<String>,
    /// Cached session scan keyed by file metadata.
    session_scan: Arc<Mutex<sessions::SessionScanner>>,
    client: Option<PiClient>,
}

impl AppState {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let last_project = crate::settings::Settings::load().last_project;
        cx.new(|cx| {
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
                collapsed_projects: std::collections::HashSet::new(),
                session_name: None,
                session_file: None,
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
                                    s.streaming = payload.is_streaming;
                                    s.remember_session_file(payload.session_file.as_deref());
                                }
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

    /// Fold or unfold a project in the sidebar.
    pub fn toggle_project(&mut self, cwd: &str, cx: &mut Context<Self>) {
        toggle_fold(&mut self.collapsed_projects, cwd);
        self.sidebar_rows = flatten_groups(&self.session_groups, &self.collapsed_projects);
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

    /// Drop the session and return to the welcome screen.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
        self.conn = Conn::Idle;
        self.transcript.clear();
        self.streaming = false;
        self.stats = None;
        self.session_name = None;
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
                s.sidebar_rows = flatten_groups(&groups, &s.collapsed_projects);
                s.session_groups = groups;
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
        }
        self.transcript.push(TranscriptItem::User {
            text: text.clone().into(),
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
            .rposition(|item| matches!(item, TranscriptItem::User { text: t } if t == text))
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
                            s.session_name = None;
                            s.stats = None;
                            s.session_epoch += 1;
                            s.pending_steers.clear();
                            s.push_notice(
                                NoticeKind::Info,
                                i18n::t(cx, "new_session_started").to_string(),
                            );
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
                            s.session_name = None;
                            s.stats = None;
                            s.session_epoch += 1;
                            s.pending_steers.clear();
                            s.remember_session_file(Some(&session_file));
                            s.apply_session_cwd(cwd, cx);
                            s.push_notice(
                                NoticeKind::Info,
                                i18n::t(cx, "session_restored").to_string(),
                            );
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
                    s.remember_session_file(payload.session_file.as_deref());
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

    fn push_notice(&mut self, kind: NoticeKind, text: String) {
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
                is_error,
                result,
                ..
            } => {
                if let Some(card) = self.tool_card_mut(&tool_call_id) {
                    card.running = false;
                    card.failed = is_error;
                    card.took = Some(card.started.elapsed());
                    if let Some(result) = result {
                        card.result = Some(truncate_for_display(&result.text(), 2000).into());
                    }
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
                        self.transcript
                            .push(TranscriptItem::User { text: text.into() });
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
            content: None,
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
            session_name: None,
            session_file: None,
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
            pending_composer_text: None,
            session_scan: Arc::new(Mutex::new(sessions::SessionScanner::default())),
            client: None,
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
            content: Some(pi_rpc::protocol::ToolResultContent {
                content: vec![pi_rpc::protocol::ToolResultBlock::Text {
                    text: "a.txt".into(),
                }],
                details: None,
            }),
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
                TranscriptItem::User { text } => format!("user:{text}"),
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
        let open = flatten_groups(&groups, &Default::default());
        assert_eq!(open.len(), 2 + 3 + 2, "two headers and five sessions");

        let mut folded = std::collections::HashSet::new();
        folded.insert("D:/a".to_string());
        let rows = flatten_groups(&groups, &folded);
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
    fn toggling_folds_and_unfolds() {
        let mut collapsed = std::collections::HashSet::new();
        toggle_fold(&mut collapsed, "D:/a");
        assert!(collapsed.contains("D:/a"));
        toggle_fold(&mut collapsed, "D:/a");
        assert!(!collapsed.contains("D:/a"));
    }

    #[test]
    fn renaming_retitles_only_the_matching_row() {
        let mut rows = vec![
            SidebarRow::Group {
                label: "a".into(),
                cwd: "D:/a".into(),
                folded: false,
                sessions: 2,
            },
            SidebarRow::Session {
                path: "D:/a/one.jsonl".into(),
                title: "old".into(),
                updated: Some(1),
            },
            SidebarRow::Session {
                path: "D:/a/two.jsonl".into(),
                title: "keep".into(),
                updated: Some(2),
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
