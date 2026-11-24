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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::io_util::read_line_capped;
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

/// Overrides where `pi` lives. A GUI launch (a `.desktop` entry or an
/// AppImage on Linux) inherits the session's `PATH`, not the shell's, so the
/// directory an installer appended to `~/.bashrc` is invisible to it.
pub const PI_PATH_ENV: &str = "PIDO_PI_PATH";

/// User directories that hold `pi` but are rarely on a desktop session's
/// `PATH`: the pi installer's own bin, and the usual per-user package
/// managers. Searched only after `PATH`.
#[cfg(not(windows))]
const UNIX_PI_DIRS: &[&str] = &[
    ".pi/agent/bin",
    ".pi/bin",
    ".local/bin",
    ".npm-global/bin",
    ".nix-profile/bin",
    ".bun/bin",
    ".volta/bin",
    ".asdf/shims",
];

/// System directories worth a direct look when `PATH` came up empty.
#[cfg(not(windows))]
const UNIX_PI_ROOTS: &[&str] = &[
    "/usr/local/bin",
    "/usr/bin",
    "/snap/bin",
    "/opt/homebrew/bin",
    "/home/linuxbrew/.linuxbrew/bin",
    "/nix/var/nix/profiles/default/bin",
];

/// Resolve the actual pi executable to spawn.
///
/// A bare `pi` is searched for explicitly instead of being handed to
/// `Command::new`: Windows needs the `pi.cmd` npm shim (std does no `PATHEXT`
/// lookup), and on Unix a desktop-launched app has a `PATH` that misses the
/// directories installers write to, so the well-known ones are tried after
/// `PATH`. An explicit path — or [`PI_PATH_ENV`] — always wins.
fn resolve_pi_program(program: &Path) -> PathBuf {
    let is_bare_name = program.parent().is_none_or(|p| p.as_os_str().is_empty());
    if !is_bare_name || program.extension().is_some() {
        return program.to_path_buf();
    }
    if let Some(raw) = std::env::var_os(PI_PATH_ENV) {
        let candidate = PathBuf::from(raw);
        if is_runnable(&candidate) {
            return candidate;
        }
        // Set but unusable: say so, then fall through rather than failing the
        // whole connection on a typo.
        log::warn!(
            "pi-rpc: {PI_PATH_ENV}={} is not an executable file; searching PATH instead",
            candidate.display()
        );
    }
    #[cfg(windows)]
    if let Some(found) = find_in_dirs(path_dirs(), ["pi.cmd", "pi.exe", "pi.bat"]) {
        return found;
    }
    #[cfg(not(windows))]
    {
        if let Some(found) = find_in_dirs(path_dirs(), ["pi"]) {
            return found;
        }
        if let Some(found) = find_in_unix_fallbacks() {
            log::info!("pi-rpc: pi is not on PATH; using {}", found.display());
            return found;
        }
    }
    program.to_path_buf()
}

/// The directories on `PATH`, in order.
fn path_dirs() -> impl Iterator<Item = PathBuf> {
    std::env::var_os("PATH")
        .map(|path_var| std::env::split_paths(&path_var).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

/// The first runnable file called any of `names` in `dirs`.
fn find_in_dirs<I, N>(dirs: I, names: N) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
    N: IntoIterator<Item = &'static str>,
{
    let names = names.into_iter().collect::<Vec<_>>();
    for dir in dirs {
        for name in &names {
            let candidate = dir.join(name);
            if is_runnable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether `path` is a file this process may execute. On Unix a missing
/// execute bit is the usual reason an installed pi "is not there".
fn is_runnable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && has_execute_bit(&metadata)
}

#[cfg(unix)]
fn has_execute_bit(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_execute_bit(_metadata: &std::fs::Metadata) -> bool {
    true
}

/// The per-user and system directories an installer is likely to have used.
#[cfg(not(windows))]
fn find_in_unix_fallbacks() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let user_dirs = UNIX_PI_DIRS.iter().map(|dir| home.join(dir));
    if let Some(found) = find_in_dirs(user_dirs, ["pi"]) {
        return Some(found);
    }
    if let Some(found) = find_in_node_managers(&home) {
        return Some(found);
    }
    let system_dirs = UNIX_PI_ROOTS.iter().map(PathBuf::from);
    find_in_dirs(system_dirs, ["pi"])
}

/// Where version-managed Node installs keep their global binaries. `nvm` and
/// `fnm` put each release in its own directory, so `npm -g` under either one
/// leaves `pi` somewhere a desktop launch never looks.
#[cfg(not(windows))]
const NODE_MANAGER_ROOTS: &[&str] = &[
    ".nvm/versions/node",
    ".local/share/fnm/node-versions",
    ".fnm/node-versions",
];

/// The `pi` of the newest Node release under `home`, if any is installed.
#[cfg(not(windows))]
fn find_in_node_managers(home: &Path) -> Option<PathBuf> {
    let mut found: Vec<((u64, u64, u64), PathBuf)> = Vec::new();
    for root in NODE_MANAGER_ROOTS {
        let Ok(entries) = std::fs::read_dir(home.join(root)) else {
            continue;
        };
        for entry in entries.flatten() {
            let version = version_key(&entry.file_name().to_string_lossy());
            for suffix in ["bin", "installation/bin"] {
                let candidate = entry.path().join(suffix).join("pi");
                if is_runnable(&candidate) {
                    found.push((version, candidate));
                }
            }
        }
    }
    found.sort_by_key(|left| std::cmp::Reverse(left.0));
    found.into_iter().next().map(|(_, path)| path)
}

/// `v22.19.0` → `(22, 19, 0)`; anything unparseable sorts as zero.
#[cfg(not(windows))]
fn version_key(name: &str) -> (u64, u64, u64) {
    let mut parts = name.trim_start_matches('v').split('.');
    let major = parts.next().and_then(|part| part.parse::<u64>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u64>().ok());
    let patch = parts.next().and_then(|part| part.parse::<u64>().ok());
    (major.unwrap_or(0), minor.unwrap_or(0), patch.unwrap_or(0))
}

/// A `pi` command that can find its own dependencies.
///
/// A `pi` resolved outside `PATH` (an nvm, volta or asdf shim, most often) is
/// usually launched through `#!/usr/bin/env node`, and that `node` sits in the
/// same directory. Prepending it lets the child run without the user having to
/// rebuild their desktop session's environment.
fn pi_command(program: &Path) -> ProcessCommand {
    let mut command = ProcessCommand::new(program);
    let Some(dir) = program
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter(|dir| !path_dirs().any(|path_dir| path_dir == *dir))
    else {
        return command;
    };
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(path_dirs());
    if let Ok(path) = std::env::join_paths(dirs) {
        command.env("PATH", path);
    }
    command
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
    /// The client killed an unresponsive process.
    Killed,
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
    /// Set by [`PiClient::kill`] so the reaper can distinguish a deliberate
    /// kill from a graceful stdin close.
    killed: AtomicBool,
    /// The final disconnect reason, replayed to subscribers that attach after
    /// the reaper has already fired.
    disconnected: Mutex<Option<DisconnectReason>>,
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
        let mut command = pi_command(&program);
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
            killed: AtomicBool::new(false),
            disconnected: Mutex::new(None),
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
                    const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
                    let mut reader = BufReader::new(stdout);
                    let mut buf = Vec::new();
                    loop {
                        let len = match read_line_capped(&mut reader, &mut buf, MAX_RECORD_BYTES) {
                            Ok(0) => break,
                            Ok(len) => len,
                            Err(err) => {
                                log::error!("reading pi stdout failed: {err}");
                                return;
                            }
                        };
                        if len > MAX_RECORD_BYTES {
                            log::error!(
                                "pi record of {len} bytes exceeds the {MAX_RECORD_BYTES}-byte cap; skipping"
                            );
                            continue;
                        }
                        let Ok(text) = std::str::from_utf8(&buf) else {
                            log::warn!("pi emitted non-utf8 record, skipping");
                            continue;
                        };
                        let text = text.trim_end_matches(['\r', '\n']);
                        if text.is_empty() {
                            continue;
                        }
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
                                        // The waiter may have timed out between
                                        // removing its entry and this send. In
                                        // that case hand the payload to the
                                        // subscribers instead of dropping it.
                                        if let Err(error) = sender.send(response.clone()) {
                                            fan_out(
                                                &inner,
                                                ClientEvent::Record(RpcRecord::Response(
                                                    error.into_inner(),
                                                )),
                                            );
                                        }
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
                    let mut delay_ms = 20u64;
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
                            Ok(None) => {
                                std::thread::sleep(Duration::from_millis(delay_ms));
                                delay_ms = (delay_ms * 2).min(250);
                            }
                            Err(error) => {
                                log::warn!("pi-rpc: waiting for pi failed: {error}");
                                break None;
                            }
                        }
                    };
                    let shutting_down = *inner.shutdown.lock().unwrap();
                    let killed = inner.killed.load(Ordering::Relaxed);
                    log::info!(
                        "pi-rpc: reaper fired, exit code {code:?}, shutdown flag {shutting_down}, killed {killed}, strong refs {}",
                        Arc::strong_count(&inner)
                    );
                    let reason = if killed {
                        DisconnectReason::Killed
                    } else if shutting_down {
                        DisconnectReason::ClosedByClient
                    } else {
                        DisconnectReason::Exited { code }
                    };
                    fail_pending(&inner.pending);
                    *inner.disconnected.lock().unwrap() = Some(reason.clone());
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
        // A subscriber that arrives after the process is gone must still see
        // the disconnect, or its pump would wait forever on an empty channel.
        if let Some(reason) = self.inner.disconnected.lock().unwrap().clone() {
            let _ = tx.send(ClientEvent::Disconnected(reason));
        }
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
        self.request_envelope(Request::new(command), timeout)
    }

    /// Send a command under a caller-chosen id. `bash` output events echo this
    /// id, so a UI can stream a shell command's output into its row.
    pub fn request_with_id(&self, id: String, command: Command) -> anyhow::Result<Response> {
        self.request_envelope(
            Request {
                id: Some(id),
                command,
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
    }

    fn request_envelope(&self, request: Request, timeout: Duration) -> anyhow::Result<Response> {
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
        {
            let mut shutdown = self.inner.shutdown.lock().unwrap();
            if *shutdown {
                return;
            }
            log::info!(
                "pi-rpc: shutdown initiated (strong refs {}), closing stdin",
                Arc::strong_count(&self.inner)
            );
            *shutdown = true;
        }

        // Never wait for the stdin lock here: a blocked `write_request` holds
        // it precisely when pi has stopped reading. Try once; the watchdog
        // below kills the process if the pipe cannot be closed.
        if let Ok(mut stdin) = self.inner.stdin.try_lock() {
            *stdin = None;
        }

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
        self.inner.killed.store(true, Ordering::Relaxed);
        *self.inner.shutdown.lock().unwrap() = true;
        // A blocked writer may hold the stdin lock. Do not wait; killing the
        // child breaks the pipe and unblocks it.
        if let Ok(mut stdin) = self.inner.stdin.try_lock() {
            *stdin = None;
        }
        let mut guard = self.inner.child.lock().unwrap();
        if let Some(child) = guard.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
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

/// What a probe of the `pi` CLI found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiProbe {
    /// The CLI ran and reported a version.
    Ready { program: PathBuf, version: String },
    /// No usable `pi` executable was found, on `PATH` or in an installer's
    /// directory.
    Missing,
    /// Something called `pi` exists, but running it failed: a wrong
    /// architecture, a missing execute bit, or — most often on Linux — a
    /// `node` the desktop session's `PATH` cannot see.
    Broken {
        program: PathBuf,
        /// How the process ended, as `ExitStatus` words it (exit code or
        /// signal).
        status: String,
        /// The first few stderr lines, which is where pi puts the reason.
        detail: String,
    },
}

/// `pi --version` output ("1.5.2", tolerating a `v` prefix or a trailing
/// tag like `-beta.1`) as a comparable tuple. `None` when the text carries
/// no leading number at all — callers must not warn on garbage.
pub fn pi_version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?
        .split('.');
    let parse = |part: Option<&str>| -> Option<u64> {
        let digits: String = part?
            .chars()
            .take_while(|char| char.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            None
        } else {
            digits.parse().ok()
        }
    };
    let major = parse(parts.next())?;
    let minor = parse(parts.next()).unwrap_or(0);
    let patch = parse(parts.next()).unwrap_or(0);
    Some((major, minor, patch))
}

/// pi's resolved `retry.enabled`, the same merge order pi itself uses: the
/// global settings file, then the project's on top. Absent or unreadable
/// means pi's default, which is on.
pub fn pi_retry_enabled(cwd: Option<&Path>) -> bool {
    let pick = |text: &str| {
        serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|settings| settings.get("retry")?.get("enabled")?.as_bool())
    };
    if let Some(global) = pi_agent_dir().map(|dir| dir.join("settings.json"))
        && let Ok(text) = std::fs::read_to_string(global)
        && let Some(enabled) = pick(&text)
    {
        return enabled;
    }
    if let Some(cwd) = cwd {
        let project = cwd.join(".pi").join("settings.json");
        if let Ok(text) = std::fs::read_to_string(project)
            && let Some(enabled) = pick(&text)
        {
            return enabled;
        }
    }
    true
}

/// The pi agent directory: `PI_CODING_AGENT_DIR` when set, else the default
/// `~/.pi/agent`. pi itself resolves the variable the same way.
pub fn pi_agent_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|home| home.join(".pi").join("agent"))
}

/// Providers with stored credentials in pi's `auth.json`, as sorted
/// top-level keys. Only the names are read; the credentials stay untouched.
pub fn pi_auth_provider_keys() -> Vec<String> {
    let Some(path) = pi_agent_dir().map(|dir| dir.join("auth.json")) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut keys = value
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// pi's global settings.json as raw JSON; `Value::Null` when absent or
/// unreadable, so callers treat every field as optional.
pub fn pi_agent_settings() -> serde_json::Value {
    let Some(path) = pi_agent_dir().map(|dir| dir.join("settings.json")) else {
        return serde_json::Value::Null;
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null)
}

impl PiProbe {
    /// The version string, when the CLI answered.
    pub fn version(&self) -> Option<&str> {
        match self {
            PiProbe::Ready { version, .. } => Some(version.as_str()),
            _ => None,
        }
    }

    /// A one-line explanation for a UI banner, when `pi` was found but is
    /// unusable. `None` when it is ready, or simply absent (that case has its
    /// own install hint).
    pub fn problem(&self) -> Option<String> {
        match self {
            PiProbe::Ready { .. } | PiProbe::Missing => None,
            PiProbe::Broken {
                program,
                status,
                detail,
            } => {
                let mut text = format!("{} ({status})", program.display());
                if !detail.is_empty() {
                    text.push_str(": ");
                    text.push_str(detail);
                }
                Some(text)
            }
        }
    }
}

/// Probe the `pi` CLI for onboarding UIs.
///
/// The three outcomes are distinguished on purpose: a GUI launch that cannot
/// see the installer's directory and an installed-but-unrunnable CLI both used
/// to read as "pi is not installed", which sent users looking for the wrong
/// problem.
pub fn probe_pi_install(pi_path: Option<&PathBuf>) -> PiProbe {
    let program = resolve_pi_program(&pi_path.cloned().unwrap_or_else(|| "pi".into()));
    let mut command = pi_command(&program);
    command.arg("--version").stdin(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    match command.output() {
        Ok(output) if output.status.success() => PiProbe::Ready {
            program,
            version: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        },
        Ok(output) => PiProbe::Broken {
            program,
            status: output.status.to_string(),
            detail: first_lines(&String::from_utf8_lossy(&output.stderr), 3),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PiProbe::Missing,
        Err(error) => PiProbe::Broken {
            program,
            status: error.kind().to_string(),
            detail: error.to_string(),
        },
    }
}

/// The first `count` non-empty lines of `text`, joined with ` · ` and capped
/// at 300 characters, for a banner.
fn first_lines(text: &str, count: usize) -> String {
    let joined = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(count)
        .collect::<Vec<_>>()
        .join(" · ");
    joined.chars().take(300).collect()
}

/// Check whether pi is authenticated for a provider, via `pi auth check`.
///
/// `Ok(true)` ready, `Ok(false)` not ready, `Err` when the command itself
/// could not run. Exit codes: 0 ready, 1 not_ready, 2 error.
pub fn probe_auth(provider: &str, pi_path: Option<&PathBuf>) -> anyhow::Result<bool> {
    let program = resolve_pi_program(&pi_path.cloned().unwrap_or_else(|| "pi".into()));
    let mut command = pi_command(&program);
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
    fn pi_versions_parse_for_comparison() {
        assert_eq!(pi_version_tuple("1.5.2"), Some((1, 5, 2)));
        assert_eq!(pi_version_tuple("v0.85"), Some((0, 85, 0)));
        assert_eq!(pi_version_tuple("1.10.0-beta.1"), Some((1, 10, 0)));
        assert_eq!(pi_version_tuple(" 2.0 "), Some((2, 0, 0)));
        assert_eq!(pi_version_tuple("unknown"), None);
        assert_eq!(pi_version_tuple(""), None);
    }

    #[test]
    fn probe_pi_install_finds_local_pi() {
        // This machine has pi installed (it is a project prerequisite), so a
        // successful probe here validates the plumbing end to end.
        if which_pi().is_some() {
            assert!(probe_pi_install(None).version().is_some());
        }
    }

    fn which_pi() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(if cfg!(windows) { "pi.cmd" } else { "pi" }))
                .find(|path| path.exists())
        })
    }

    #[test]
    fn a_missing_program_is_missing_not_broken() {
        let absent = PathBuf::from("pido-test-no-such-directory").join("pi");
        assert_eq!(probe_pi_install(Some(&absent)), PiProbe::Missing);
    }

    /// A script standing in for `pi`: records whether a non-executable file
    /// reads as broken instead of missing, which is what a bad `chmod` or a
    /// missing shebang interpreter looks like from here.
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str, mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("chmod script");
        path
    }

    #[cfg(unix)]
    #[test]
    fn a_working_pi_reports_its_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(dir.path(), "pi", "#!/bin/sh\necho 0.99.0\n", 0o755);
        match probe_pi_install(Some(&path)) {
            PiProbe::Ready { version, .. } => assert_eq!(version, "0.99.0"),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unrunnable_pi_reports_the_reason() {
        // Exit 127 with stderr is exactly what `#!/usr/bin/env node` prints
        // when the desktop session's PATH has no node.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            dir.path(),
            "pi",
            "#!/bin/sh\necho 'env: node: No such file or directory' >&2\nexit 127\n",
            0o755,
        );
        match probe_pi_install(Some(&path)) {
            PiProbe::Broken { status, detail, .. } => {
                assert!(status.contains("127"), "status was {status}");
                assert!(detail.contains("node"), "detail was {detail}");
            }
            other => panic!("expected Broken, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_non_executable_pi_is_not_taken_for_a_missing_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(dir.path(), "pi", "#!/bin/sh\nexit 0\n", 0o644);
        assert!(!is_runnable(&path), "0664 is not executable");
        assert!(
            matches!(probe_pi_install(Some(&path)), PiProbe::Broken { .. }),
            "an unreadable install must not read as absent"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_runnable(&path));
    }

    #[test]
    fn directories_are_searched_in_order_and_skip_non_executables() {
        let empty = tempfile::tempdir().expect("tempdir");
        let real = tempfile::tempdir().expect("tempdir");
        #[cfg(unix)]
        let name = "pi";
        #[cfg(windows)]
        let name = "pi.cmd";
        let found = real.path().join(name);
        std::fs::write(&found, "").expect("write candidate");
        // On Unix a runnable file needs its execute bit; `fs::write` leaves
        // 0o644, which `is_runnable` correctly rejects.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&found, std::fs::Permissions::from_mode(0o755))
                .expect("chmod candidate");
        }

        // The same name without the execute bit must be walked past (Unix
        // only: Windows has no such bit).
        #[cfg(unix)]
        let stale = tempfile::tempdir().expect("tempdir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let stale_copy = stale.path().join(name);
            std::fs::write(&stale_copy, "").expect("write stale candidate");
            std::fs::set_permissions(&stale_copy, std::fs::Permissions::from_mode(0o644))
                .expect("chmod stale candidate");
        }

        // An empty directory is skipped, an unrunnable copy is skipped, and
        // the runnable file in the last directory wins.
        let mut dirs = vec![empty.path().to_path_buf()];
        #[cfg(unix)]
        dirs.push(stale.path().to_path_buf());
        dirs.push(real.path().to_path_buf());
        let hit = find_in_dirs(dirs, [name]);
        assert_eq!(hit.as_deref(), Some(found.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn a_pi_under_a_versioned_node_manager_is_found() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("tempdir");
        let old = home.path().join(".nvm/versions/node/v18.0.0/bin");
        let new = home.path().join(".nvm/versions/node/v22.19.0/bin");
        for dir in [&old, &new] {
            std::fs::create_dir_all(dir).expect("mkdir");
            let pi = dir.join("pi");
            std::fs::write(&pi, "#!/bin/sh\nexit 0\n").expect("write pi");
            std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        assert_eq!(
            find_in_node_managers(home.path()),
            Some(new.join("pi")),
            "the newest Node release wins"
        );
    }

    #[test]
    fn a_pi_outside_path_gets_its_own_directory_first() {
        // The nvm/volta/asdf case: pi is found by its shim directory, and the
        // `node` its shebang needs lives beside it.
        let dir = tempfile::tempdir().expect("tempdir");
        let program = dir.path().join(if cfg!(windows) { "pi.cmd" } else { "pi" });
        std::fs::write(&program, "").expect("write candidate");

        let path = pi_command(&program)
            .get_envs()
            .find(|(key, _)| key.to_string_lossy() == "PATH")
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
            .expect("PATH is set when pi lives outside it");
        assert_eq!(
            std::env::split_paths(&path).next().as_deref(),
            Some(dir.path())
        );
    }
}
