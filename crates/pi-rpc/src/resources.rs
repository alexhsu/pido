//! Read-only views of pi's optional resources — skills and configured
//! extension/package sources — for pido's settings window. Everything
//! user-visible is parsed here so the UI only formats.

use std::path::Path;

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
}

/// A configured extension or package source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionSource {
    pub source: String,
    pub scope: Scope,
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
/// (project). A directory counts only when its SKILL.md parses.
pub fn pi_skills(agent_dir: Option<&Path>, project_cwd: Option<&Path>) -> Vec<SkillSummary> {
    let mut skills = Vec::new();
    if let Some(dir) = agent_dir {
        collect_skills(&dir.join("skills"), Scope::User, &mut skills);
    }
    if let Some(cwd) = project_cwd {
        collect_skills(&cwd.join(".pi").join("skills"), Scope::Project, &mut skills);
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn collect_skills(dir: &Path, scope: Scope, out: &mut Vec<SkillSummary>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path().join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some((name, description)) = parse_skill_frontmatter(&text) else {
            continue;
        };
        out.push(SkillSummary {
            name,
            description,
            scope,
        });
    }
}

/// Configured extension/package sources: both levels' settings.json
/// `packages` and `extensions` arrays (string or `{source}` entries), plus
/// loose files in the agent directory's `extensions/` folder. Global first,
/// then project, matching pi's combine order.
pub fn pi_extensions(agent_dir: Option<&Path>, project_cwd: Option<&Path>) -> Vec<ExtensionSource> {
    let mut sources = Vec::new();
    if let Some(dir) = agent_dir {
        collect_setting_sources(&dir.join("settings.json"), Scope::User, &mut sources);
        collect_extension_files(&dir.join("extensions"), Scope::User, &mut sources);
    }
    if let Some(cwd) = project_cwd {
        collect_setting_sources(
            &cwd.join(".pi").join("settings.json"),
            Scope::Project,
            &mut sources,
        );
        collect_extension_files(&cwd.join(".pi").join("extensions"), Scope::Project, &mut sources);
    }
    sources
}

/// Parse the `packages`/`extensions` arrays out of one settings file's
/// text. Split out for tests; [`pi_extensions`] feeds it from disk.
pub fn extension_sources_from_text(text: &str, scope: Scope, out: &mut Vec<ExtensionSource>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(object) = value.as_object() else {
        return;
    };
    for key in ["packages", "extensions"] {
        let Some(list) = object.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for entry in list {
            let source = match entry {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Object(map) => map
                    .get("source")
                    .and_then(|source| source.as_str())
                    .map(String::from),
                _ => None,
            };
            let Some(source) = source else { continue };
            if source.is_empty() {
                continue;
            }
            // pi tolerates duplicates across scopes; showing one row per
            // (source, scope) keeps the list honest without noise.
            if !out
                .iter()
                .any(|existing| existing.source == source && existing.scope == scope)
            {
                out.push(ExtensionSource { source, scope });
            }
        }
    }
}

fn collect_setting_sources(path: &Path, scope: Scope, out: &mut Vec<ExtensionSource>) {
    if let Ok(text) = std::fs::read_to_string(path) {
        extension_sources_from_text(&text, scope, out);
    }
}

fn collect_extension_files(dir: &Path, scope: Scope, out: &mut Vec<ExtensionSource>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // pi discovers directories with an entry point and loose .ts/.js
        // files; anything else in the folder is noise.
        let discovered = path.is_dir()
            || matches!(path.extension().and_then(|ext| ext.to_str()), Some("ts" | "js"));
        if !discovered {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        out.push(ExtensionSource {
            source: name.to_string(),
            scope,
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
            Some((
                "blocky".to_string(),
                "first line · second line".to_string()
            ))
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
    fn extension_sources_read_strings_objects_and_dedupe() {
        let text = r#"{
            "packages": [
                "npm:@scope/pkg@1",
                {"source": "git:github.com/x/y@main", "skills": true},
                "npm:@scope/pkg@1"
            ],
            "extensions": ["./ext.ts"],
            "retry": {"enabled": true}
        }"#;
        let mut sources = Vec::new();
        extension_sources_from_text(text, Scope::User, &mut sources);
        assert_eq!(
            sources,
            vec![
                ExtensionSource {
                    source: "npm:@scope/pkg@1".to_string(),
                    scope: Scope::User,
                },
                ExtensionSource {
                    source: "git:github.com/x/y@main".to_string(),
                    scope: Scope::User,
                },
                ExtensionSource {
                    source: "./ext.ts".to_string(),
                    scope: Scope::User,
                },
            ]
        );
    }

    #[test]
    fn extension_sources_ignore_garbage_and_empty() {
        let mut sources = Vec::new();
        extension_sources_from_text("not json", Scope::User, &mut sources);
        assert!(sources.is_empty());
        extension_sources_from_text(
            r#"{"packages": ["", {"no_source": 1}, 42]}"#,
            Scope::User,
            &mut sources,
        );
        assert!(sources.is_empty());
    }
}
