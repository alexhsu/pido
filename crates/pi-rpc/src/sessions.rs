//! Discover Pi session files on disk.
//!
//! Pi stores sessions as JSONL under `~/.pi/agent/sessions/`, grouped into
//! directories named after the sanitized working directory
//! (`--<path-with-separators-dashed>--`, e.g. `C:\dev\x` becomes
//! `--C--dev-x--`). Each file is named `<timestamp>_<session-id>.jsonl`.
//!
//! The first line is a `session` header (authoritative `id`, `cwd`,
//! `timestamp`); `session_info` entries anywhere in the file set the display
//! name (last one wins). There is no `pi sessions` list command, so a desktop
//! client lists sessions by scanning this directory.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::io_util::read_line_capped_terminated;

/// Default session root: `$PI_CODING_AGENT_SESSION_DIR`, else
/// `~/.pi/agent/sessions`. Pi honours the same variable.
pub fn default_session_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|home| home.join(".pi").join("agent").join("sessions"))
}

/// Metadata parsed from a session JSONL file on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    /// Absolute path of the `.jsonl` file; pass to `switch_session`.
    pub path: PathBuf,
    /// Session id from the header line.
    pub session_id: String,
    /// Working directory recorded in the header.
    pub cwd: String,
    /// Header timestamp, ISO-8601 string as written by Pi.
    pub timestamp: Option<String>,
    /// When the file was last written — the session's last activity, as
    /// unix seconds (pi appends as the conversation progresses).
    pub updated_secs: Option<i64>,
    /// Display name from the latest `session_info` entry, if any.
    pub name: Option<String>,
    /// Preview text: the first user message, truncated.
    pub preview: Option<String>,
    /// Number of `message` entries in the file.
    pub message_count: usize,
    /// The session file this one was forked/cloned from, as recorded in the
    /// header by pi's `createBranchedSession`. `None` for original sessions.
    pub parent_session: Option<String>,
}

/// A project directory plus its sessions, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionGroup {
    /// Working directory (from session headers, not the folder name).
    pub cwd: String,
    /// Sessions sorted newest first.
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Deserialize)]
struct SessionHeader {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, rename = "parentSession")]
    parent_session: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EntryTail {
    /// The `message` payload for `message` entries.
    #[serde(default)]
    message: Option<serde_json::Value>,
    /// The `name` for `session_info` entries.
    #[serde(default)]
    name: Option<String>,
    /// The sanitized cwd for `label` entries (unused, kept for completeness).
    #[serde(default, rename = "type")]
    entry_type: Option<String>,
}

/// What a scan has learned from the bytes it has already read. Kept next to
/// the consumed offset so an append-only file only needs its new tail read.
#[derive(Clone, Default)]
struct ScanAccum {
    name: Option<String>,
    /// The first user message, untruncated.
    preview: Option<String>,
    message_count: usize,
}

impl ScanAccum {
    /// Fold one JSONL entry into the accumulators.
    fn consume(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(entry) = serde_json::from_str::<EntryTail>(line) else {
            return;
        };
        match entry.entry_type.as_deref() {
            Some("session_info") => {
                // Last one wins; a blank name clears the title, exactly as
                // pi's own getSessionName resolves it.
                self.name = entry
                    .name
                    .filter(|entry_name| !entry_name.trim().is_empty());
            }
            Some("message") => {
                self.message_count += 1;
                if self.preview.is_none()
                    && let Some(message) = &entry.message
                    && message.get("role").and_then(|r| r.as_str()) == Some("user")
                {
                    self.preview = user_message_preview(message);
                }
            }
            _ => {}
        }
    }
}

/// The header line's fields, which never change once written.
#[derive(Clone)]
struct SessionHead {
    session_id: String,
    cwd: String,
    timestamp: Option<String>,
    parent_session: Option<String>,
}

/// A cached session summary keyed by the file's metadata.
struct CachedSummary {
    modified: SystemTime,
    len: u64,
    /// How many bytes have been parsed: the end of the last complete line.
    consumed: u64,
    accum: ScanAccum,
    head: SessionHead,
    summary: SessionSummary,
}

/// Scans a session directory, reusing summaries for files whose size and
/// modification time have not changed. Session files can be large (the whole
/// transcript), so rescanning every file on every settle is wasteful.
#[derive(Default)]
pub struct SessionScanner {
    cache: HashMap<PathBuf, CachedSummary>,
}

impl SessionScanner {
    /// Scan `root`, parsing only new or changed files. Unreadable or malformed
    /// files are skipped; a partial write must not hide the rest of the list.
    ///
    /// A file that only grew (the normal case for the active session, which
    /// appends on every turn) is parsed from the byte it was last left at;
    /// a file that shrank or was rewritten in place is parsed whole again.
    pub fn scan(&mut self, root: &Path) -> anyhow::Result<Vec<SessionGroup>> {
        let files = walk_dirs(root)?;
        let present: HashSet<&Path> = files.iter().map(PathBuf::as_path).collect();
        let mut summaries = Vec::with_capacity(files.len());

        for path in &files {
            let stamp = file_stamp(path);
            let Some((modified, len)) = stamp else {
                // No metadata: parse without caching.
                if let Some(summary) = parse_session_file(path) {
                    summaries.push(summary);
                } else {
                    log::warn!("skipping unparseable session file: {}", path.display());
                }
                continue;
            };
            if let Some(cached) = self.cache.get(path) {
                if cached.modified == modified && cached.len == len {
                    summaries.push(cached.summary.clone());
                    continue;
                }
                if len > cached.consumed
                    && let Some((summary, consumed, accum)) =
                        append_scan(path, cached.consumed, &cached.head, cached.accum.clone())
                {
                    self.cache.insert(
                        path.clone(),
                        CachedSummary {
                            modified,
                            len,
                            consumed,
                            accum,
                            head: cached.head.clone(),
                            summary: summary.clone(),
                        },
                    );
                    summaries.push(summary);
                    continue;
                }
            }
            let Some((summary, consumed, accum, head)) = parse_session_file_full(path) else {
                log::warn!("skipping unparseable session file: {}", path.display());
                continue;
            };
            self.cache.insert(
                path.clone(),
                CachedSummary {
                    modified,
                    len,
                    consumed,
                    accum,
                    head,
                    summary: summary.clone(),
                },
            );
            summaries.push(summary);
        }

        self.cache
            .retain(|path, _| present.contains(path.as_path()));
        Ok(group_summaries(summaries))
    }
}

/// Parse just the bytes appended since `consumed`. Returns the refreshed
/// summary, the new consumed offset and the accumulators that produced it;
/// `None` when the file cannot be reopened or seeked (the caller then parses
/// it whole).
fn append_scan(
    path: &Path,
    consumed: u64,
    head: &SessionHead,
    mut accum: ScanAccum,
) -> Option<(SessionSummary, u64, ScanAccum)> {
    let mut file = std::fs::File::open(path).ok()?;
    use std::io::Seek as _;
    file.seek(std::io::SeekFrom::Start(consumed)).ok()?;
    let mut reader = BufReader::new(file);
    let next = scan_body(&mut reader, &mut accum, consumed, path);
    Some((build_summary(path, head, &accum), next, accum))
}

/// Fold every terminated line from `reader` into `accum`, starting at byte
/// offset `from`. Returns the offset just past the last line consumed.
///
/// An unterminated final line is never consumed: pi terminates every entry
/// with a newline, so an unterminated tail is a write still in flight.
/// Parsing it now would count it twice once the writer finishes the line.
fn scan_body<R: BufRead>(reader: &mut R, accum: &mut ScanAccum, from: u64, path: &Path) -> u64 {
    let mut offset = from;
    let mut buf = Vec::new();
    while let Ok((len, terminated)) = read_line_capped_terminated(reader, &mut buf, MAX_LINE) {
        if len == 0 || !terminated {
            break;
        }
        if len > MAX_LINE {
            log::warn!("session {}: skipped an oversized line", path.display());
        } else if let Ok(line) = std::str::from_utf8(&buf) {
            accum.consume(line.trim_end_matches(['\r', '\n']));
        }
        offset += len as u64;
    }
    offset
}

/// The summary implied by a header and the accumulators.
fn build_summary(path: &Path, head: &SessionHead, accum: &ScanAccum) -> SessionSummary {
    SessionSummary {
        updated_secs: file_mtime(path),
        path: path.to_path_buf(),
        session_id: head.session_id.clone(),
        cwd: head.cwd.clone(),
        timestamp: head.timestamp.clone(),
        name: accum.name.clone(),
        preview: accum.preview.as_ref().map(|p| truncate(p, 120)),
        message_count: accum.message_count,
        parent_session: head.parent_session.clone(),
    }
}

/// Scan a session root directory and return all found sessions grouped by
/// working directory, groups ordered by their newest session.
///
/// Unreadable or malformed files are skipped (a partial write should not hide
/// the rest of the list).
pub fn scan_sessions(root: &Path) -> anyhow::Result<Vec<SessionGroup>> {
    SessionScanner::default().scan(root)
}

/// Read the working directory from a session header without loading the
/// whole transcript. Used when switching to a session from another project.
pub fn read_session_cwd(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    let header: SessionHeader = serde_json::from_str(&line).ok()?;
    header.cwd
}

/// Append a `session_info` entry naming the session. Pi does the same when it
/// renames; writing it here lets pido rename a session that is not open
/// without switching to it first. The entry hangs off the file's last entry,
/// so the tree stays linear.
///
/// An empty `name` is not a no-op: pi resolves the title from the latest
/// `session_info` and treats a blank one as "cleared" — the only way to drop
/// a name, since its RPC refuses empty strings.
pub fn append_session_name(path: &Path, name: &str) -> std::io::Result<()> {
    let name = name.replace(['\r', '\n'], " ").trim().to_string();
    let text = std::fs::read_to_string(path)?;
    let mut parent = None;
    let mut taken = HashSet::new();
    for line in text.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(id) = value.get("id").and_then(|id| id.as_str())
        {
            taken.insert(id.to_string());
            parent = Some(id.to_string());
        }
    }
    let entry = serde_json::json!({
        "type": "session_info",
        "id": fresh_id(&taken),
        "parentId": parent,
        "timestamp": iso8601_now(),
        "name": name,
    });
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    // One write keeps our newline and entry from interleaving with a
    // concurrent pi append (the read/write race itself remains unavoidable).
    let mut payload = String::new();
    if !text.ends_with('\n') {
        payload.push('\n');
    }
    payload.push_str(&entry.to_string());
    payload.push('\n');
    file.write_all(payload.as_bytes())
}

/// An id Pi's session tree has not used yet: eight hex digits, like its own.
fn fresh_id(taken: &HashSet<String>) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos ^ (u64::from(std::process::id())).rotate_left(17);
    loop {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let id = format!("{:08x}", (seed >> 24) as u32);
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// Now, in the ISO-8601 form Pi writes.
fn iso8601_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    iso8601(secs)
}

fn iso8601(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Group summaries by cwd, newest activity first. Because `summaries` is
/// already sorted, the first occurrence of a cwd fixes that group's position.
fn group_summaries(mut summaries: Vec<SessionSummary>) -> Vec<SessionGroup> {
    summaries.sort_by(|a, b| {
        activity(b)
            .cmp(&activity(a))
            .then_with(|| sort_key(b).cmp(&sort_key(a)))
    });

    let mut groups: Vec<SessionGroup> = Vec::new();
    for summary in summaries {
        let key = cwd_key(&summary.cwd);
        let Some(group) = groups.iter_mut().find(|group| cwd_key(&group.cwd) == key) else {
            groups.push(SessionGroup {
                cwd: summary.cwd.clone(),
                sessions: vec![summary],
            });
            continue;
        };
        group.sessions.push(summary);
    }
    groups
}

/// Compare working directories without caring about slash style, a trailing
/// separator, or (on Windows) letter case. The sidebar's project grouping and
/// project labels use the same identity.
fn cwd_key(cwd: &str) -> String {
    let mut key = cwd.replace('\\', "/");
    while key.len() > 1 && key.ends_with('/') {
        key.pop();
    }
    if cfg!(windows) {
        key.make_ascii_lowercase();
    }
    key
}

/// Last activity, from the file's modification time. Falls back to 0 when
/// the filesystem cannot report one.
fn activity(summary: &SessionSummary) -> i64 {
    summary.updated_secs.unwrap_or(0)
}

/// Tie-breaker for sessions with the same activity second.
fn sort_key(summary: &SessionSummary) -> String {
    summary
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Size and modification time, used to decide whether a cached summary is
/// still valid.
fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Recursively collect `*.jsonl` files under `root` (one level of project
/// directories in practice, but recursion costs nothing here).
fn walk_dirs(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(root)
        .map_err(|err| anyhow::anyhow!("cannot read session dir {}: {err}", root.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!("session dir {}: {error}", root.display());
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let children = match std::fs::read_dir(&path) {
            Ok(children) => children,
            Err(error) => {
                log::warn!("session dir {}: {error}", path.display());
                continue;
            }
        };
        for child in children {
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    log::warn!("session dir {}: {error}", path.display());
                    continue;
                }
            };
            let child_path = child.path();
            if child_path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(child_path);
            }
        }
    }
    Ok(files)
}

/// The per-line cap for summary scanning: a multi-megabyte transcript (base64
/// tool images can make one line huge) must not be loaded whole.
const MAX_LINE: usize = 4 * 1024 * 1024;

/// Parse one session file into a summary. `None` when the header line is
/// missing or malformed (not a session file).
fn parse_session_file(path: &Path) -> Option<SessionSummary> {
    parse_session_file_full(path).map(|(summary, _, _, _)| summary)
}

/// Parse one session file whole: the summary, how many bytes that consumed,
/// the accumulators behind it, and the immutable header fields. The scanner
/// caches these so a later scan can read only the appended tail.
fn parse_session_file_full(path: &Path) -> Option<(SessionSummary, u64, ScanAccum, SessionHead)> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();

    let (header_len, _) = read_line_capped_terminated(&mut reader, &mut buf, MAX_LINE).ok()?;
    if header_len == 0 || header_len > MAX_LINE {
        return None;
    }
    let header_text = std::str::from_utf8(&buf).ok()?;
    let header: SessionHeader =
        serde_json::from_str(header_text.trim_end_matches(['\r', '\n'])).ok()?;
    let head = SessionHead {
        session_id: header.id?,
        cwd: header.cwd.unwrap_or_default(),
        timestamp: header.timestamp,
        parent_session: header.parent_session,
    };

    let mut accum = ScanAccum::default();
    let consumed = scan_body(&mut reader, &mut accum, header_len as u64, path);
    let summary = build_summary(path, &head, &accum);
    Some((summary, consumed, accum, head))
}

/// The file's last modification time, as unix seconds; None when unknown.
fn file_mtime(path: &Path) -> Option<i64> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Extract a preview string from a user message payload (`content` may be a
/// string or an array of text blocks).
fn user_message_preview(message: &serde_json::Value) -> Option<String> {
    match message.get("content")? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(blocks) => {
            let text: String = blocks
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        block.get("text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() { None } else { Some(text) }
        }
        _ => None,
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// Find the session files whose message text contains `query` (case-
/// insensitive), with a short snippet around the first hit in each. Reads
/// line by line and stops at a file's first hit; JSONL keeps each message on
/// one line, so a raw-line hit is a cheap prefilter before parsing only that
/// line.
pub fn search_content(root: &Path, query: &str) -> Vec<(PathBuf, String)> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let Ok(files) = walk_dirs(root) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for path in files {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if !line.to_lowercase().contains(&needle) || !line.starts_with("{\"type\":\"message\"")
            {
                continue;
            }
            // Parse only the line that matched, then aim the snippet at the
            // match instead of the text's head. The snippet comes from the
            // lowercased text: `to_lowercase` can shift byte offsets between
            // the two, and a quiet-case snippet is fine.
            let Ok(entry) = serde_json::from_str::<EntryTail>(line.trim_end()) else {
                continue;
            };
            if let Some(text) = entry.message.as_ref().and_then(user_message_preview)
                && let Some(at) = text.to_lowercase().find(&needle)
            {
                let lower = text.to_lowercase();
                // Char boundary at or before the match; plain byte
                // arithmetic would risk splitting a char.
                let start = lower[..at]
                    .char_indices()
                    .map(|(ix, _)| ix)
                    .rfind(|_| true)
                    .unwrap_or(0);
                let snippet: String = lower[start..].chars().take(120).collect();
                hits.push((path.clone(), snippet));
                break;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a session and set its modification time, in seconds since the
    /// epoch, so activity ordering is deterministic.
    fn write_session(dir: &Path, file: &str, lines: &[String], mtime_secs: u64) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs))
            .unwrap();
        path
    }

    #[test]
    fn scan_groups_and_orders_sessions() {
        let root = tempdir();
        let project_a = root.path().join("--C--proj-a--");
        let project_b = root.path().join("--C--proj-b--");

        write_session(
            &project_a,
            "2026-01-02T10-00-00-000Z_id-old.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-old","timestamp":"2026-01-02T10:00:00.000Z","cwd":"C:\\proj-a"}"#.into(),
                r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-02T10:00:01.000Z","message":{"role":"user","content":"Older question about Rust","timestamp":1}}"#.into(),
            ],
            10,
        );
        write_session(
            &project_a,
            "2026-01-03T10-00-00-000Z_id-new.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-new","timestamp":"2026-01-03T10:00:00.000Z","cwd":"C:\\proj-a"}"#.into(),
                r#"{"type":"message","id":"m2","parentId":null,"timestamp":"2026-01-03T10:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"Newer question"}],"timestamp":2}}"#.into(),
                r#"{"type":"session_info","id":"s1","parentId":"m2","timestamp":"2026-01-03T10:05:00.000Z","name":"my session"}"#.into(),
                r#"{"type":"session_info","id":"s2","parentId":"s1","timestamp":"2026-01-03T10:06:00.000Z","name":"renamed"}"#.into(),
            ],
            30,
        );
        write_session(
            &project_b,
            "2026-01-01T10-00-00-000Z_id-b.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-b","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\proj-b"}"#.into(),
            ],
            20,
        );
        // A stray non-session jsonl file must be skipped.
        write_session(
            &root.path().join("--junk--"),
            "not-a-session.jsonl",
            &[r#"{"type":"model_change"}"#.into()],
            40,
        );

        let groups = scan_sessions(root.path()).unwrap();
        assert_eq!(groups.len(), 2, "two project groups");

        // Groups ordered by newest activity: proj-a (30s) before proj-b
        // (20s), regardless of the header timestamps' calendar order.
        assert_eq!(groups[0].cwd, "C:\\proj-a");
        assert_eq!(groups[1].cwd, "C:\\proj-b");

        // Within the group: newest session first, name from latest
        // session_info, preview from the first user message.
        let group_a = &groups[0];
        assert_eq!(group_a.sessions.len(), 2);
        assert_eq!(group_a.sessions[0].session_id, "id-new");
        assert_eq!(group_a.sessions[0].name.as_deref(), Some("renamed"));
        assert_eq!(
            group_a.sessions[0].preview.as_deref(),
            Some("Newer question")
        );
        assert_eq!(group_a.sessions[1].session_id, "id-old");
        assert_eq!(
            group_a.sessions[1].preview.as_deref(),
            Some("Older question about Rust")
        );
        assert_eq!(group_a.sessions[1].message_count, 1);
    }

    #[test]
    fn search_content_finds_messages_with_snippets() {
        let root = tempdir();
        let project = root.path().join("--C--proj--");
        write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id-hit.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-hit","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\proj"}"#.into(),
                r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T10:00:01.000Z","message":{"role":"user","content":"How does token compaction work?","timestamp":1}}"#.into(),
                r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-01-01T10:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Compaction folds older messages into a summary."}],"timestamp":2}}"#.into(),
            ],
            10,
        );
        write_session(
            &project,
            "2026-01-01T09-00-00-000Z_id-other.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-other","timestamp":"2026-01-01T09:00:00.000Z","cwd":"C:\proj"}"#.into(),
                r#"{"type":"message","id":"m0","parentId":null,"timestamp":"2026-01-01T09:00:01.000Z","message":{"role":"user","content":"Unrelated question","timestamp":0}}"#.into(),
            ],
            5,
        );

        let hits = search_content(root.path(), "COMPACTION");
        assert_eq!(hits.len(), 1, "only the session that mentions it");
        // The snippet aims at the match, not the text's head.
        assert!(hits[0].1.to_lowercase().contains("compaction"));
        assert!(search_content(root.path(), "zzz").is_empty());
    }

    #[test]
    fn summary_records_the_fork_lineage_from_the_header() {
        let root = tempdir();
        let project = root.path().join("--C--proj--");
        write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id-child.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-child","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\proj","parentSession":"C:\\Users\\x\\.pi\\agent\\sessions\\--C--proj--\\parent.jsonl"}"#.into(),
            ],
            10,
        );
        write_session(
            &project,
            "2026-01-01T09-00-00-000Z_id-root.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-root","timestamp":"2026-01-01T09:00:00.000Z","cwd":"C:\\proj"}"#.into(),
            ],
            5,
        );

        let groups = scan_sessions(root.path()).unwrap();
        let sessions = &groups[0].sessions;
        let child = sessions
            .iter()
            .find(|session| session.session_id == "id-child")
            .unwrap();
        let root_session = sessions
            .iter()
            .find(|session| session.session_id == "id-root")
            .unwrap();
        assert!(
            child
                .parent_session
                .as_deref()
                .unwrap()
                .ends_with("parent.jsonl"),
            "the fork records its parent"
        );
        assert!(root_session.parent_session.is_none());
    }

    #[test]
    fn scanner_reuses_unchanged_files_and_sees_updates() {
        let root = tempdir();
        let project = root.path().join("--C--one--");
        let path = write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#.into(),
            ],
            10,
        );

        let mut scanner = SessionScanner::default();
        let first = scanner.scan(root.path()).unwrap();
        assert_eq!(first[0].sessions[0].message_count, 0);

        // Appending a message changes the size, so the cached summary is
        // invalidated and the message count updates.
        let mut lines = std::fs::read_to_string(&path).unwrap();
        lines.push_str(
            r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T10:00:01.000Z","message":{"role":"user","content":"hi","timestamp":1}}"#,
        );
        lines.push('\n');
        std::fs::write(&path, lines).unwrap();
        let second = scanner.scan(root.path()).unwrap();
        assert_eq!(second[0].sessions[0].message_count, 1);
    }

    #[test]
    fn incremental_scan_matches_a_full_parse() {
        let root = tempdir();
        let project = root.path().join("--C--one--");
        let path = write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#.into(),
                r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T10:00:01.000Z","message":{"role":"user","content":"first ask","timestamp":1}}"#.into(),
            ],
            10,
        );
        let mut scanner = SessionScanner::default();
        scanner.scan(root.path()).unwrap();

        // Append a name and two messages — the ordinary turn-by-turn growth.
        append_session_name(&path, "named later").unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        for id in ["m2", "m3"] {
            text.push_str(&format!(
                r#"{{"type":"message","id":"{id}","parentId":"m1","timestamp":"2026-01-01T10:00:02.000Z","message":{{"role":"user","content":"more","timestamp":2}}}}"#
            ));
            text.push('\n');
        }
        std::fs::write(&path, &text).unwrap();

        let incremented = scanner.scan(root.path()).unwrap()[0].sessions[0].clone();
        let full = parse_session_file(&path).expect("parses");
        assert_eq!(incremented.name.as_deref(), Some("named later"));
        assert_eq!(incremented.preview.as_deref(), Some("first ask"));
        assert_eq!(incremented.message_count, 3);
        assert_eq!(
            (
                incremented.name,
                incremented.preview,
                incremented.message_count
            ),
            (full.name, full.preview, full.message_count)
        );
    }

    #[test]
    fn a_partially_written_line_waits_for_its_newline() {
        let root = tempdir();
        let project = root.path().join("--C--one--");
        let path = write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#.into()],
            10,
        );
        // A writer stopped halfway through an entry: no trailing newline.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(
            r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T10:00:01.000Z","message":{"role":"user","content":"half"#,
        );
        std::fs::write(&path, &text).unwrap();
        let mut scanner = SessionScanner::default();
        assert_eq!(
            scanner.scan(root.path()).unwrap()[0].sessions[0].message_count,
            0,
            "an unterminated tail is not an entry yet"
        );

        // The writer finishes the line: it must count exactly once.
        text.push_str(r#""},"timestamp":1}"#);
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            scanner.scan(root.path()).unwrap()[0].sessions[0].message_count,
            1
        );
        // And a rescan with no further writes keeps that count.
        assert_eq!(
            scanner.scan(root.path()).unwrap()[0].sessions[0].message_count,
            1
        );
    }

    #[test]
    fn a_truncated_file_falls_back_to_a_full_parse() {
        let root = tempdir();
        let project = root.path().join("--C--one--");
        let header = r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#;
        let message = |id: &str| {
            format!(
                r#"{{"type":"message","id":"{id}","parentId":null,"timestamp":"2026-01-01T10:00:01.000Z","message":{{"role":"user","content":"hi","timestamp":1}}}}"#
            )
        };
        let path = write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[header.into(), message("m1"), message("m2")],
            10,
        );
        let mut scanner = SessionScanner::default();
        assert_eq!(
            scanner.scan(root.path()).unwrap()[0].sessions[0].message_count,
            2
        );

        // Rewritten shorter (a compaction or a hand edit): the incremental
        // path cannot see the removed bytes, so it must re-parse whole.
        std::fs::write(&path, format!("{header}\n{}\n", message("m3"))).unwrap();
        let summary = scanner.scan(root.path()).unwrap()[0].sessions[0].clone();
        assert_eq!(summary.message_count, 1);
    }

    #[test]
    fn read_session_cwd_reads_only_the_header() {
        let root = tempdir();
        let project = root.path().join("--C--one--");
        let path = write_session(
            &project,
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#.into(),
            ],
            10,
        );
        assert_eq!(read_session_cwd(&path).as_deref(), Some("C:\\one"));
    }

    #[test]
    fn cwd_spellings_share_one_key() {
        assert_eq!(cwd_key("C:\\dev\\x\\"), cwd_key("C:/dev/x"));
        assert_eq!(cwd_key("/home/a/"), cwd_key("/home/a"));
        assert_eq!(cwd_key("/"), "/");
    }

    #[test]
    fn preview_is_truncated() {
        let long = "x".repeat(500);
        assert!(truncate(&long, 120).chars().count() == 121);
        assert_eq!(truncate("short", 120), "short");
    }

    /// Minimal tempdir helper.
    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn appending_a_name_becomes_the_latest_session_info() {
        let root = tempdir();
        let path = write_session(
            &root.path().join("--C--one--"),
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#.into(),
                r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T10:01:00.000Z","message":{"role":"user","content":"hi","timestamp":1}}"#.into(),
            ],
            1_000,
        );
        append_session_name(&path, "  renamed\nnow  ").unwrap();
        let summary = parse_session_file(&path).expect("parses");
        assert_eq!(summary.name.as_deref(), Some("renamed now"));
        let text = std::fs::read_to_string(&path).unwrap();
        let last = text.lines().last().unwrap();
        let entry: serde_json::Value = serde_json::from_str(last).unwrap();
        assert_eq!(entry["type"], "session_info");
        assert_eq!(entry["parentId"], "m1", "hangs off the last entry");
        assert_eq!(entry["id"].as_str().unwrap().len(), 8);
    }

    #[test]
    fn an_empty_name_clears_the_title() {
        // pi's RPC refuses empty strings; its file format carries the clear
        // as a `session_info` whose name is blank, and the parser must read
        // that back as "no name".
        let root = tempdir();
        let path = write_session(
            &root.path().join("--C--one--"),
            "2026-01-01T10-00-00-000Z_id.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\one"}"#.into(),
                r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T10:01:00.000Z","message":{"role":"user","content":"hi","timestamp":1}}"#.into(),
            ],
            1_000,
        );
        append_session_name(&path, "named").unwrap();
        assert_eq!(
            parse_session_file(&path).expect("parses").name.as_deref(),
            Some("named")
        );

        append_session_name(&path, "   ").unwrap();
        let summary = parse_session_file(&path).expect("parses");
        assert_eq!(summary.name, None, "a blank session_info clears the title");
        let text = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(entry["type"], "session_info");
        assert_eq!(entry["name"], "");
    }

    #[test]
    fn an_appended_name_keeps_the_file_newline_discipline() {
        let root = tempdir();
        let dir = root.path().join("--C--one--");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        // No trailing newline: the appended entry must start on its own line.
        std::fs::write(
            &path,
            "{\"type\":\"session\",\"version\":3,\"id\":\"id\",\"cwd\":\"C:/one\"}",
        )
        .unwrap();
        append_session_name(&path, "named").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().last().unwrap().contains("\"named\""));
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn iso8601_matches_a_known_instant() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20.000Z");
    }
}
