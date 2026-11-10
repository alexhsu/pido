//! Typed records for the Pi RPC JSONL protocol.
//!
//! Field names and shapes follow the protocol spec shipped with Pi 0.85.1
//! (`docs/rpc.md` in the `@earendil-works/pi-coding-agent` package): record
//! and field names are camelCase on the wire, all types tolerate unknown
//! fields so newer Pi versions do not break deserialization.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Commands (written to Pi's stdin)
// ---------------------------------------------------------------------------

/// A command envelope: an optional correlation `id` plus the command itself.
///
/// Responses echo the `id`, and `bash_execution_update` events repeat the `id`
/// of their originating `bash` command, so always set one when you need to
/// correlate.
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub command: Command,
}

impl Request {
    /// Wrap a command with a fresh correlation id.
    pub fn new(command: Command) -> Self {
        Self {
            id: Some(crate::client::next_request_id()),
            command,
        }
    }

    /// Wrap a command without an id (Pi does not require one).
    pub fn anonymous(command: Command) -> Self {
        Self { id: None, command }
    }
}

impl From<Command> for Request {
    fn from(command: Command) -> Self {
        Self::new(command)
    }
}

/// How a `prompt` behaves when the agent is already streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    /// Delivered after the current assistant turn finishes its tool calls.
    Steer,
    /// Delivered only when the agent stops.
    FollowUp,
}

/// An image attachment in Pi's `ImageContent` format.
#[derive(Debug, Clone, Serialize)]
pub struct ImageInput {
    #[serde(rename = "type")]
    pub content_type: &'static str,
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

impl ImageInput {
    pub fn png(data: String) -> Self {
        Self {
            content_type: "image",
            data,
            mime_type: "image/png".into(),
        }
    }

    pub fn jpeg(data: String) -> Self {
        Self {
            content_type: "image",
            data,
            mime_type: "image/jpeg".into(),
        }
    }
}

/// A command written to Pi's stdin, tagged by its `type` field.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Send a user prompt. `success: true` only means *accepted*; watch events
    /// (`agent_settled` in particular) for completion.
    #[serde(rename_all = "camelCase")]
    Prompt {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ImageInput>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    /// Queue a steering message delivered mid-run, after the current turn's
    /// tool calls. Extension commands are not allowed here.
    Steer {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ImageInput>>,
    },
    /// Queue a follow-up delivered when the agent finishes.
    FollowUp {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ImageInput>>,
    },
    /// Abort the current operation; the response arrives once idle.
    Abort,
    /// Remove queued steering/follow-up messages; returns their text.
    ClearQueue,
    /// Start a fresh session.
    #[serde(rename_all = "camelCase")]
    NewSession {
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_session: Option<String>,
    },
    /// Get current session state (model, thinking level, streaming flags...).
    GetState,
    /// Get all messages in the conversation.
    GetMessages,
    /// Switch to a specific model.
    #[serde(rename_all = "camelCase")]
    SetModel {
        provider: String,
        model_id: String,
    },
    /// Cycle to the next model in the configured scope.
    CycleModel,
    /// List all configured models.
    GetAvailableModels,
    /// Set the reasoning/thinking level.
    SetThinkingLevel {
        level: ThinkingLevel,
    },
    /// Cycle through available thinking levels.
    CycleThinkingLevel,
    /// List thinking levels supported by the current model.
    GetAvailableThinkingLevels,
    /// Control how steering messages are delivered.
    SetSteeringMode {
        mode: QueueMode,
    },
    /// Control how follow-up messages are delivered.
    SetFollowUpMode {
        mode: QueueMode,
    },
    /// Manually compact the conversation context.
    #[serde(rename_all = "camelCase")]
    Compact {
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    SetAutoCompaction {
        enabled: bool,
    },
    SetAutoRetry {
        enabled: bool,
    },
    AbortRetry,
    /// Run a shell command now; output joins the LLM context on the next
    /// prompt. Streamed chunks arrive as `bash_execution_update` events
    /// carrying this command's id.
    Bash {
        command: String,
    },
    AbortBash,
    GetSessionStats,
    #[serde(rename_all = "camelCase")]
    ExportHtml {
        #[serde(skip_serializing_if = "Option::is_none")]
        output_path: Option<String>,
    },
    /// Load a different session file.
    #[serde(rename_all = "camelCase")]
    SwitchSession {
        session_path: String,
    },
    /// Fork the active branch from a previous user message.
    #[serde(rename_all = "camelCase")]
    Fork {
        entry_id: String,
    },
    /// Duplicate the current active branch into a new session.
    Clone,
    GetForkMessages,
    /// All session entries in append order; `since` is a durable cursor.
    GetEntries {
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    /// The session as a tree of entries.
    GetTree,
    GetLastAssistantText,
    SetSessionName {
        name: String,
    },
    GetCommands,
    /// Answer an `extension_ui_request` dialog (select/input/editor/confirm).
    #[serde(rename_all = "camelCase")]
    ExtensionUiResponse {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        confirmed: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        cancelled: Option<bool>,
    },
}

/// Reasoning/thinking levels. `xhigh` and `max` exist only for models that
/// support them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Delivery mode for steering and follow-up queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueMode {
    All,
    OneAtATime,
}

// ---------------------------------------------------------------------------
// Records (read from Pi's stdout)
// ---------------------------------------------------------------------------

/// Any record read from Pi's stdout.
#[derive(Debug, Clone)]
pub enum RpcRecord {
    /// Response to a command we sent, correlated by `id`.
    Response(Response),
    /// A session event (agent lifecycle, streaming, tool execution...).
    Event(Event),
    /// An extension asking the user something, or pushing status.
    ExtensionUiRequest(ExtensionUiRequest),
    /// A record whose `type` we do not know (forward compatibility).
    Unknown { record_type: String, raw: Value },
}

/// Parse one JSONL line from Pi's stdout into a typed record.
///
/// Record and event types this crate does not know are preserved as
/// [`RpcRecord::Unknown`] rather than failing, so a newer Pi never breaks the
/// client. A known type that fails to parse is a real error and surfaces as
/// `Err`.
pub fn parse_record(line: &str) -> anyhow::Result<RpcRecord> {
    let value: Value = serde_json::from_str(line.trim_end_matches('\r'))?;
    let record_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("record missing \"type\" field: {line}"))?;
    match record_type {
        "response" => Ok(RpcRecord::Response(serde_json::from_value(value)?)),
        "extension_ui_request" => Ok(RpcRecord::ExtensionUiRequest(serde_json::from_value(
            value,
        )?)),
        _ => match serde_json::from_value::<Event>(value.clone()) {
            Ok(event) => Ok(RpcRecord::Event(event)),
            Err(_err) if Event::is_unknown(&value) => Ok(RpcRecord::Unknown {
                record_type: record_type.to_string(),
                raw: value,
            }),
            Err(err) => Err(anyhow::anyhow!(
                "failed to parse {record_type} record: {err}"
            )),
        },
    }
}

/// Response to a command.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    /// Correlation id, present when the command carried one.
    #[serde(default)]
    pub id: Option<String>,
    /// Always `"response"`.
    #[serde(rename = "type")]
    pub record_type: String,
    /// Name of the command being answered (or `"parse"` for malformed input).
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub data: Option<Value>,
}

impl Response {
    /// Interpret `data` as a typed payload. `Ok(None)` when there is no data.
    pub fn data_as<T: serde::de::DeserializeOwned>(&self) -> anyhow::Result<Option<T>> {
        match &self.data {
            Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
            None => Ok(None),
        }
    }
}

/// Known event type tags, for the unknown-record fallback in `parse_record`.
const KNOWN_EVENT_TYPES: &[&str] = &[
    "agent_start",
    "agent_end",
    "agent_settled",
    "turn_start",
    "turn_end",
    "message_start",
    "message_update",
    "message_end",
    "bash_execution_update",
    "tool_execution_start",
    "tool_execution_update",
    "tool_execution_end",
    "queue_update",
    "compaction_start",
    "compaction_end",
    "auto_retry_start",
    "auto_retry_end",
    "summarization_retry_scheduled",
    "summarization_retry_attempt_start",
    "summarization_retry_finished",
    "extension_error",
];

/// A session event streamed from Pi.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AgentStart,
    /// One low-level run completed; retries or queued work may still follow.
    #[serde(rename_all = "camelCase")]
    AgentEnd {
        #[serde(default)]
        messages: Vec<AgentMessage>,
        #[serde(default)]
        will_retry: bool,
    },
    /// Fully settled: no automatic retry, compaction retry, or queued
    /// continuation remains. This is the completion signal for a prompt.
    AgentSettled {
        #[serde(default)]
        aborted: bool,
    },
    TurnStart,
    #[serde(rename_all = "camelCase")]
    TurnEnd {
        #[serde(default)]
        message: Option<AgentMessage>,
        #[serde(default)]
        tool_results: Vec<AgentMessage>,
    },
    #[serde(rename_all = "camelCase")]
    MessageStart {
        #[serde(default)]
        message: Option<AgentMessage>,
    },
    /// Streaming delta; buffer by `contentIndex` and treat `message_end` as
    /// authoritative.
    #[serde(rename_all = "camelCase")]
    MessageUpdate {
        #[serde(default)]
        usage: Option<Usage>,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    /// Output chunk from a direct `bash` command; `id` echoes the command id.
    BashExecutionUpdate {
        #[serde(default)]
        id: Option<String>,
        delta: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        args: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        args: Option<Value>,
        #[serde(default)]
        partial_result: Option<ToolResultContent>,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        result: Option<ToolResultContent>,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(rename_all = "camelCase")]
    QueueUpdate {
        #[serde(default)]
        steering: Vec<String>,
        #[serde(default)]
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    #[serde(rename_all = "camelCase")]
    CompactionEnd {
        reason: CompactionReason,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        aborted: bool,
        #[serde(default)]
        will_retry: bool,
        #[serde(default)]
        error_message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        #[serde(default)]
        error_message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        #[serde(default)]
        final_error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SummarizationRetryScheduled {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        #[serde(default)]
        error_message: Option<String>,
    },
    SummarizationRetryAttemptStart {
        source: String,
        #[serde(default)]
        reason: Option<String>,
    },
    SummarizationRetryFinished,
    #[serde(rename_all = "camelCase")]
    ExtensionError {
        #[serde(default)]
        extension_path: Option<String>,
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },
}

impl Event {
    /// True when the JSON `type` is not a known event tag.
    fn is_unknown(value: &Value) -> bool {
        !KNOWN_EVENT_TYPES.contains(
            &value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
    }
}

/// Why compaction ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CompactionReason {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "threshold")]
    Threshold,
    #[serde(rename = "overflow")]
    Overflow,
}

// ---------------------------------------------------------------------------
// Streaming deltas
// ---------------------------------------------------------------------------

/// One streaming delta inside [`Event::MessageUpdate`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    #[serde(rename_all = "camelCase")]
    TextStart { content_index: u32 },
    #[serde(rename_all = "camelCase")]
    TextDelta { content_index: u32, delta: String },
    /// Authoritative text for the block; replaces buffered deltas.
    #[serde(rename_all = "camelCase")]
    TextEnd { content_index: u32, content: String },
    #[serde(rename_all = "camelCase")]
    ThinkingStart { content_index: u32 },
    #[serde(rename_all = "camelCase")]
    ThinkingDelta { content_index: u32, delta: String },
    #[serde(rename_all = "camelCase")]
    ThinkingEnd { content_index: u32 },
    #[serde(rename_all = "camelCase")]
    ToolCallStart {
        content_index: u32,
        id: String,
        tool_name: String,
    },
    /// Chunk of tool-call arguments; buffer and concatenate.
    #[serde(rename_all = "camelCase")]
    ToolCallDelta { content_index: u32, delta: String },
    #[serde(rename_all = "camelCase")]
    ToolCallEnd {
        content_index: u32,
        tool_call: ToolCall,
    },
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// A message in the conversation, discriminated by `role`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    BashExecution(BashExecutionMessage),
    /// A system-prompt snapshot (preamble/tools sections). Sessions record
    /// one; the UI skips it, but it must parse or the whole history fails.
    System(SystemMessage),
    /// A role this crate does not know. Forward compatibility: an unknown
    /// role in restored history must not fail the entire entries payload.
    #[serde(other)]
    Unknown,
}

impl AgentMessage {
    /// The wire name of this message's role, for diagnostics.
    pub fn role_name(&self) -> &'static str {
        match self {
            AgentMessage::User(_) => "user",
            AgentMessage::Assistant(_) => "assistant",
            AgentMessage::ToolResult(_) => "toolResult",
            AgentMessage::BashExecution(_) => "bashExecution",
            AgentMessage::System(_) => "system",
            AgentMessage::Unknown => "unknown",
        }
    }

    pub fn timestamp(&self) -> Option<f64> {
        match self {
            AgentMessage::User(m) => Some(m.timestamp),
            AgentMessage::Assistant(m) => Some(m.timestamp),
            AgentMessage::ToolResult(m) => Some(m.timestamp),
            AgentMessage::BashExecution(m) => Some(m.timestamp),
            AgentMessage::System(m) => Some(m.timestamp),
            AgentMessage::Unknown => None,
        }
    }
}

/// `role: "system"` message: the prompt snapshot pi stores per session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub content: UserContent,
    /// Preamble/tools sections; kept opaque.
    #[serde(default)]
    pub sections: Option<Value>,
    #[serde(default)]
    pub timestamp: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: UserContent,
    #[serde(default)]
    pub timestamp: f64,
    #[serde(default)]
    pub attachments: Option<Vec<Value>>,
}

impl UserMessage {
    /// Plain-text view of the content (concatenated text blocks).
    pub fn text(&self) -> String {
        match &self.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    UserContentBlock::Text { text } => Some(text.as_str()),
                    UserContentBlock::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<UserContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContentBlock {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        data: String,
        mime_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantContent>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub timestamp: f64,
}

/// Content blocks of an assistant message, tagged by `type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub content: Option<ToolResultContent>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub timestamp: f64,
}

impl ToolResultMessage {
    /// Plain-text view of the result content.
    pub fn text(&self) -> String {
        self.content.as_ref().map(|c| c.text()).unwrap_or_default()
    }
}

/// Tool result payload: a list of content blocks plus opaque details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultContent {
    #[serde(default)]
    pub content: Vec<ToolResultBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ToolResultContent {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ToolResultBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultBlock {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        data: String,
        mime_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub full_output_path: Option<String>,
    #[serde(default)]
    pub timestamp: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "length")]
    Length,
    #[serde(rename = "toolUse")]
    ToolUse,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "aborted")]
    Aborted,
}

// ---------------------------------------------------------------------------
// Usage / models
// ---------------------------------------------------------------------------

/// Token usage reported by the provider; cumulative during streaming.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub cost: Option<Cost>,
}

/// Monetary cost in USD per million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub total: Option<f64>,
}

/// A model definition, as returned by `get_state` / `get_available_models`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub cost: Option<Cost>,
}

// ---------------------------------------------------------------------------
// Extension UI
// ---------------------------------------------------------------------------

/// An extension UI request: dialogs need a response, the rest are
/// fire-and-forget.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionUiRequest {
    pub id: String,
    pub method: ExtensionUiMethod,
    #[serde(default)]
    pub title: Option<String>,
    /// Dialog methods: auto-resolves agent-side when the timeout expires.
    #[serde(default)]
    pub timeout: Option<u64>,
    // Method-specific payloads.
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub prefill: Option<String>,
    #[serde(default)]
    pub notify_type: Option<String>,
    #[serde(default)]
    pub status_key: Option<String>,
    #[serde(default)]
    pub status_text: Option<String>,
    #[serde(default)]
    pub widget_key: Option<String>,
    #[serde(default)]
    pub widget_lines: Option<Vec<String>>,
    #[serde(default)]
    pub widget_placement: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ExtensionUiMethod {
    #[serde(rename = "select")]
    Select,
    #[serde(rename = "confirm")]
    Confirm,
    #[serde(rename = "input")]
    Input,
    #[serde(rename = "editor")]
    Editor,
    #[serde(rename = "notify")]
    Notify,
    #[serde(rename = "setStatus")]
    SetStatus,
    #[serde(rename = "setWidget")]
    SetWidget,
    #[serde(rename = "setTitle")]
    SetTitle,
    #[serde(rename = "set_editor_text")]
    SetEditorText,
}

impl ExtensionUiMethod {
    /// Dialog methods block the extension until answered (or timed out).
    pub fn is_dialog(self) -> bool {
        matches!(
            self,
            ExtensionUiMethod::Select
                | ExtensionUiMethod::Confirm
                | ExtensionUiMethod::Input
                | ExtensionUiMethod::Editor
        )
    }
}

// ---------------------------------------------------------------------------
// Typed response payloads
// ---------------------------------------------------------------------------

/// `get_state` payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    #[serde(default)]
    pub model: Option<Model>,
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,
    #[serde(default)]
    pub is_streaming: bool,
    #[serde(default)]
    pub is_compacting: bool,
    #[serde(default)]
    pub steering_mode: Option<QueueMode>,
    #[serde(default)]
    pub follow_up_mode: Option<QueueMode>,
    #[serde(default)]
    pub session_file: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub auto_compaction_enabled: bool,
    #[serde(default)]
    pub message_count: u64,
    #[serde(default)]
    pub pending_message_count: u64,
}

/// `get_available_models` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AvailableModels {
    #[serde(default)]
    pub models: Vec<Model>,
}

/// `get_available_thinking_levels` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AvailableThinkingLevels {
    #[serde(default)]
    pub levels: Vec<ThinkingLevel>,
}

/// `get_entries` payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntries {
    #[serde(default)]
    pub entries: Vec<SessionEntry>,
    pub leaf_id: Option<String>,
}

/// One entry in the session tree.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub message: Option<AgentMessage>,
}

/// `get_session_stats` payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    #[serde(default)]
    pub session_file: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub user_messages: u64,
    #[serde(default)]
    pub assistant_messages: u64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub tool_results: u64,
    #[serde(default)]
    pub total_messages: u64,
    #[serde(default)]
    pub tokens: Option<Usage>,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub context_usage: Option<ContextUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub percent: Option<f64>,
}

/// `get_commands` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AvailableCommands {
    #[serde(default)]
    pub commands: Vec<CommandInfo>,
}

/// One invocable command: an extension command, a prompt template or a skill.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    /// Invoked by sending `/name`.
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// `extension`, `prompt` or `skill`.
    #[serde(default)]
    pub source: Option<String>,
    /// `user`, `project` or `path`, for non-extension commands.
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// `get_messages` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct Messages {
    #[serde(default)]
    pub messages: Vec<AgentMessage>,
}

/// `clear_queue` payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearedQueue {
    #[serde(default)]
    pub steering: Vec<String>,
    #[serde(default)]
    pub follow_up: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_prompt_command() {
        let request = Request::new(Command::Prompt {
            message: "Hello".into(),
            images: None,
            streaming_behavior: None,
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["type"], "prompt");
        assert_eq!(json["message"], "Hello");
        assert!(json.get("images").is_none());
        assert!(json.get("streamingBehavior").is_none());
        assert!(json["id"].is_string());
    }

    #[test]
    fn serialize_prompt_with_streaming_behavior() {
        let request = Request::anonymous(Command::Prompt {
            message: "New instruction".into(),
            images: None,
            streaming_behavior: Some(StreamingBehavior::Steer),
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["streamingBehavior"], "steer");
        assert!(json.get("id").is_none());
    }

    #[test]
    fn serialize_misc_commands() {
        let json = serde_json::to_value(Request::anonymous(Command::SetModel {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4".into(),
        }))
        .unwrap();
        assert_eq!(json["type"], "set_model");
        assert_eq!(json["modelId"], "claude-sonnet-4");

        let json = serde_json::to_value(Request::anonymous(Command::SwitchSession {
            session_path: "C:/s.jsonl".into(),
        }))
        .unwrap();
        assert_eq!(json["type"], "switch_session");
        assert_eq!(json["sessionPath"], "C:/s.jsonl");

        let json = serde_json::to_value(Request::anonymous(Command::GetEntries {
            since: Some("abc".into()),
        }))
        .unwrap();
        assert_eq!(json["type"], "get_entries");
        assert_eq!(json["since"], "abc");
    }

    #[test]
    fn parse_response_record() {
        let record = parse_record(
            r#"{"id":"req-1","type":"response","command":"get_state","success":true,"data":{"isStreaming":false}}"#,
        )
        .unwrap();
        match record {
            RpcRecord::Response(response) => {
                assert_eq!(response.id.as_deref(), Some("req-1"));
                assert_eq!(response.command, "get_state");
                assert!(response.success);
                let state = response.data_as::<SessionState>().unwrap().unwrap();
                assert!(!state.is_streaming);
            }
            other => panic!("expected response, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_response() {
        let record = parse_record(
            r#"{"type":"response","command":"set_model","success":false,"error":"Model not found: invalid/model"}"#,
        )
        .unwrap();
        match record {
            RpcRecord::Response(response) => {
                assert!(!response.success);
                assert_eq!(
                    response.error.as_deref(),
                    Some("Model not found: invalid/model")
                );
            }
            other => panic!("expected response, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_update_delta() {
        let record = parse_record(
            r#"{"type":"message_update","usage":{"input":100,"output":1},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello "}}"#,
        )
        .unwrap();
        match record {
            RpcRecord::Event(Event::MessageUpdate {
                usage,
                assistant_message_event,
            }) => {
                assert_eq!(usage.unwrap().input, 100);
                assert!(matches!(
                    assistant_message_event,
                    AssistantMessageEvent::TextDelta { content_index: 0, ref delta } if delta == "Hello "
                ));
            }
            other => panic!("expected message_update, got {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_message_end() {
        let record = parse_record(
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}],"timestamp":1733234567890}}"#,
        )
        .unwrap();
        match record {
            RpcRecord::Event(Event::MessageEnd { message }) => match message {
                AgentMessage::Assistant(assistant) => {
                    assert_eq!(assistant.content.len(), 1);
                    assert!(matches!(
                        &assistant.content[0],
                        AssistantContent::Text { text } if text == "Hi"
                    ));
                }
                other => panic!("expected assistant message, got {other:?}"),
            },
            other => panic!("expected message_end, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_execution_events() {
        let start = parse_record(
            r#"{"type":"tool_execution_start","toolCallId":"call_abc123","toolName":"bash","args":{"command":"ls -la"}}"#,
        )
        .unwrap();
        match start {
            RpcRecord::Event(Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            }) => {
                assert_eq!(tool_call_id, "call_abc123");
                assert_eq!(tool_name, "bash");
                assert_eq!(args.unwrap()["command"], "ls -la");
            }
            other => panic!("expected tool_execution_start, got {other:?}"),
        }

        let end = parse_record(
            r#"{"type":"tool_execution_end","toolCallId":"call_abc123","toolName":"bash","result":{"content":[{"type":"text","text":"total 48\n"}]},"isError":false}"#,
        )
        .unwrap();
        match end {
            RpcRecord::Event(Event::ToolExecutionEnd {
                result, is_error, ..
            }) => {
                assert!(!is_error);
                assert_eq!(result.unwrap().text(), "total 48\n");
            }
            other => panic!("expected tool_execution_end, got {other:?}"),
        }
    }

    #[test]
    fn parse_extension_ui_request() {
        let record = parse_record(
            r#"{"type":"extension_ui_request","id":"uuid-1","method":"select","title":"Allow?","options":["Allow","Block"],"timeout":10000}"#,
        )
        .unwrap();
        match record {
            RpcRecord::ExtensionUiRequest(request) => {
                assert_eq!(request.method, ExtensionUiMethod::Select);
                assert!(request.method.is_dialog());
                assert_eq!(request.options.unwrap(), vec!["Allow", "Block"]);
            }
            other => panic!("expected extension_ui_request, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_is_preserved() {
        let record = parse_record(r#"{"type":"shiny_new_event","payload":{"a":1}}"#).unwrap();
        match record {
            RpcRecord::Unknown { record_type, raw } => {
                assert_eq!(record_type, "shiny_new_event");
                assert_eq!(raw["payload"]["a"], 1);
            }
            other => panic!("expected unknown record, got {other:?}"),
        }
    }

    #[test]
    fn malformed_line_is_an_error() {
        assert!(parse_record("not json").is_err());
        assert!(parse_record(r#"{"no_type":true}"#).is_err());
    }

    #[test]
    fn parse_user_message_text_or_blocks() {
        let text: UserMessage =
            serde_json::from_str(r#"{"role":"user","content":"Hello!","timestamp":1}"#).unwrap();
        assert_eq!(text.text(), "Hello!");

        let blocks: UserMessage = serde_json::from_str(
            r#"{"role":"user","content":[{"type":"text","text":"See "},{"type":"image","data":"abc","mimeType":"image/png"}]}"#,
        )
        .unwrap();
        assert_eq!(blocks.text(), "See ");
    }

    #[test]
    fn parse_assistant_tool_call_block() {
        let message: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","content":[{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}],"stopReason":"toolUse"}"#,
        )
        .unwrap();
        assert_eq!(message.stop_reason, Some(StopReason::ToolUse));
        assert!(matches!(
            &message.content[0],
            AssistantContent::ToolCall(call) if call.name == "bash"
        ));
    }

    #[test]
    fn session_state_parses() {
        let state: SessionState = serde_json::from_str(
            r#"{"model":{"id":"m","name":"M","reasoning":true},"thinkingLevel":"medium","isStreaming":false,"steeringMode":"one-at-a-time","messageCount":5}"#,
        )
        .unwrap();
        assert_eq!(state.thinking_level, Some(ThinkingLevel::Medium));
        assert_eq!(state.steering_mode, Some(QueueMode::OneAtATime));
        assert_eq!(state.message_count, 5);
        assert_eq!(state.model.as_ref().unwrap().name, "M");
    }

    #[test]
    fn session_entries_parses() {
        let entries: SessionEntries = serde_json::from_str(
            r#"{"entries":[{"type":"message","id":"def456","parentId":"abc123","timestamp":"...","message":{"role":"user","content":"hi"}}],"leafId":"def456"}"#,
        )
        .unwrap();
        assert_eq!(entries.leaf_id.as_deref(), Some("def456"));
        let entry = &entries.entries[0];
        assert_eq!(entry.entry_type, "message");
        assert_eq!(entry.parent_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn session_stats_parses() {
        let stats: SessionStats = serde_json::from_str(
            r#"{"sessionId":"abc","userMessages":5,"assistantMessages":5,"toolCalls":12,"totalMessages":22,"tokens":{"input":50000,"output":10000,"totalTokens":60000},"cost":0.45,"contextUsage":{"tokens":60000,"contextWindow":200000,"percent":30}}"#,
        )
        .unwrap();
        assert_eq!(stats.total_messages, 22);
        assert_eq!(stats.tokens.as_ref().unwrap().total_tokens, Some(60000));
        assert_eq!(stats.context_usage.as_ref().unwrap().percent, Some(30.0));
    }

    #[test]
    fn agent_message_handles_system_and_unknown_roles() {
        // Real sessions store a system-prompt snapshot per session; failing
        // to parse it once broke loading the whole history (user report).
        let message: AgentMessage = serde_json::from_str(
            r#"{"role":"system","content":"","sections":{"preamble":"You are..."},"timestamp":1}"#,
        )
        .unwrap();
        assert!(matches!(message, AgentMessage::System(_)));

        // Forward compatibility: a role this crate has never heard of must
        // not fail the surrounding payload.
        let message: AgentMessage =
            serde_json::from_str(r#"{"role":"brandNew","content":"hi"}"#).unwrap();
        assert!(matches!(message, AgentMessage::Unknown));
    }

    #[test]
    fn entries_with_system_message_parse() {
        let payload: SessionEntries = serde_json::from_str(
            r#"{"entries":[
                {"type":"message","id":"a","parentId":null,"timestamp":"...","message":{"role":"system","content":"","sections":{"preamble":"p"}}},
                {"type":"message","id":"b","parentId":"a","timestamp":"...","message":{"role":"user","content":"hi"}},
                {"type":"custom","id":"c","parentId":"b"}
            ],"leafId":"b"}"#,
        )
        .unwrap();
        assert_eq!(payload.entries.len(), 3);
        assert!(matches!(
            payload.entries[0].message,
            Some(AgentMessage::System(_))
        ));
    }
}
