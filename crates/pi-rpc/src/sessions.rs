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

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Default session root: `~/.pi/agent/sessions`.
pub fn default_session_dir() -> Option<PathBuf> {
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
    /// Display name from the latest `session_info` entry, if any.
    pub name: Option<String>,
    /// Preview text: the first user message, truncated.
    pub preview: Option<String>,
    /// Number of `message` entries in the file.
    pub message_count: usize,
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

/// Scan a session root directory and return all found sessions grouped by
/// working directory, groups ordered by their newest session.
///
/// Unreadable or malformed files are skipped (a partial write should not hide
/// the rest of the list).
pub fn scan_sessions(root: &Path) -> anyhow::Result<Vec<SessionGroup>> {
    let mut summaries = Vec::new();
    for dir_entry in walk_dirs(root)? {
        let summary = match parse_session_file(&dir_entry) {
            Some(summary) => summary,
            None => {
                log::warn!("skipping unparseable session file: {}", dir_entry.display());
                continue;
            }
        };
        summaries.push(summary);
    }

    // Newest first within a group.
    summaries.sort_by_key(|summary| std::cmp::Reverse(sort_key(summary)));

    let mut groups: Vec<SessionGroup> = Vec::new();
    for summary in summaries {
        if let Some(group) = groups.iter_mut().find(|g| g.cwd == summary.cwd) {
            group.sessions.push(summary);
        } else {
            groups.push(SessionGroup {
                cwd: summary.cwd.clone(),
                sessions: vec![summary],
            });
        }
    }

    // Groups by newest session first.
    groups.sort_by_key(|g| {
        g.sessions
            .first()
            .and_then(|s| s.timestamp.clone())
            .unwrap_or_default()
    });
    groups.reverse();
    Ok(groups)
}

/// Sessions sort by their header timestamp, falling back to the file name.
fn sort_key(summary: &SessionSummary) -> (String, String) {
    (
        summary.timestamp.clone().unwrap_or_default(),
        summary
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// Recursively collect `*.jsonl` files under `root` (one level of project
/// directories in practice, but recursion costs nothing here).
fn walk_dirs(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(root)
        .map_err(|err| anyhow::anyhow!("cannot read session dir {}: {err}", root.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && let Ok(children) = std::fs::read_dir(&path)
        {
            for child in children.flatten() {
                let child_path = child.path();
                if child_path.extension().is_some_and(|ext| ext == "jsonl") {
                    files.push(child_path);
                }
            }
        }
    }
    Ok(files)
}

/// Parse one session file into a summary. `None` when the header line is
/// missing or malformed (not a session file).
fn parse_session_file(path: &Path) -> Option<SessionSummary> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();

    let header: SessionHeader = serde_json::from_str(lines.next()?).ok()?;
    let session_id = header.id?;
    let cwd = header.cwd.unwrap_or_default();

    let mut name: Option<String> = None;
    let mut preview: Option<String> = None;
    let mut message_count = 0usize;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<EntryTail>(line) else {
            continue;
        };
        match entry.entry_type.as_deref() {
            Some("session_info") => {
                if let Some(entry_name) = entry.name {
                    name = Some(entry_name); // last one wins
                }
            }
            Some("message") => {
                message_count += 1;
                if preview.is_none()
                    && let Some(message) = &entry.message
                    && message.get("role").and_then(|r| r.as_str()) == Some("user")
                {
                    preview = user_message_preview(message);
                }
            }
            _ => {}
        }
    }

    Some(SessionSummary {
        path: path.to_path_buf(),
        session_id,
        cwd,
        timestamp: header.timestamp,
        name,
        preview: preview.map(|p| truncate(&p, 120)),
        message_count,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_session(dir: &Path, file: &str, lines: &[String]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
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
        );
        write_session(
            &project_b,
            "2026-01-01T10-00-00-000Z_id-b.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"id-b","timestamp":"2026-01-01T10:00:00.000Z","cwd":"C:\\proj-b"}"#.into(),
            ],
        );
        // A stray non-session jsonl file must be skipped.
        write_session(
            &root.path().join("--junk--"),
            "not-a-session.jsonl",
            &[r#"{"type":"model_change"}"#.into()],
        );

        let groups = scan_sessions(root.path()).unwrap();
        assert_eq!(groups.len(), 2, "two project groups");

        // Groups ordered newest first: proj-a (Jan 3) before proj-b (Jan 1).
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
    fn preview_is_truncated() {
        let long = "x".repeat(500);
        assert!(truncate(&long, 120).chars().count() == 121);
        assert_eq!(truncate("short", 120), "short");
    }

    /// Minimal tempdir helper.
    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
}
