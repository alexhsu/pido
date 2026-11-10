//! Application state: one Pi session, its transcript, and the bridge from
//! the RPC client's threads into GPUI's main thread.
//!
//! All blocking work (spawning pi, awaiting responses, scanning sessions)
//! runs on GPUI's background executor; results hop back through
//! `WeakEntity::update`, so the UI never blocks. The event pump subscribes to
//! the client and folds every record into the transcript.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity};
use pi_rpc::client::{ClientEvent, PiClient, PiProcessConfig};
use pi_rpc::protocol::{
    AgentMessage, AssistantMessageEvent, Command, Event, Model, Response, RpcRecord, SessionEntry,
    SessionState, SessionStats, ThinkingLevel,
};
use pi_rpc::sessions::{self, SessionGroup};

use crate::i18n;

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
    User { text: String },
    Assistant(Box<AssistantView>),
    Tool(ToolView),
    Notice { kind: NoticeKind, text: String },
}

#[derive(Debug, Clone)]
pub struct AssistantView {
    pub thinking: String,
    pub thinking_live: bool,
    pub text: String,
    pub text_live: bool,
    pub finished: bool,
    pub started: Instant,
    pub took: Option<Duration>,
}

impl AssistantView {
    fn streaming() -> Self {
        Self {
            thinking: String::new(),
            thinking_live: false,
            text: String::new(),
            text_live: false,
            finished: false,
            started: Instant::now(),
            took: None,
        }
    }

    fn from_message(message: &pi_rpc::protocol::AssistantMessage) -> Self {
        let mut view = Self::streaming();
        for block in &message.content {
            match block {
                pi_rpc::protocol::AssistantContent::Text { text } => {
                    view.text.push_str(text);
                }
                pi_rpc::protocol::AssistantContent::Thinking { thinking } => {
                    view.thinking.push_str(thinking);
                }
                pi_rpc::protocol::AssistantContent::ToolCall(_) => {}
            }
        }
        view.finished = true;
        view
    }
}

#[derive(Debug, Clone)]
pub struct ToolView {
    pub call_id: String,
    pub name: String,
    /// Pretty-printed arguments, when known.
    pub args: Option<String>,
    pub running: bool,
    pub failed: bool,
    pub result: Option<String>,
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
    pub session_groups: Vec<SessionGroup>,
    pub session_name: Option<String>,
    pub stats: Option<SessionStats>,
    /// A dismissible banner over the transcript.
    pub notice: Option<(NoticeKind, String)>,
    /// The last-used project directory, to prefill the welcome picker.
    pub last_project: Option<String>,
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
                session_name: None,
                stats: None,
                notice: None,
                last_project,
                client: None,
            };
            state.probe_environment(cx);
            state
        })
    }

    fn client(&self) -> Option<PiClient> {
        self.client.clone()
    }

    // -- environment --------------------------------------------------------

    /// Probe the pi CLI and provider auth in the background for the welcome
    /// screen. Probes the common providers; only ready ones are reported.
    fn probe_environment(&self, cx: &Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let pi_version = pi_rpc::client::probe_pi_install(None);
                    let mut ready_providers = Vec::new();
                    if pi_version.is_some() {
                        for provider in ["deepseek", "anthropic", "openai", "openrouter", "google"]
                        {
                            if pi_rpc::client::probe_auth(provider, None).unwrap_or(false) {
                                ready_providers.push(provider.to_string());
                            }
                        }
                    }
                    Environment {
                        pi_version,
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

    /// Spawn pi in `cwd` and initialize the session.
    pub fn connect(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        if !cwd.is_dir() {
            self.notice = Some((NoticeKind::Error, i18n::t(cx, "dir_invalid").to_string()));
            cx.notify();
            return;
        }
        self.conn = Conn::Connecting;
        self.cwd = Some(cwd.clone());
        self.transcript.clear();
        self.notice = None;
        self.last_project = Some(cwd.to_string_lossy().into_owned());
        crate::settings::save_setting(|s| {
            s.last_project = self.last_project.clone();
        });
        cx.notify();

        let config = PiProcessConfig::new(&cwd);
        log::info!("state: connecting in {}", config.cwd.display());
        cx.spawn(async move |this, cx| {
            let spawned = cx
                .background_executor()
                .spawn(async move { PiClient::spawn(&config) })
                .await;
            match spawned {
                Ok(client) => {
                    log::info!("state: pi spawned, running init requests");
                    let init_client = client.clone();
                    let init = cx
                        .background_executor()
                        .spawn(async move {
                            let state = init_client.request(Command::GetState);
                            let models = init_client.request(Command::GetAvailableModels);
                            let levels = init_client.request(Command::GetAvailableThinkingLevels);
                            (state, models, levels)
                        })
                        .await;
                    let (state, models, levels) = init;
                    log::info!(
                        "state: init done, get_state ok={}",
                        state.as_ref().map(|r| r.success).unwrap_or(false)
                    );
                    this.update(cx, |s, cx| {
                        let failed = state.as_ref().err().map(|e| e.to_string());
                        log::info!("state: applying connect result, failed={failed:?}");
                        s.client = Some(client.clone());
                        match failed {
                            Some(error) => {
                                s.conn = Conn::Dead(error);
                            }
                            None => {
                                s.conn = Conn::Live;
                                if let Ok(Ok(Some(payload))) =
                                    state.map(|r| r.data_as::<SessionState>())
                                {
                                    s.current_model = payload.model;
                                    s.thinking_level = payload.thinking_level;
                                    s.session_name = payload.session_name;
                                }
                                if let Ok(Ok(Some(payload))) =
                                    models.map(|r| r.data_as::<pi_rpc::protocol::AvailableModels>())
                                {
                                    s.models = payload.models;
                                }
                                if let Ok(Ok(Some(payload))) = levels.map(|r| {
                                    r.data_as::<pi_rpc::protocol::AvailableThinkingLevels>()
                                }) {
                                    s.thinking_levels = payload.levels;
                                }
                                s.start_pump(client.clone(), cx);
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
        cx.notify();
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

    /// Rescan `~/.pi/agent/sessions` in the background.
    pub fn refresh_sessions(&self, cx: &Context<Self>) {
        cx.spawn(async move |this, cx| {
            let groups = cx
                .background_executor()
                .spawn(async move {
                    sessions::default_session_dir()
                        .and_then(|dir| sessions::scan_sessions(&dir).ok())
                        .unwrap_or_default()
                })
                .await;
            this.update(cx, |s, cx| {
                s.session_groups = groups;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -- actions ------------------------------------------------------------

    /// Send a prompt, or steer the running agent. The optimistic user bubble
    /// appears immediately; the wire call happens in the background.
    pub fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        if text.trim().is_empty() || self.conn != Conn::Live {
            return;
        }
        let command = if self.streaming {
            Command::Steer {
                message: text.clone(),
                images: None,
            }
        } else {
            Command::Prompt {
                message: text.clone(),
                images: None,
                streaming_behavior: None,
            }
        };
        self.transcript.push(TranscriptItem::User { text });
        self.check_command(command, "prompt", cx);
        cx.notify();
    }

    /// Interactive Esc semantics: drop queued steering, then abort the run.
    pub fn stop(&mut self, cx: &Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _ = client.request(Command::ClearQueue);
                    client.request(Command::Abort)
                })
                .await;
            this.update(cx, |s, cx| {
                if let Err(error) = result {
                    s.notice = Some((NoticeKind::Error, error.to_string()));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Start a fresh session in the same project.
    pub fn new_chat(&mut self, cx: &mut Context<Self>) {
        if self.conn != Conn::Live {
            return;
        }
        self.check_command(
            Command::NewSession {
                parent_session: None,
            },
            "new_session",
            cx,
        );
        self.transcript.clear();
        self.session_name = None;
        self.stats = None;
        self.push_notice(
            NoticeKind::Info,
            i18n::t(cx, "new_session_started").to_string(),
        );
        self.query_state(cx);
        cx.notify();
    }

    /// Load a past session into the running pi process and rebuild the
    /// transcript from its entries.
    pub fn switch_session(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.conn != Conn::Live {
            return;
        }
        let Some(client) = self.client() else { return };
        self.transcript.clear();
        self.session_name = None;
        self.push_notice(
            NoticeKind::Info,
            i18n::t(cx, "session_restored").to_string(),
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let switched = client.request(Command::SwitchSession {
                        session_path: path.to_string_lossy().into_owned(),
                    })?;
                    if !switched.success {
                        anyhow::bail!("switch_session failed: {:?}", switched.error);
                    }
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
                        s.load_entries(&payload.entries);
                        s.query_state(cx);
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

    pub fn set_model(&mut self, model_id: &str, cx: &mut Context<Self>) {
        let Some(model) = self.models.iter().find(|m| m.id == model_id).cloned() else {
            return;
        };
        let provider = model.provider.clone().unwrap_or_default();
        self.current_model = Some(model);
        self.check_command(
            Command::SetModel {
                provider,
                model_id: model_id.to_string(),
            },
            "set_model",
            cx,
        );
        // Thinking levels change with the model.
        self.refresh_thinking_levels(cx);
        cx.notify();
    }

    pub fn set_thinking(&mut self, level: ThinkingLevel, cx: &mut Context<Self>) {
        self.thinking_level = Some(level);
        self.check_command(Command::SetThinkingLevel { level }, "set_thinking", cx);
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
                    self.conn = Conn::Dead(format!("{reason:?}"));
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
                view.text_live = false;
                view.thinking_live = false;
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
                // MVP: dialog requests are left to time out agent-side;
                // fire-and-forget notices become banners.
                if !request.method.is_dialog() {
                    let text = request
                        .message
                        .or(request.title)
                        .unwrap_or_else(|| i18n::t(cx, "extension_notify").to_string());
                    self.push_notice(NoticeKind::Warn, text);
                } else {
                    log::debug!("extension dialog ignored (agent times out): {request:?}");
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
                // Any assistant view still live is finished now.
                self.finalize_open_views();
                if aborted {
                    self.push_notice(NoticeKind::Warn, i18n::t(cx, "disconnected").to_string());
                }
                self.refresh_stats(cx);
                self.refresh_sessions(cx);
            }
            Event::MessageStart { .. } => {
                self.transcript.push(TranscriptItem::Assistant(Box::new(
                    AssistantView::streaming(),
                )));
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
                    card.result = Some(truncate_for_display(&partial.text(), 2000));
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
                        card.result = Some(truncate_for_display(&result.text(), 2000));
                    }
                }
            }
            Event::CompactionStart { .. } => {}
            Event::CompactionEnd {
                aborted, result, ..
            } => {
                let text = if aborted {
                    format!("{} (aborted)", i18n::t(cx, "context_compacted"))
                } else if result.is_some() {
                    i18n::t(cx, "context_compacted").to_string()
                } else {
                    format!("{}: {}", i18n::t(cx, "context_compacted"), "failed")
                };
                self.push_notice(NoticeKind::Info, text);
            }
            Event::AutoRetryStart { .. } => {
                self.push_notice(NoticeKind::Warn, i18n::t(cx, "retrying").to_string());
            }
            Event::ExtensionError { error, .. } => {
                if let Some(error) = error {
                    self.push_notice(NoticeKind::Error, error);
                }
            }
            Event::QueueUpdate { .. } => {}
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
            AssistantMessageEvent::TextStart { .. } => view.text_live = true,
            AssistantMessageEvent::TextDelta { delta, .. } => view.text.push_str(&delta),
            AssistantMessageEvent::TextEnd { content, .. } => {
                view.text = content;
                view.text_live = false;
            }
            AssistantMessageEvent::ThinkingStart { .. } => view.thinking_live = true,
            AssistantMessageEvent::ThinkingDelta { delta, .. } => view.thinking.push_str(&delta),
            AssistantMessageEvent::ThinkingEnd { .. } => {
                view.thinking_live = false;
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
                card.args = args.map(pretty_json);
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
                args: args.map(pretty_json),
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
    fn load_entries(&mut self, entries: &[SessionEntry]) {
        self.transcript.clear();
        for entry in entries {
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
                        self.transcript.push(TranscriptItem::User { text });
                    }
                }
                AgentMessage::Assistant(assistant) => {
                    for block in &assistant.content {
                        if let pi_rpc::protocol::AssistantContent::ToolCall(call) = block {
                            self.ensure_tool_card(
                                call.id.clone(),
                                call.name.clone(),
                                Some(&call.arguments),
                            );
                        }
                    }
                    let mut view = AssistantView::from_message(assistant);
                    view.took = None;
                    self.transcript
                        .push(TranscriptItem::Assistant(Box::new(view)));
                }
                AgentMessage::ToolResult(result) => {
                    let text = result.text();
                    if let Some(card) = self.tool_card_mut(&result.tool_call_id) {
                        card.running = false;
                        card.failed = result.is_error;
                        card.result = Some(truncate_for_display(&text, 2000));
                    } else {
                        self.transcript.push(TranscriptItem::Tool(ToolView {
                            call_id: result.tool_call_id.clone(),
                            name: result.tool_name.clone(),
                            args: None,
                            running: false,
                            failed: result.is_error,
                            result: Some(truncate_for_display(&text, 2000)),
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
            }
        }
    }
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
