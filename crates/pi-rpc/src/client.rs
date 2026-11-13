//! Spawn and drive a `pi --mode rpc` child process.
//!
//! Protocol rules implemented here (from Pi's `rpc.md`):
//! - Strict JSONL framing: records are delimited by `\n` only; a trailing
//!   `\r` is stripped. Never split on U+2028/U+2029.
//! - Pi's stdout must be drained continuously or the pipe buffer fills and Pi
//!   stalls, so a dedicated reader thread feeds an internal dispatcher.
//! - Responses are correlated by `id`, never by order: `request` registers a
//!   one-shot channel keyed by id and the dispatcher completes it.
//! - Graceful shutdown: close Pi's stdin and give it a moment before killing.
//!
//! Topology:
//!
//! ```text
//! pi stdout ──▶ reader thread ──▶ raw channel ──▶ dispatcher ──▶ subscribers
//!                                                     │              (events)
//! pi stdin  ◀── send()/request()                     └─▶ pending one-shots
//! ```

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::protocol::{Command, Request, Response, RpcRecord, parse_record};

/// Generates monotonically increasing request ids. Prefixed `pido-` so they
/// are recognizable in Pi-side logs.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_request_id() -> String {
    format!("pido-{}", REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Windows: hide the console window when a GUI app spawns these CLI helpers.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Resolve the actual pi executable to spawn.
///
/// On Windows, `std::process::Command` does not resolve `PATHEXT` and npm
/// installs pi as `pi.cmd`, so spawning bare `pi` fails. Search `PATH` for
/// the Windows executable forms; std routes `.cmd`/`.bat` through `cmd.exe`
/// with safe escaping. On Unix, `pi` on `PATH` works directly.
fn resolve_pi_program(program: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let is_bare_name = program.parent().is_none_or(|p| p.as_os_str().is_empty());
        if program.extension().is_none()
            && is_bare_name
            && let Some(path_var) = std::env::var_os("PATH")
        {
            for dir in std::env::split_paths(&path_var) {
                for name in ["pi.cmd", "pi.exe", "pi.bat"] {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = program;
    program.to_path_buf()
}

/// Default bound for `request`. `abort` and `bash` only respond once idle or
/// finished, so this is deliberately generous.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Configuration for the Pi child process.
#[derive(Debug, Clone)]
pub struct PiProcessConfig {
    /// Directory the agent works in. Pi runs in the process cwd (there is no
    /// `--cwd` flag), so this becomes the child's current directory.
    pub cwd: PathBuf,
    /// Extra CLI arguments, e.g. `["--provider", "deepseek"]` or
    /// `["--session", "<path>"]`.
    pub extra_args: Vec<String>,
    /// Override the `pi` executable path; defaults to searching `PATH`.
    pub pi_path: Option<PathBuf>,
}

impl PiProcessConfig {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            extra_args: Vec::new(),
            pi_path: None,
        }
    }

    pub fn with_args(mut self, extra_args: Vec<String>) -> Self {
        self.extra_args = extra_args;
        self
    }
}

/// Where Pi went and why, for UIs that need to explain a dead session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Pi exited on its own (crash, fatal error, external kill).
    Exited { code: Option<i32> },
    /// The client initiated shutdown by closing stdin.
    ClosedByClient,
    /// Reading Pi's stdout failed (pipe broken).
    ReadFailed(String),
}

/// What a running client broadcasts to subscribers.
///
/// The size skew between variants is fine: each event is sent through a
/// channel exactly once, so the occasional large `Record` costs less than a
/// box allocation per event.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ClientEvent {
    /// A record arrived from Pi's stdout.
    Record(RpcRecord),
    /// The process is gone; no further events will arrive.
    Disconnected(DisconnectReason),
}

type Pending = crossbeam_channel::Sender<Response>;

struct Inner {
    child: Mutex<Option<Child>>,
    /// Writer to Pi's stdin; `None` once shut down.
    stdin: Mutex<Option<ChildStdin>>,
    /// Waiting `request` calls keyed by their id.
    pending: Mutex<HashMap<String, Pending>>,
    /// Fan-out of everything except responses matched to a pending request.
    subscribers: Mutex<Vec<Sender<ClientEvent>>>,
    shutdown: Mutex<bool>,
    /// How many `PiClient` handles are alive. The I/O threads hold bare
    /// `Arc<Inner>` clones that do not count: only the last *handle* drop
    /// may close Pi's stdin.
    handle_count: AtomicU64,
    /// The last few stderr lines, for diagnostics when a spawn or request
    /// fails. Pi writes fatal startup errors here.
    stderr_tail: Mutex<VecDeque<String>>,
    /// Every record that came in from Pi, counted. A watchdog compares this
    /// across a failed probe to tell a wedged process from a busy one.
    inbound_records: AtomicU64,
}

/// Handle to a running Pi RPC process.
///
/// The handle is cheap to clone; dropping the last clone initiates a graceful
/// shutdown (Pi exits when its stdin closes). Dropping any earlier clone does
/// nothing — the process keeps serving the remaining handles.
pub struct PiClient {
    inner: Arc<Inner>,
}

impl Clone for PiClient {
    fn clone(&self) -> Self {
        self.inner.handle_count.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Drop for PiClient {
    fn drop(&mut self) {
        // Every handle's Drop runs, not just the last one's; only the final
        // handle may shut Pi down. (Rust's Drop is not last-Arc-aware.)
        if self.inner.handle_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shutdown();
        }
    }
}

impl PiClient {
    /// Spawn `pi --mode rpc` with the given configuration and start pumping
    /// its stdout.
    ///
    /// Errors immediately if the executable cannot be spawned or the working
    /// directory is missing. Everything after spawn (protocol errors,
    /// crashes) is reported through the event stream.
    pub fn spawn(config: &PiProcessConfig) -> anyhow::Result<Self> {
        if !config.cwd.exists() {
            anyhow::bail!("working directory does not exist: {}", config.cwd.display());
        }
        let program = resolve_pi_program(&config.pi_path.clone().unwrap_or_else(|| "pi".into()));
        let mut command = ProcessCommand::new(&program);
        command
            .args(["--mode", "rpc"])
            .args(&config.extra_args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|err| anyhow::anyhow!("failed to spawn pi ({}): {err}", program.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("pi stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("pi stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("pi stderr was not piped"))?;

        let inner = Arc::new(Inner {
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            pending: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
            shutdown: Mutex::new(false),
            handle_count: AtomicU64::new(1),
            inbound_records: AtomicU64::new(0),
            stderr_tail: Mutex::new(VecDeque::new()),
        });

        // stderr thread: Pi uses stderr for startup failures and provider
        // errors. Keep a short tail so connection failures can explain
        // themselves instead of surfacing only a timeout.
        {
            let inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("pi-rpc-stderr".into())
                .spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        let line = line.trim_end().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        log::warn!("pi stderr: {line}");
                        let mut tail = inner.stderr_tail.lock().unwrap();
                        if tail.len() >= 40 {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                })
                .expect("failed to spawn pi stderr thread");
        }

        // Reader thread: drains stdout continuously (backpressure would stall
        // Pi). Splits on `\n` only, per the spec, and tolerates a trailing
        // `\r`. IO errors are logged and the thread returns; the reaper owns
        // the single authoritative disconnect notification.
        {
            let (raw_tx, raw_rx) = unbounded::<ClientEvent>();
            std::thread::Builder::new()
                .name("pi-rpc-reader".into())
                .spawn(move || {
                    let reader = BufReader::new(stdout);
                    for line in reader.split(b'\n') {
                        match line {
                            Ok(bytes) => {
                                if bytes.is_empty() {
                                    continue;
                                }
                                let Ok(text) = String::from_utf8(bytes) else {
                                    log::warn!("pi emitted non-utf8 record, skipping");
                                    continue;
                                };
                                let text = text.strip_suffix('\r').unwrap_or(&text);
                                match parse_record(text) {
                                    Ok(record) => {
                                        if raw_tx.send(ClientEvent::Record(record)).is_err() {
                                            return;
                                        }
                                    }
                                    Err(err) => {
                                        log::error!("unparseable record from pi: {err}");
                                    }
                                }
                            }
                            Err(err) => {
                                log::error!("reading pi stdout failed: {err}");
                                return;
                            }
                        }
                    }
                })
                .expect("failed to spawn pi reader thread");

            // Dispatcher thread: the single consumer of the raw stream. It
            // completes pending `request` one-shots by id and fans everything
            // else out to subscribers. When the raw stream ends it unblocks
            // waiters; the reaper delivers the disconnect notification.
            let inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("pi-rpc-dispatch".into())
                .spawn(move || {
                    for event in raw_rx {
                        inner
                            .inbound_records
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        match &event {
                            ClientEvent::Record(RpcRecord::Response(response)) => {
                                let waiter = response
                                    .id
                                    .as_ref()
                                    .and_then(|id| inner.pending.lock().unwrap().remove(id));
                                match waiter {
                                    // Consumed by its requester; do not also
                                    // fan out (an unmatched response, such as
                                    // a `parse` error without id, still goes
                                    // to subscribers).
                                    Some(sender) => {
                                        let _ = sender.send(response.clone());
                                    }
                                    None => fan_out(&inner, event),
                                }
                            }
                            _ => fan_out(&inner, event),
                        }
                    }
                    // Raw stream ended: Pi's stdout is closed and the process
                    // is on its way out. Unblock any remaining waiters now;
                    // the reaper reports the final status.
                    fail_pending(&inner.pending);
                })
                .expect("failed to spawn pi dispatcher thread");
        }

        // Reaper thread: waits for the child and reports the disconnect
        // exactly once, with the real exit code.
        {
            let inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("pi-rpc-reaper".into())
                .spawn(move || {
                    // Poll instead of holding the child lock across `wait`:
                    // the shutdown path needs that lock to kill a wedged Pi.
                    let code = loop {
                        let status = {
                            let mut guard = inner.child.lock().unwrap();
                            match guard.as_mut() {
                                Some(child) => child.try_wait(),
                                None => break None,
                            }
                        };
                        match status {
                            Ok(Some(status)) => break status.code(),
                            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                            Err(error) => {
                                log::warn!("pi-rpc: waiting for pi failed: {error}");
                                break None;
                            }
                        }
                    };
                    let shutting_down = *inner.shutdown.lock().unwrap();
                    log::info!(
                        "pi-rpc: reaper fired, exit code {code:?}, shutdown flag {shutting_down}, strong refs {}",
                        Arc::strong_count(&inner)
                    );
                    let reason = if shutting_down {
                        DisconnectReason::ClosedByClient
                    } else {
                        DisconnectReason::Exited { code }
                    };
                    fail_pending(&inner.pending);
                    fan_out(&inner, ClientEvent::Disconnected(reason));
                })
                .expect("failed to spawn pi reaper thread");
        }

        Ok(Self { inner })
    }

    /// Subscribe to every event, extension UI record, unmatched response and
    /// the final disconnect. Responses matched to a `request` call are
    /// delivered to that call only.
    pub fn subscribe(&self) -> Receiver<ClientEvent> {
        let (tx, rx) = unbounded();
        self.inner.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Write a command to Pi's stdin without waiting for its response.
    /// Fails when Pi is already shut down.
    pub fn send(&self, command: Command) -> anyhow::Result<()> {
        let request = Request::new(command);
        self.write_request(&request)
    }

    /// Write a command with no envelope id. Required for
    /// `extension_ui_response`, whose own `id` field names the dialog being
    /// answered: an envelope id would serialize a duplicate `"id"` key.
    pub fn send_uncorrelated(&self, command: Command) -> anyhow::Result<()> {
        let request = Request::anonymous(command);
        self.write_request(&request)
    }

    fn write_request(&self, request: &Request) -> anyhow::Result<()> {
        let mut json = serde_json::to_string(request)?;
        json.push('\n');
        let mut guard = self.inner.stdin.lock().unwrap();
        match guard.as_mut() {
            Some(stdin) => stdin
                .write_all(json.as_bytes())
                .and_then(|_| stdin.flush())
                .map_err(|err| anyhow::anyhow!("failed to write to pi stdin: {err}")),
            None => Err(anyhow::anyhow!("pi process is shut down")),
        }
    }

    /// Send a command and wait for the response with the matching id.
    ///
    /// While waiting, every other record keeps flowing to subscribers — only
    /// the matched response is consumed here. Note that `abort` and `bash`
    /// respond only once idle/finished, so the default timeout is generous.
    pub fn request(&self, command: Command) -> anyhow::Result<Response> {
        self.request_with_timeout(command, DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn request_with_timeout(
        &self,
        command: Command,
        timeout: Duration,
    ) -> anyhow::Result<Response> {
        let request = Request::new(command);
        let id = request
            .id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("request must carry an id"))?;
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.inner.pending.lock().unwrap().insert(id.clone(), tx);
        if let Err(err) = self.write_request(&request) {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(err);
        }
        let deadline = Instant::now() + timeout;
        let received = rx.recv_deadline(deadline);
        self.inner.pending.lock().unwrap().remove(&id);
        match received {
            Ok(response) => Ok(response),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                Err(anyhow::anyhow!("timed out waiting for response to {id}"))
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                Err(anyhow::anyhow!("pi disconnected before answering {id}"))
            }
        }
    }

    /// Graceful shutdown: closing stdin tells Pi to exit cleanly. The
    /// disconnect is reported to subscribers.
    pub fn shutdown(&self) {
        let mut shutdown = self.inner.shutdown.lock().unwrap();
        if *shutdown {
            return;
        }
        log::info!(
            "pi-rpc: shutdown initiated (strong refs {}), closing stdin",
            Arc::strong_count(&self.inner)
        );
        *shutdown = true;
        // Dropping the stdin handle closes the pipe.
        *self.inner.stdin.lock().unwrap() = None;

        // Graceful shutdown is the documented path, but a wedged Pi must not
        // outlive the app. Give it three seconds, then kill it.
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("pi-rpc-kill".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                let mut guard = inner.child.lock().unwrap();
                if let Some(child) = guard.as_mut()
                    && matches!(child.try_wait(), Ok(None))
                {
                    log::warn!("pi-rpc: pi did not exit after stdin closed; killing");
                    let _ = child.kill();
                }
            })
            .ok();
    }

    /// Whether shutdown has been initiated.
    pub fn is_shut_down(&self) -> bool {
        *self.inner.shutdown.lock().unwrap()
    }

    /// How many records Pi has sent so far. A watchdog snapshots this around
    /// a failed probe: new records mean the process is alive, just busy.
    pub fn records_seen(&self) -> u64 {
        self.inner.inbound_records.load(Ordering::Relaxed)
    }

    /// Kill the process right away. A wedged Pi will not read its stdin, so
    /// graceful shutdown would hang; killing it surfaces the disconnect to
    /// subscribers, which is what a watchdog wants.
    pub fn kill(&self) {
        *self.inner.shutdown.lock().unwrap() = true;
        *self.inner.stdin.lock().unwrap() = None;
        let mut guard = self.inner.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            log::warn!("pi-rpc: killing the unresponsive process");
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The last stderr lines Pi wrote, newest last. Useful when a request or
    /// startup fails and the protocol response alone does not explain why.
    pub fn last_stderr(&self) -> String {
        self.inner
            .stderr_tail
            .lock()
            .map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}

/// Forward an event to every live subscriber; dead receivers are dropped.
fn fan_out(inner: &Inner, event: ClientEvent) {
    let mut subscribers = inner.subscribers.lock().unwrap();
    subscribers.retain(|tx| tx.send(event.clone()).is_ok());
}

/// Unblock every waiting `request` with an error, e.g. on disconnect.
///
/// Dropping the one-shot senders makes the waiters see a disconnected channel.
fn fail_pending(pending: &Mutex<HashMap<String, Pending>>) {
    pending.lock().unwrap().clear();
}

/// Probe whether a `pi` executable is available and report its version, for
/// onboarding UIs.
pub fn probe_pi_install(pi_path: Option<&PathBuf>) -> Option<String> {
    let program = resolve_pi_program(&pi_path.cloned().unwrap_or_else(|| "pi".into()));
    let mut command = ProcessCommand::new(&program);
    command.arg("--version").stdin(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command.output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Check whether pi is authenticated for a provider, via `pi auth check`.
///
/// `Ok(true)` ready, `Ok(false)` not ready, `Err` when the command itself
/// could not run. Exit codes: 0 ready, 1 not_ready, 2 error.
pub fn probe_auth(provider: &str, pi_path: Option<&PathBuf>) -> anyhow::Result<bool> {
    let program = resolve_pi_program(&pi_path.cloned().unwrap_or_else(|| "pi".into()));
    let mut command = ProcessCommand::new(&program);
    command
        .args(["auth", "check", "--provider", provider])
        .stdin(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command
        .output()
        .map_err(|err| anyhow::anyhow!("failed to run pi auth check: {err}"))?;
    Ok(output.status.code() == Some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_increase() {
        let a = next_request_id();
        let b = next_request_id();
        assert_ne!(a, b);
    }

    #[test]
    fn config_defaults() {
        let config = PiProcessConfig::new("/tmp/project");
        assert_eq!(config.cwd, PathBuf::from("/tmp/project"));
        assert!(config.extra_args.is_empty());
        assert!(config.pi_path.is_none());
    }

    #[test]
    fn probe_pi_install_finds_local_pi() {
        // This machine has pi installed (it is a project prerequisite), so a
        // successful probe here validates the plumbing end to end.
        if which_pi().is_some() {
            assert!(probe_pi_install(None).is_some());
        }
    }

    fn which_pi() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(if cfg!(windows) { "pi.cmd" } else { "pi" }))
                .find(|path| path.exists())
        })
    }
}
