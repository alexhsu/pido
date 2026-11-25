//! Read-modify-write helpers for pi's configuration files (`settings.json`,
//! `auth.json`) and the enable/disable semantics pi's own `pi config`
//! implements. pi exposes no RPC for any of this, so pido edits the files
//! directly. pi reads settings, skills and extensions only at startup — a
//! running session picks changes up after a restart — while `auth.json` is
//! re-read on every credential use, so sign-outs apply immediately.

use std::path::Path;

use serde_json::{Map, Value};

/// pi's thinking levels, lowest to highest: the settings window's picker for
/// pi's startup default.
pub const DEFAULT_THINKING_LEVELS: &[&str] =
    &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

// ---------------------------------------------------------------------------
// File IO
// ---------------------------------------------------------------------------

/// The agent directory's `settings.json`.
pub fn agent_settings_path(agent_dir: &Path) -> std::path::PathBuf {
    agent_dir.join("settings.json")
}

/// The project's `.pi/settings.json`.
pub fn project_settings_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".pi").join("settings.json")
}

/// A parsed JSON file; `Value::Null` when absent. Malformed content is an
/// `Err` so callers never treat a broken file as "no settings".
pub fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Read-modify-write one JSON settings file. A malformed existing file is an
/// error — pido must not overwrite content it cannot parse. Unknown fields
/// survive naturally: only the key the closure touches is written. The write
/// lands through a sibling temp file + rename, like pido's own settings.
pub fn write_json_atomic(path: &Path, mutate: impl FnOnce(&mut Value)) -> Result<(), String> {
    let mut value = if path.exists() {
        read_json(path)?
    } else {
        Value::Object(Map::new())
    };
    if !value.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }
    mutate(&mut value);
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    write_text_atomic(path, text)
}

/// Replace `path`'s content atomically: write a sibling temp file, then
/// rename. Windows cannot rename over an existing file, so there the old
/// file is removed only after the temp write has succeeded — the same
/// fallback pido's own settings writer uses.
fn write_text_atomic(path: &Path, text: String) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Err(error.to_string());
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| error.to_string())?;
    if std::fs::rename(&temp, path).is_ok() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let _ = std::fs::remove_file(path);
        if std::fs::rename(&temp, path).is_ok() {
            return Ok(());
        }
    }
    let error = format!("cannot replace {}", path.display());
    let _ = std::fs::remove_file(&temp);
    Err(error)
}

/// Remove one provider's credentials from `auth.json`. Returns whether an
/// entry was actually removed. pi re-reads the file on every credential use,
/// so running sessions notice without a restart.
pub fn auth_remove_provider(agent_dir: &Path, provider: &str) -> Result<bool, String> {
    let path = agent_dir.join("auth.json");
    if !path.exists() {
        return Ok(false);
    }
    let mut value = read_json(&path)?;
    let Some(map) = value.as_object_mut() else {
        return Ok(false);
    };
    if map.remove(provider).is_none() {
        return Ok(false);
    }
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    write_text_atomic(&path, text)?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Resource enable/disable — pi config's semantics
// ---------------------------------------------------------------------------

/// The settings-array pattern pi writes for a discovered resource: its path
/// relative to the scope's base directory (the agent dir, or the project's
/// `.pi`), in native separators — the same bytes `pi config` writes. Skill
/// resources are their `SKILL.md` files, so a user-scope toggle reads
/// `-skills\<name>\SKILL.md` on Windows.
pub fn resource_pattern(base_dir: &Path, resource_path: &Path) -> Option<String> {
    let relative = resource_path.strip_prefix(base_dir).ok()?;
    Some(relative.to_string_lossy().into_owned())
}

/// Whether pi loads a discovered resource given one scope's settings array
/// (the `skills`/`extensions` list of that scope's settings.json). Mirrors
/// pi's `isEnabledByOverrides`: on by default, `!glob` excludes, `+pattern`
/// force-includes, `-pattern` force-excludes — and a later `-` beats an
/// earlier `+`. Plain entries do not affect discovery; they load extra
/// resources instead.
pub fn resource_enabled(
    base_dir: &Path,
    resource_path: &Path,
    entries: &[String],
    is_skill: bool,
) -> bool {
    let relative = resource_path
        .strip_prefix(base_dir)
        .unwrap_or(resource_path);
    let full = resource_path.to_string_lossy();
    let name = resource_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut candidates = vec![
        to_posix(&relative.to_string_lossy()),
        to_posix(&full),
        name.clone(),
    ];
    if is_skill {
        // A skill's resource path is its SKILL.md file; patterns written for
        // the skill's folder must match too (`-skills/review` or even
        // `-review`).
        candidates.push(
            to_posix(&relative.to_string_lossy())
                .rsplit_once('/')
                .map_or_else(String::new, |(dir, _)| dir.to_string()),
        );
        if let Some(parent) = resource_path.parent()
            && let Some(parent_name) = parent.file_name()
        {
            candidates.push(parent_name.to_string_lossy().into_owned());
        }
    }
    let candidates: Vec<String> = candidates.iter().map(|c| normalize_pattern(c)).collect();
    decide(entries, &candidates)
}

/// Whether a builtin extension loads, and which settings file owns the
/// decision: any project override entry matching it moves the whole decision
/// to the project's `extensions` array; otherwise the global array applies —
/// exactly pi's rule. `id` is the full form, e.g. `builtin:mcp`.
/// Returns `(enabled, project_owns)`.
pub fn builtin_extension_state(user: &Value, project: &Value, id: &str) -> (bool, bool) {
    let project_entries = string_array(project, "extensions");
    if project_entries
        .iter()
        .any(|entry| is_override(entry) && entry_matches(entry, id))
    {
        let candidates = [normalize_pattern(id)];
        return (decide(&project_entries, &candidates), true);
    }
    let candidates = [normalize_pattern(id)];
    (
        decide(&string_array(user, "extensions"), &candidates),
        false,
    )
}

/// Flip one resource in a settings array the way `pi config` does: drop every
/// existing entry for it (any `!`/`+`/`-` form of the same pattern), then
/// append the explicit `+pattern`/`-pattern`.
pub fn toggle_resource(entries: &mut Vec<String>, pattern: &str, enabled: bool) {
    entries.retain(|entry| {
        let trimmed = entry.trim();
        let stripped = trimmed.strip_prefix(['!', '+', '-']).unwrap_or(trimmed);
        stripped != pattern
    });
    entries.push(format!("{}{}", if enabled { '+' } else { '-' }, pattern));
}

/// Remove `source` from the `key` array (`extensions` or `packages`) of a
/// parsed settings value; entries may be plain strings or `{source}`
/// objects. Returns whether anything was removed.
pub fn remove_list_entry(value: &mut Value, key: &str, source: &str) -> bool {
    let Some(list) = value.get_mut(key).and_then(Value::as_array_mut) else {
        return false;
    };
    let before = list.len();
    list.retain(|entry| match entry {
        Value::String(text) => text != source,
        Value::Object(map) => map.get("source").and_then(Value::as_str) != Some(source),
        _ => true,
    });
    list.len() != before
}

/// Append a plain source string to the `key` array of a parsed settings
/// value, creating the array (and the object) when missing.
pub fn push_list_entry(value: &mut Value, key: &str, source: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let list = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(list) = list {
        list.push(Value::String(source.to_string()));
    }
}

// ---------------------------------------------------------------------------
// Pattern matching internals
// ---------------------------------------------------------------------------

/// `true` for entries that override discovery (`!`, `+`, `-` prefixes).
fn is_override(entry: &str) -> bool {
    matches!(entry.trim().chars().next(), Some('!' | '+' | '-'))
}

/// The string array at `key` of a parsed settings value; missing or
/// non-string entries read as absent.
pub fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Walk `entries` in pi's precedence order against the resource's candidate
/// names: default on, `!` glob excludes, `+` exact includes, `-` exact
/// excludes (a later match wins over an earlier one).
fn decide(entries: &[String], candidates: &[String]) -> bool {
    let mut enabled = true;
    for entry in entries {
        let trimmed = entry.trim();
        let Some(prefix) = trimmed.chars().next() else {
            continue;
        };
        if !matches!(prefix, '!' | '+' | '-') {
            continue;
        }
        let target = normalize_pattern(&trimmed[1..]);
        let hit = if prefix == '!' {
            candidates.iter().any(|c| glob_match(&target, c))
        } else {
            candidates.contains(&target)
        };
        if hit {
            // `!` and `-` switch off, `+` forces back on.
            enabled = prefix == '+';
        }
    }
    enabled
}

fn entry_matches(entry: &str, id: &str) -> bool {
    let trimmed = entry.trim();
    let stripped = trimmed.strip_prefix(['!', '+', '-']).unwrap_or(trimmed);
    normalize_pattern(stripped) == normalize_pattern(id)
}

/// pi-normalized pattern text: separators folded to `/`, a leading `./`
/// dropped, empty and `.` segments removed. Byte comparisons happen on this
/// form, so `-skills\xreview\xSKILL.md` matches the posix relative path.
fn normalize_pattern(pattern: &str) -> String {
    let trimmed = pattern.trim();
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
    to_posix(stripped)
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn to_posix(text: &str) -> String {
    text.replace('\\', "/")
}

/// A small glob subset — `*` within a path segment, `**` across segments,
/// `?` for one character — covers the exclusion patterns settings files
/// actually carry. pi itself uses the full minimatch.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let text_segments: Vec<&str> = text.split('/').collect();
    match_segments(&pattern_segments, &text_segments)
}

fn match_segments(pattern: &[&str], text: &[&str]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(&"**") => (0..=text.len()).any(|skip| match_segments(&pattern[1..], &text[skip..])),
        Some(segment) => {
            !text.is_empty()
                && segment_match(segment, text[0])
                && match_segments(&pattern[1..], &text[1..])
        }
    }
}

/// `*` and `?` within one path segment, the classic two-pointer scan.
fn segment_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(star) = star {
            p = star + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|entry| entry.to_string()).collect()
    }

    #[test]
    fn discovery_defaults_to_enabled_and_plain_entries_do_not_filter() {
        let base = PathBuf::from("/agent");
        let skill = base.join("skills").join("review").join("SKILL.md");
        assert!(resource_enabled(&base, &skill, &[], true));
        // Plain entries load extra resources; discovery is untouched.
        assert!(resource_enabled(
            &base,
            &skill,
            &entries(&["skills/other", "*.ts"]),
            true
        ));
    }

    #[test]
    fn exclusions_and_force_flags_follow_pi_precedence() {
        let base = PathBuf::from("/agent");
        let skill = base.join("skills").join("review").join("SKILL.md");
        // `!` glob excludes, matching the parent folder name too.
        assert!(!resource_enabled(
            &base,
            &skill,
            &entries(&["!skills/re*iew"]),
            true
        ));
        assert!(!resource_enabled(
            &base,
            &skill,
            &entries(&["!review"]),
            true
        ));
        // `+` force-includes over an exclusion...
        assert!(resource_enabled(
            &base,
            &skill,
            &entries(&["!review", "+skills/review/SKILL.md"]),
            true
        ));
        // ...and a later `-` wins over the `+`.
        assert!(!resource_enabled(
            &base,
            &skill,
            &entries(&["+skills/review/SKILL.md", "-skills/review/SKILL.md"]),
            true
        ));
        // Windows separators in the settings file match posix relative paths.
        assert!(!resource_enabled(
            &base,
            &skill,
            &entries(&[r"-skills\review\SKILL.md"]),
            true
        ));
        // The skill's folder alone is a valid exact target.
        assert!(!resource_enabled(
            &base,
            &skill,
            &entries(&["-skills/review"]),
            true
        ));
    }

    #[test]
    fn extensions_match_by_file_name_and_relative_path() {
        let base = PathBuf::from("/agent");
        let file = base.join("extensions").join("greet.ts");
        assert!(!resource_enabled(
            &base,
            &file,
            &entries(&["-extensions/greet.ts"]),
            false
        ));
        assert!(resource_enabled(
            &base,
            &file,
            &entries(&["-extensions/other.ts"]),
            false
        ));
        // `*.ts` globs hit the file name.
        assert!(!resource_enabled(&base, &file, &entries(&["!*.ts"]), false));
    }

    #[test]
    fn toggle_replaces_every_prior_form_of_the_pattern() {
        let mut list = entries(&[
            "skills/review/SKILL.md",
            "!other",
            "-skills/review/SKILL.md",
        ]);
        toggle_resource(&mut list, "skills/review/SKILL.md", false);
        assert_eq!(list, entries(&["!other", "-skills/review/SKILL.md"]));
        toggle_resource(&mut list, "skills/review/SKILL.md", true);
        assert_eq!(list, entries(&["!other", "+skills/review/SKILL.md"]));
        // Toggling twice leaves exactly one entry.
        toggle_resource(&mut list, "skills/review/SKILL.md", false);
        assert_eq!(list, entries(&["!other", "-skills/review/SKILL.md"]));
    }

    #[test]
    fn builtin_project_overrides_beat_the_global_array() {
        let user = serde_json::json!({});
        let project = serde_json::json!({"extensions": ["-builtin:mcp"]});
        let (enabled, owns) = builtin_extension_state(&user, &project, "builtin:mcp");
        assert!(!enabled);
        assert!(owns);
        // No project override: the global array decides and owns.
        let user_off = serde_json::json!({"extensions": ["-builtin:mcp"]});
        let (enabled, owns) =
            builtin_extension_state(&user_off, &serde_json::json!({}), "builtin:mcp");
        assert!(!enabled);
        assert!(!owns);
        // Plain project entries are not overrides.
        let plain = serde_json::json!({"extensions": ["./local.ts"]});
        let (enabled, owns) = builtin_extension_state(&user, &plain, "builtin:mcp");
        assert!(enabled);
        assert!(!owns);
    }

    #[test]
    fn write_preserves_unknown_fields_and_creates_missing_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"defaultProvider":"deepseek","custom":{"keep":true}}"#,
        )
        .expect("write");
        write_json_atomic(&path, |value| {
            push_list_entry(value, "skills", "-skills/review/SKILL.md");
        })
        .expect("write");
        let value = read_json(&path).expect("read");
        assert_eq!(value["defaultProvider"], "deepseek");
        assert_eq!(value["custom"]["keep"], true);
        assert_eq!(
            value["skills"],
            serde_json::json!(["-skills/review/SKILL.md"])
        );
        // A malformed file is refused, not clobbered.
        std::fs::write(&path, "not json").expect("write");
        assert!(write_json_atomic(&path, |_| {}).is_err());
        // A missing file starts from an empty object.
        let fresh = dir.path().join("nested").join("settings.json");
        write_json_atomic(&fresh, |value| {
            push_list_entry(value, "extensions", "./ext.ts")
        })
        .expect("write");
        assert_eq!(
            read_json(&fresh).expect("read")["extensions"][0],
            "./ext.ts"
        );
    }

    #[test]
    fn remove_and_auth_helpers_edit_only_their_own_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            "{\"packages\":[\"npm:a\",{\"source\":\"npm:b\"}],\"extensions\":[\"./x.ts\"]}",
        )
        .expect("write");
        let mut value = read_json(&path).expect("read");
        assert!(remove_list_entry(&mut value, "packages", "npm:a"));
        assert_eq!(value["packages"], serde_json::json!([{"source": "npm:b"}]));
        assert!(!remove_list_entry(&mut value, "packages", "npm:missing"));

        let auth = dir.path().join("auth.json");
        std::fs::write(
            &auth,
            r#"{"deepseek":{"type":"api_key"},"openai":{"type":"api_key"}}"#,
        )
        .expect("write");
        assert!(auth_remove_provider(dir.path(), "deepseek").expect("remove"));
        assert!(!auth_remove_provider(dir.path(), "deepseek").expect("remove again"));
        let value = read_json(&auth).expect("read");
        assert!(value.get("openai").is_some());
        assert!(value.get("deepseek").is_none());
    }

    #[test]
    fn patterns_use_native_separators_and_globs_cover_the_subset() {
        let base = PathBuf::from("/agent");
        let pattern = resource_pattern(&base, &base.join("skills").join("review").join("SKILL.md"))
            .expect("under base");
        // Windows builds carry `\`; both forms normalize to the same target.
        assert_eq!(normalize_pattern(&pattern), "skills/review/SKILL.md");
        assert_eq!(
            normalize_pattern("./skills/review/SKILL.md"),
            "skills/review/SKILL.md"
        );
        assert!(glob_match("skills/*", "skills/review"));
        assert!(!glob_match("skills/*", "skills/review/SKILL.md"));
        assert!(glob_match("skills/**", "skills/review/SKILL.md"));
        assert!(glob_match("*.ts", "greet.ts"));
        assert!(glob_match("g?eet.ts", "greet.ts"));
        assert!(!glob_match("g?eet.ts", "grreet.ts"));
    }
}
