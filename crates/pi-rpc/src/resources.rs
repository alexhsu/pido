//! Views of pi's optional resources — skills and configured extension and
//! package sources — for pido's settings window. Everything user-visible is
//! parsed here so the UI only formats; the enable flags come from the same
//! rules pi applies at startup (see [`crate::config`]).

use std::path::{Path, PathBuf};

use crate::config;

/// Whether a resource comes from pi's agent directory or the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `~/.pi/agent/...`
    User,
    /// `<cwd>/.pi/...`
    Project,
}

/// A skill found in one of pi's skill directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    /// The skill's folder, for reveal buttons and toggles.
    pub path: PathBuf,
    /// Whether pi currently loads it — that scope's settings array decides.
    pub enabled: bool,
}

/// Where a configured extension comes from; drives what editing it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionChannel {
    /// A path listed in an `extensions` array.
    SettingArray,
    /// A `packages` entry (`npm:`/`git:`/`https://` source).
    Package,
    /// A loose file or folder in an `extensions/` directory.
    Discovered,
}

/// One extension row for the settings window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionEntry {
    pub source: String,
    pub scope: Scope,
    pub channel: ExtensionChannel,
    /// Whether pi currently loads it.
    pub enabled: bool,
    /// The resolved local path, when the entry is a local file or folder.
    pub path: Option<PathBuf>,
}

/// pi's builtin extensions, always loaded unless disabled by name in
/// `extensions` (`-builtin:mcp`) or via `pi config`.
pub const BUILTIN_EXTENSIONS: &[&str] = &[
    "builtin:mcp",
    "builtin:llama.cpp",
    "builtin:codemode",
    "builtin:tool-search",
];

/// One-line text: newlines flattened so single-line UI components never see
/// a `\n` (they grow tall, and shaping panics on it outright).
pub fn flatten_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Strip one layer of matching YAML quotes.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Parse `name` and `description` out of a SKILL.md YAML frontmatter. The
/// description may be a plain scalar or a block scalar (`|`/`>` with the
/// indented lines that follow); anything else is `None`.
pub fn parse_skill_frontmatter(text: &str) -> Option<(String, String)> {
    const BLOCK_MARKERS: [&str; 6] = ["|", "|-", "|+", ">", ">-", ">+"];
    // Editors on Windows save UTF-8 with a BOM; without stripping it the first
    // line reads as "\u{feff}---" and the whole skill vanishes silently.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().copied().map(str::trim_end) != Some("---") {
        return None;
    }
    let mut name = None;
    let mut description = None;
    let mut ix = 1;
    while ix < lines.len() {
        let line = lines[ix];
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let value = unquote(rest).trim();
            if !value.is_empty() && name.is_none() {
                name = Some(value.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("description:") {
            let value = rest.trim();
            if description.is_none() {
                if BLOCK_MARKERS.contains(&value) {
                    // Block scalar: every more-indented line that follows.
                    let mut parts = Vec::new();
                    ix += 1;
                    while ix < lines.len()
                        && (lines[ix].starts_with(' ') || lines[ix].starts_with('\t'))
                    {
                        parts.push(lines[ix].trim().to_string());
                        ix += 1;
                    }
                    description = Some(flatten_line(&parts.join("\n")));
                    continue;
                }
                if !value.is_empty() {
                    description = Some(flatten_line(unquote(value)));
                }
            }
        }
        ix += 1;
        if name.is_some() && description.is_some() {
            break;
        }
    }
    Some((name?, description?))
}

/// Skills from `<agent_dir>/skills/*` (user) and `<cwd>/.pi/skills/*`
/// (project). A directory counts only when its SKILL.md parses. Each entry
/// carries the enabled state pi's matching scope settings array gives it.
pub fn pi_skills(agent_dir: Option<&Path>, project_cwd: Option<&Path>) -> Vec<SkillSummary> {
    let mut skills = Vec::new();
    if let Some(dir) = agent_dir {
        let entries = settings_file_strings(&config::agent_settings_path(dir), "skills");
        collect_skills(&dir.join("skills"), dir, Scope::User, &entries, &mut skills);
    }
    if let Some(cwd) = project_cwd {
        let base = cwd.join(".pi");
        let entries = settings_file_strings(&config::project_settings_path(cwd), "skills");
        collect_skills(
            &base.join("skills"),
            &base,
            Scope::Project,
            &entries,
            &mut skills,
        );
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Read one settings file and pull a string array out of it; a missing or
/// malformed file reads as empty (every resource then shows enabled).
fn settings_file_strings(path: &Path, key: &str) -> Vec<String> {
    config::read_json(path)
        .map(|value| config::string_array(&value, key))
        .unwrap_or_default()
}

fn collect_skills(
    dir: &Path,
    base_dir: &Path,
    scope: Scope,
    entries: &[String],
    out: &mut Vec<SkillSummary>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let skill_md = path.join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        let Some((name, description)) = parse_skill_frontmatter(&text) else {
            continue;
        };
        let enabled = config::resource_enabled(base_dir, &skill_md, entries, true);
        out.push(SkillSummary {
            name,
            description,
            scope,
            path,
            enabled,
        });
    }
}

/// Configured extension/package entries: both levels' settings.json
/// `packages` and `extensions` arrays (string or `{source}` entries), plus
/// loose files in each scope's `extensions/` folder. Global first, then
/// project, matching pi's combine order. Builtin extensions are not listed
/// here — the settings window renders [`BUILTIN_EXTENSIONS`] with
/// [`config::builtin_extension_state`].
pub fn pi_extensions(agent_dir: Option<&Path>, project_cwd: Option<&Path>) -> Vec<ExtensionEntry> {
    let mut entries = Vec::new();
    let user_settings = agent_dir
        .map(config::agent_settings_path)
        .and_then(|path| config::read_json(&path).ok());
    let project_settings = project_cwd
        .map(config::project_settings_path)
        .and_then(|path| config::read_json(&path).ok());
    if let Some(dir) = agent_dir {
        extension_entries_from_value(
            user_settings.as_ref().unwrap_or(&serde_json::Value::Null),
            Scope::User,
            dir,
            &mut entries,
        );
        collect_extension_files(
            &dir.join("extensions"),
            dir,
            Scope::User,
            user_settings.as_ref(),
            &mut entries,
        );
    }
    if let Some(cwd) = project_cwd {
        let base = cwd.join(".pi");
        extension_entries_from_value(
            project_settings
                .as_ref()
                .unwrap_or(&serde_json::Value::Null),
            Scope::Project,
            &base,
            &mut entries,
        );
        collect_extension_files(
            &base.join("extensions"),
            &base,
            Scope::Project,
            project_settings.as_ref(),
            &mut entries,
        );
    }
    entries
}

/// Parse the `packages` and `extensions` arrays out of one settings value
/// into channel-tagged rows. Split out for tests; [`pi_extensions`] feeds it
/// from disk.
pub fn extension_entries_from_value(
    value: &serde_json::Value,
    scope: Scope,
    base_dir: &Path,
    out: &mut Vec<ExtensionEntry>,
) {
    let object = match value.as_object() {
        Some(object) => object,
        None => return,
    };
    for (key, channel) in [
        ("packages", ExtensionChannel::Package),
        ("extensions", ExtensionChannel::SettingArray),
    ] {
        let list = match object.get(key).and_then(|list| list.as_array()) {
            Some(list) => list,
            None => continue,
        };
        let overrides = config::string_array(value, "extensions");
        for entry in list {
            let source = match entry {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Object(map) => map
                    .get("source")
                    .and_then(|source| source.as_str())
                    .map(String::from),
                _ => None,
            };
            let Some(source) = source.filter(|source| !source.is_empty()) else {
                continue;
            };
            // In an `extensions` array, override entries (`+`/`-`/`!`) are
            // toggles for discovered resources, not rows of their own — pi's
            // loader splits them out before resolving sources.
            if channel == ExtensionChannel::SettingArray
                && matches!(source.chars().next(), Some('!' | '+' | '-'))
            {
                continue;
            }
            // pi tolerates duplicates across scopes; showing one row per
            // (source, scope, channel) keeps the list honest without noise.
            if out.iter().any(|existing| {
                existing.source == source && existing.scope == scope && existing.channel == channel
            }) {
                continue;
            }
            // A local path resolves against the settings file that named it;
            // remote specs (npm:/git:/https) have no local face. The enabled
            // flag follows the same scope's `extensions` overrides.
            let local = if source.starts_with("npm:")
                || source.starts_with("git:")
                || source.starts_with("https://")
                || source.starts_with("http://")
            {
                None
            } else {
                let candidate = PathBuf::from(&source);
                Some(if candidate.is_absolute() {
                    candidate
                } else {
                    base_dir.join(candidate)
                })
            };
            let enabled = local
                .as_deref()
                .is_none_or(|path| config::resource_enabled(base_dir, path, &overrides, false));
            out.push(ExtensionEntry {
                source,
                scope,
                channel,
                enabled,
                path: local,
            });
        }
    }
}

fn collect_extension_files(
    dir: &Path,
    base_dir: &Path,
    scope: Scope,
    settings: Option<&serde_json::Value>,
    out: &mut Vec<ExtensionEntry>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let overrides = settings
        .map(|value| config::string_array(value, "extensions"))
        .unwrap_or_default();
    for entry in read.flatten() {
        let path = entry.path();
        // pi discovers directories with an entry point and loose .ts/.js
        // files; anything else in the folder is noise.
        let discovered = path.is_dir()
            || matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("ts" | "js")
            );
        if !discovered {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let enabled = config::resource_enabled(base_dir, &path, &overrides, false);
        out.push(ExtensionEntry {
            source: name.to_string(),
            scope,
            channel: ExtensionChannel::Discovered,
            enabled,
            path: Some(path),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_line_removes_every_newline() {
        assert_eq!(flatten_line("a\r\nb\nc"), "a · b · c");
        assert_eq!(flatten_line("  spaced  "), "spaced");
        assert_eq!(flatten_line("\n\n"), "");
        assert_eq!(flatten_line("plain"), "plain");
    }

    #[test]
    fn frontmatter_reads_simple_fields() {
        let text = "---\nname: code-review\ndescription: Review the current diff.\nlicense: MIT\n---\n\n# Body";
        assert_eq!(
            parse_skill_frontmatter(text),
            Some((
                "code-review".to_string(),
                "Review the current diff.".to_string()
            ))
        );
    }

    #[test]
    fn frontmatter_reads_quoted_and_block_descriptions() {
        let quoted = "---\nname: \"quoted-name\"\ndescription: 'has: colon'\n---\nbody";
        assert_eq!(
            parse_skill_frontmatter(quoted),
            Some(("quoted-name".to_string(), "has: colon".to_string()))
        );
        let block = "---\nname: blocky\ndescription: |\n  first line\n  second line\n---\nbody";
        assert_eq!(
            parse_skill_frontmatter(block),
            Some(("blocky".to_string(), "first line · second line".to_string()))
        );
    }

    #[test]
    fn frontmatter_rejects_missing_fields_and_documents() {
        assert_eq!(parse_skill_frontmatter("no frontmatter at all"), None);
        assert_eq!(parse_skill_frontmatter("---\nname: only\n---\n"), None);
        let both = "---\ndescription: desc only\n---\n";
        assert_eq!(parse_skill_frontmatter(both), None);
    }

    #[test]
    fn a_utf8_bom_does_not_hide_a_skill() {
        // Windows editors save SKILL.md with a BOM; pi's own loader reads the
        // file as text, so the skill must not disappear from the list.
        let text = "\u{feff}---\nname: bom-skill\ndescription: Saved by Notepad.\n---\nbody";
        assert_eq!(
            parse_skill_frontmatter(text),
            Some(("bom-skill".to_string(), "Saved by Notepad.".to_string()))
        );
    }

    #[test]
    fn extension_entries_read_strings_objects_channels_and_dedupe() {
        let text = r#"{
            "packages": [
                "npm:@scope/pkg@1",
                {"source": "git:github.com/x/y@main", "skills": true},
                "npm:@scope/pkg@1"
            ],
            "extensions": ["./ext.ts"],
            "retry": {"enabled": true}
        }"#;
        let value: serde_json::Value = serde_json::from_str(text).expect("json");
        let mut entries = Vec::new();
        extension_entries_from_value(&value, Scope::User, Path::new("/agent"), &mut entries);
        assert_eq!(
            entries,
            vec![
                ExtensionEntry {
                    source: "npm:@scope/pkg@1".to_string(),
                    scope: Scope::User,
                    channel: ExtensionChannel::Package,
                    enabled: true,
                    path: None,
                },
                ExtensionEntry {
                    source: "git:github.com/x/y@main".to_string(),
                    scope: Scope::User,
                    channel: ExtensionChannel::Package,
                    enabled: true,
                    path: None,
                },
                ExtensionEntry {
                    source: "./ext.ts".to_string(),
                    scope: Scope::User,
                    channel: ExtensionChannel::SettingArray,
                    enabled: true,
                    path: Some(PathBuf::from("/agent/ext.ts")),
                },
            ]
        );
    }

    #[test]
    fn extension_entries_ignore_garbage_and_empty() {
        let mut entries = Vec::new();
        extension_entries_from_value(
            &serde_json::Value::Null,
            Scope::User,
            Path::new("/agent"),
            &mut entries,
        );
        assert!(entries.is_empty());
        let garbage: serde_json::Value =
            serde_json::from_str(r#"{"packages": ["", {"no_source": 1}, 42]}"#).expect("json");
        extension_entries_from_value(&garbage, Scope::User, Path::new("/agent"), &mut entries);
        assert!(entries.is_empty());
    }

    #[test]
    fn a_settings_override_disables_a_local_extension_entry() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"extensions": ["./ext.ts", "-ext.ts"]}"#).expect("json");
        let mut entries = Vec::new();
        extension_entries_from_value(&value, Scope::User, Path::new("/agent"), &mut entries);
        // Only the plain entry is a configured row; the `-ext.ts` override is
        // what switches it off.
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].enabled);
    }
}
