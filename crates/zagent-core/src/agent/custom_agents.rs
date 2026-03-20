use std::collections::HashSet;
use std::fs;

use crate::provider::types::ToolDefinition;
use regex::Regex;
use serde::Deserialize;

const CUSTOM_AGENTS_DIR: &str = ".agents";
const MAX_CUSTOM_AGENTS: usize = 64;
const MAX_CUSTOM_AGENT_FILE_BYTES: usize = 32_000;

#[derive(Debug, Clone)]
pub(crate) struct CustomAgentDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) model: String,
    pub(crate) user_invokable: bool,
    pub(crate) invoke_default: bool,
    pub(crate) tools: Vec<String>,
    pub(crate) handoffs: Vec<CustomAgentHandoffDefinition>,
    pub(crate) instructions: String,
    pub(crate) tool_name: String,
    pub(crate) relative_path_from_cwd: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CustomAgentHandoffDefinition {
    pub(crate) label: String,
    pub(crate) agent: String,
    pub(crate) prompt: Option<String>,
    pub(crate) send: Option<bool>,
    pub(crate) model: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AgentManifest {
    id: Option<serde_yaml::Value>,
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    #[serde(default, rename = "user-invokable")]
    user_invokable: bool,
    #[serde(default, rename = "invoke-default")]
    invoke_default: bool,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    handoffs: Vec<ManifestHandoff>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ManifestHandoff {
    label: Option<String>,
    agent: Option<String>,
    prompt: Option<String>,
    send: Option<bool>,
    model: Option<String>,
}

pub(crate) fn push_custom_agents_prompt_section(
    out: &mut String,
    custom_agents: &[CustomAgentDefinition],
) {
    if custom_agents.is_empty() {
        return;
    }

    out.push_str("\n\n# Custom Agents\n");
    out.push_str(
        "Specialized child agents are available. Use their handoff tools when a focused \
sub-task can be solved in isolation. During handoff, the child agent runs with a clean \
context and only the handoff request.\n\n",
    );
    for agent in custom_agents {
        out.push_str(&format!(
            "- {} (tool: {}): {} [source: {}]\n",
            agent.name, agent.tool_name, agent.description, agent.relative_path_from_cwd
        ));
        if agent.user_invokable {
            out.push_str("  user-invokable: true\n");
        }
        if agent.user_invokable && agent.invoke_default {
            out.push_str("  invoke-default: true\n");
        }
        if !agent.handoffs.is_empty() {
            out.push_str("  handoffs:\n");
            for handoff in &agent.handoffs {
                out.push_str(&format!(
                    "  - {} -> {}{}\n",
                    handoff.label,
                    handoff.agent,
                    handoff
                        .model
                        .as_ref()
                        .map(|m| format!(" (model: {m})"))
                        .unwrap_or_default()
                ));
            }
        }
    }
    out.push_str(
        "\nWhen using a handoff tool, pass a complete `task` and optional `context`. \
Do not assume the child has access to the parent conversation history.",
    );
}

pub(crate) fn collect_custom_agents(
    working_dir: &str,
    default_model: &str,
) -> Vec<CustomAgentDefinition> {
    let cwd = super::resolve_path(working_dir);
    let agents_dir = cwd.join(CUSTOM_AGENTS_DIR);
    let Ok(entries) = fs::read_dir(&agents_dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        if out.len() >= MAX_CUSTOM_AGENTS {
            break;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".md") {
            continue;
        }

        let Some(base_name) = custom_agent_base_name(&name) else {
            continue;
        };

        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let clipped = if bytes.len() > MAX_CUSTOM_AGENT_FILE_BYTES {
            &bytes[..MAX_CUSTOM_AGENT_FILE_BYTES]
        } else {
            &bytes
        };
        let mut content = String::from_utf8_lossy(clipped).to_string();
        if bytes.len() > MAX_CUSTOM_AGENT_FILE_BYTES {
            content.push_str(&format!(
                "\n\n[truncated at {} bytes]",
                MAX_CUSTOM_AGENT_FILE_BYTES
            ));
        }

        let (manifest, body) = parse_frontmatter(&content);
        if manifest.id.is_some() {
            continue;
        }
        let name = manifest.name.unwrap_or_else(|| base_name.to_string());
        let id = sanitize_custom_agent_id(&name);
        if id.is_empty() {
            continue;
        }
        let description = manifest
            .description
            .unwrap_or_else(|| format!("Specialized child agent for {}", name));
        let model = manifest
            .model
            .as_ref()
            .filter(|m| !m.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| default_model.to_string());
        let tools = manifest
            .tools
            .iter()
            .filter_map(|t| {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .collect::<Vec<_>>();
        let handoffs = manifest
            .handoffs
            .iter()
            .filter_map(|h| {
                let agent = h.agent.as_deref().map(str::trim).unwrap_or_default();
                if agent.is_empty() {
                    return None;
                }
                Some(CustomAgentHandoffDefinition {
                    label: h
                        .label
                        .as_deref()
                        .filter(|v| !v.trim().is_empty())
                        .unwrap_or("handoff")
                        .trim()
                        .to_string(),
                    agent: agent.to_string(),
                    prompt: h.prompt.clone().filter(|v| !v.trim().is_empty()),
                    send: h.send,
                    model: h.model.clone().filter(|v| !v.trim().is_empty()),
                })
            })
            .collect::<Vec<_>>();
        let instructions = body.trim().to_string();
        if instructions.is_empty() {
            continue;
        }
        let relative = path
            .strip_prefix(&cwd)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        out.push(CustomAgentDefinition {
            tool_name: format!("handoff_{id}"),
            id,
            name,
            description,
            model,
            user_invokable: manifest.user_invokable,
            invoke_default: manifest.user_invokable && manifest.invoke_default,
            tools,
            handoffs,
            instructions,
            relative_path_from_cwd: relative,
        });
    }

    out.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
    out.truncate(MAX_CUSTOM_AGENTS);
    out
}

pub(crate) fn resolve_user_invocation<'a>(
    user_message: &str,
    custom_agents: &'a [CustomAgentDefinition],
) -> (Option<&'a CustomAgentDefinition>, String, bool) {
    if let Some((target_token, remaining)) = parse_user_invocation_prefix(user_message)
        && let Some(agent) = custom_agents
            .iter()
            .filter(|a| a.user_invokable)
            .find(|a| custom_agent_name_key(&a.name) == custom_agent_name_key(&target_token))
    {
        return (Some(agent), remaining.to_string(), true);
    }

    if let Some(default_agent) = custom_agents
        .iter()
        .find(|a| a.user_invokable && a.invoke_default)
    {
        return (Some(default_agent), user_message.to_string(), false);
    }

    (None, user_message.to_string(), false)
}

fn parse_user_invocation_prefix(user_message: &str) -> Option<(String, &str)> {
    let trimmed = user_message.trim_start();
    let rest = trimmed.strip_prefix('@')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let token = parts.next()?.trim();
    if token.is_empty() {
        return None;
    }
    let remaining = parts.next().unwrap_or("").trim_start();
    Some((token.to_string(), remaining))
}

pub(crate) fn custom_agent_tool_definition(agent: &CustomAgentDefinition) -> ToolDefinition {
    ToolDefinition::function(
        agent.tool_name.clone(),
        format!(
            "Handoff a focused sub-task to child agent '{}' ({}).",
            agent.name, agent.description
        ),
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Complete task to execute in the child agent."
                },
                "context": {
                    "type": "string",
                    "description": "Optional context needed by the child agent."
                }
            },
            "required": ["task"],
            "additionalProperties": false
        }),
    )
}

pub(crate) fn resolve_handoff_scope<'a>(
    active_custom_agent_id: Option<&str>,
    custom_agents: &'a [CustomAgentDefinition],
) -> Vec<&'a CustomAgentDefinition> {
    let Some(active_id) = active_custom_agent_id else {
        return custom_agents.iter().collect();
    };
    let Some(active) = custom_agents.iter().find(|a| a.id == active_id) else {
        return custom_agents.iter().collect();
    };
    if active.handoffs.is_empty() {
        return Vec::new();
    }

    let mut allowed_ids = HashSet::new();
    for handoff in &active.handoffs {
        allowed_ids.insert(custom_agent_name_key(&handoff.agent));
    }

    custom_agents
        .iter()
        .filter(|agent| allowed_ids.contains(&custom_agent_name_key(&agent.name)))
        .collect()
}

pub(crate) fn custom_agent_name_key(name: &str) -> String {
    sanitize_custom_agent_id(name)
}

pub(crate) fn resolve_allowed_runtime_tools(
    active_custom_agent_id: Option<&str>,
    custom_agents: &[CustomAgentDefinition],
) -> Option<ToolAccessPolicy> {
    let active_id = active_custom_agent_id?;
    let active = custom_agents.iter().find(|a| a.id == active_id)?;
    let mut patterns = HashSet::new();
    for declared in &active.tools {
        for resolved in expand_declared_tool_pattern(declared) {
            patterns.insert(resolved);
        }
    }
    Some(ToolAccessPolicy::new(patterns.into_iter().collect()))
}

fn expand_declared_tool_pattern(input: &str) -> Vec<String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "search" => vec!["websearch".to_string()],
        "fetch" => vec!["webfetch".to_string()],
        "read" | "read_fs" | "read_filesystem" => {
            vec!["file_read".to_string(), "list_dir".to_string()]
        }
        "write" | "write_fs" | "write_filesystem" => {
            vec!["file_write".to_string(), "file_edit".to_string()]
        }
        "filesystem" | "fs" => vec![
            "file_read".to_string(),
            "file_write".to_string(),
            "file_edit".to_string(),
            "list_dir".to_string(),
        ],
        "vcs" | "git" | "version_control" => {
            vec![
                "shell_exec".to_string(),
                "file_read".to_string(),
                "list_dir".to_string(),
            ]
        }
        "" => vec![],
        other => vec![other.to_string()],
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ToolAccessPolicy {
    patterns: Vec<String>,
    compiled: Vec<Regex>,
}

impl ToolAccessPolicy {
    fn new(mut patterns: Vec<String>) -> Self {
        patterns.sort();
        patterns.dedup();
        let compiled = patterns
            .iter()
            .filter_map(|p| compile_tool_pattern(p))
            .collect::<Vec<_>>();
        Self { patterns, compiled }
    }

    pub(crate) fn allows(&self, tool_name: &str) -> bool {
        self.compiled.iter().any(|r| r.is_match(tool_name))
    }

    pub(crate) fn patterns(&self) -> &[String] {
        &self.patterns
    }

    pub(crate) fn resolve_allowed_names<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut out = names
            .into_iter()
            .filter(|n| self.allows(n))
            .map(str::to_string)
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        out
    }
}

fn compile_tool_pattern(pattern: &str) -> Option<Regex> {
    let normalized = pattern.trim().replace('/', "_");
    if normalized.is_empty() {
        return None;
    }

    if let Some(expr) = normalized.strip_prefix("re:") {
        return Regex::new(expr).ok();
    }

    let mut expr = String::from("^");
    for ch in normalized.chars() {
        if ch == '*' {
            expr.push_str(".*");
        } else {
            expr.push_str(&regex::escape(&ch.to_string()));
        }
    }
    expr.push('$');
    Regex::new(&expr).ok()
}

fn custom_agent_base_name(file_name: &str) -> Option<String> {
    if let Some(base) = file_name.strip_suffix(".agent.md") {
        return Some(base.to_string());
    }
    file_name.strip_suffix(".md").map(str::to_string)
}

fn parse_frontmatter(content: &str) -> (AgentManifest, String) {
    let Some(rest) = content.strip_prefix("---\n") else {
        return (AgentManifest::default(), content.to_string());
    };
    let Some(end) = rest.find("\n---\n") else {
        return (AgentManifest::default(), content.to_string());
    };
    let meta_block = &rest[..end];
    let body = &rest[end + "\n---\n".len()..];
    let parsed = serde_yaml::from_str::<AgentManifest>(meta_block).unwrap_or_default();
    (parsed, body.to_string())
}

fn sanitize_custom_agent_id(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_was_sep = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_was_sep = false;
            continue;
        }
        if !prev_was_sep {
            out.push('_');
            prev_was_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let input = r#"---
name: Rust Expert
description: Handles rust-only tasks
model: openai/gpt-5.2
---
You are a Rust specialist.
"#;
        let (meta, body) = parse_frontmatter(input);
        assert_eq!(meta.name, Some("Rust Expert".to_string()));
        assert_eq!(
            meta.description,
            Some("Handles rust-only tasks".to_string())
        );
        assert_eq!(meta.model, Some("openai/gpt-5.2".to_string()));
        assert_eq!(body.trim(), "You are a Rust specialist.");
    }

    #[test]
    fn parses_yaml_handoffs_manifest() {
        let input = r#"---
description: Generate an implementation plan
tools: ['search', 'fetch']
handoffs:
  - label: Start Implementation
    agent: Implementation Agent
    prompt: Now implement the plan outlined above.
    send: false
    model: GPT-5.2 (copilot)
---
Planner body.
"#;
        let (meta, body) = parse_frontmatter(input);
        assert_eq!(
            meta.description,
            Some("Generate an implementation plan".to_string())
        );
        assert_eq!(meta.tools, vec!["search".to_string(), "fetch".to_string()]);
        assert_eq!(meta.handoffs.len(), 1);
        let h = &meta.handoffs[0];
        assert_eq!(h.label, Some("Start Implementation".to_string()));
        assert_eq!(h.agent, Some("Implementation Agent".to_string()));
        assert_eq!(
            h.prompt,
            Some("Now implement the plan outlined above.".to_string())
        );
        assert_eq!(h.send, Some(false));
        assert_eq!(h.model, Some("GPT-5.2 (copilot)".to_string()));
        assert_eq!(body.trim(), "Planner body.");
    }

    #[test]
    fn sanitize_agent_id_rewrites_symbols() {
        assert_eq!(sanitize_custom_agent_id("Rust Expert!"), "rust_expert");
        assert_eq!(sanitize_custom_agent_id("___A---B___"), "a_b");
        assert_eq!(sanitize_custom_agent_id(""), "");
    }

    #[test]
    fn builds_handoff_tool_from_custom_agent() {
        let agent = CustomAgentDefinition {
            id: "rust".to_string(),
            name: "Rust".to_string(),
            description: "Rust coding tasks".to_string(),
            model: "minimax/minimax-m2.5".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec![],
            handoffs: vec![],
            instructions: "Focus on Rust.".to_string(),
            tool_name: "handoff_rust".to_string(),
            relative_path_from_cwd: ".agents/rust.md".to_string(),
        };
        let tool = custom_agent_tool_definition(&agent);
        assert_eq!(tool.function.name, "handoff_rust");
        assert!(tool.function.description.contains("Rust"));
    }

    #[test]
    fn handoff_scope_restricts_by_manifest() {
        let coordinator = CustomAgentDefinition {
            id: "coordinator".to_string(),
            name: "Coordinator".to_string(),
            description: "Coordinates".to_string(),
            model: "minimax/minimax-m2.5".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec![],
            handoffs: vec![CustomAgentHandoffDefinition {
                label: "Start Implementation".to_string(),
                agent: "Implementation Agent".to_string(),
                prompt: Some("Now implement.".to_string()),
                send: Some(false),
                model: Some("GPT-5.2 (copilot)".to_string()),
            }],
            instructions: "Coordinate.".to_string(),
            tool_name: "handoff_coordinator".to_string(),
            relative_path_from_cwd: ".agents/coordinator.md".to_string(),
        };
        let implementation = CustomAgentDefinition {
            id: "implementation_agent".to_string(),
            name: "Implementation Agent".to_string(),
            description: "Implements".to_string(),
            model: "minimax/minimax-m2.5".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec![],
            handoffs: vec![],
            instructions: "Implement.".to_string(),
            tool_name: "handoff_implementation".to_string(),
            relative_path_from_cwd: ".agents/implementation.md".to_string(),
        };
        let review = CustomAgentDefinition {
            id: "review".to_string(),
            name: "Review".to_string(),
            description: "Reviews".to_string(),
            model: "minimax/minimax-m2.5".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec![],
            handoffs: vec![],
            instructions: "Review.".to_string(),
            tool_name: "handoff_review".to_string(),
            relative_path_from_cwd: ".agents/review.md".to_string(),
        };
        let all = vec![coordinator, implementation, review];
        let scoped = resolve_handoff_scope(Some("coordinator"), &all);
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "implementation_agent");
    }

    #[test]
    fn handoff_scope_empty_when_active_agent_has_no_handoffs() {
        let no_handoffs = CustomAgentDefinition {
            id: "inspector".to_string(),
            name: "code_inspector".to_string(),
            description: "inspects".to_string(),
            model: "minimax/minimax-m2.5".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec!["file_read".to_string(), "list_dir".to_string()],
            handoffs: vec![],
            instructions: "Inspect only.".to_string(),
            tool_name: "handoff_code_inspector".to_string(),
            relative_path_from_cwd: ".agents/code_inspector.md".to_string(),
        };
        let other = CustomAgentDefinition {
            id: "planning".to_string(),
            name: "planning".to_string(),
            description: "plans".to_string(),
            model: "minimax/minimax-m2.5".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec!["websearch".to_string()],
            handoffs: vec![],
            instructions: "Plan.".to_string(),
            tool_name: "handoff_planning".to_string(),
            relative_path_from_cwd: ".agents/planning.md".to_string(),
        };

        let all = vec![no_handoffs, other];
        let scoped = resolve_handoff_scope(Some("inspector"), &all);
        assert!(scoped.is_empty());
    }

    #[test]
    fn collect_custom_agents_reads_manifest_handoffs() {
        let root =
            std::env::temp_dir().join(format!("zagent-custom-agent-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".agents")).expect("create .agents");
        fs::write(
            root.join(".agents/planner.agent.md"),
            r#"---
name: Planner Agent
description: Generate an implementation plan
user-invokable: true
invoke-default: true
tools: ['search', 'fetch']
handoffs:
  - label: Start Implementation
    agent: Implementation Agent
    prompt: Now implement the plan outlined above.
    send: false
    model: GPT-5.2 (copilot)
---
Planner instructions.
"#,
        )
        .expect("write agent file");

        let agents = collect_custom_agents(&root.to_string_lossy(), "minimax/minimax-m2.5");
        assert_eq!(agents.len(), 1);
        let agent = &agents[0];
        assert_eq!(agent.id, "planner_agent");
        assert!(agent.user_invokable);
        assert!(agent.invoke_default);
        assert_eq!(agent.tools, vec!["search".to_string(), "fetch".to_string()]);
        assert_eq!(agent.handoffs.len(), 1);
        assert_eq!(agent.handoffs[0].label, "Start Implementation");
        assert_eq!(agent.handoffs[0].agent, "Implementation Agent");
        assert_eq!(
            agent.handoffs[0].prompt,
            Some("Now implement the plan outlined above.".to_string())
        );
        assert_eq!(agent.handoffs[0].send, Some(false));
        assert_eq!(
            agent.handoffs[0].model,
            Some("GPT-5.2 (copilot)".to_string())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_custom_agents_uses_default_model_when_manifest_omits_it() {
        let root = std::env::temp_dir().join(format!(
            "zagent-custom-agent-default-model-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".agents")).expect("create .agents");
        fs::write(
            root.join(".agents/reviewer.agent.md"),
            r#"---
name: Reviewer Agent
description: Reviews changes
user-invokable: true
---
Reviewer instructions.
"#,
        )
        .expect("write agent file");

        let agents =
            collect_custom_agents(&root.to_string_lossy(), "openrouter:minimax/minimax-m2.5");
        assert_eq!(agents.len(), 1);
        let agent = &agents[0];
        assert_eq!(agent.name, "Reviewer Agent");
        assert_eq!(agent.model, "openrouter:minimax/minimax-m2.5");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_manifest_id_field() {
        let root = std::env::temp_dir().join(format!(
            "zagent-custom-agent-id-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".agents")).expect("create .agents");
        fs::write(
            root.join(".agents/with-id.agent.md"),
            r#"---
id: not_allowed
name: With ID
description: Should be ignored
---
Body.
"#,
        )
        .expect("write agent file");

        let agents = collect_custom_agents(&root.to_string_lossy(), "minimax/minimax-m2.5");
        assert!(agents.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolves_explicit_user_invocation_by_name() {
        let agents = vec![CustomAgentDefinition {
            id: "rust_review_agent".to_string(),
            name: "Rust Review Agent".to_string(),
            description: "review".to_string(),
            model: "m".to_string(),
            user_invokable: true,
            invoke_default: false,
            tools: vec![],
            handoffs: vec![],
            instructions: "i".to_string(),
            tool_name: "handoff_rust_review_agent".to_string(),
            relative_path_from_cwd: ".agents/rust-review.md".to_string(),
        }];

        let (selected, msg, explicit) =
            resolve_user_invocation("@rust-review-agent check this", &agents);
        assert!(explicit);
        assert_eq!(msg, "check this");
        assert_eq!(
            selected.map(|a| a.name.clone()),
            Some("Rust Review Agent".to_string())
        );
    }

    #[test]
    fn resolves_default_invokable_agent() {
        let agents = vec![
            CustomAgentDefinition {
                id: "a".to_string(),
                name: "A".to_string(),
                description: "a".to_string(),
                model: "m".to_string(),
                user_invokable: true,
                invoke_default: true,
                tools: vec![],
                handoffs: vec![],
                instructions: "i".to_string(),
                tool_name: "handoff_a".to_string(),
                relative_path_from_cwd: ".agents/a.md".to_string(),
            },
            CustomAgentDefinition {
                id: "b".to_string(),
                name: "B".to_string(),
                description: "b".to_string(),
                model: "m".to_string(),
                user_invokable: true,
                invoke_default: false,
                tools: vec![],
                handoffs: vec![],
                instructions: "i".to_string(),
                tool_name: "handoff_b".to_string(),
                relative_path_from_cwd: ".agents/b.md".to_string(),
            },
        ];

        let (selected, msg, explicit) = resolve_user_invocation("plain prompt", &agents);
        assert!(!explicit);
        assert_eq!(msg, "plain prompt");
        assert_eq!(selected.map(|a| a.id.clone()), Some("a".to_string()));
    }

    #[test]
    fn resolves_allowed_runtime_tools_from_aliases() {
        let agents = vec![CustomAgentDefinition {
            id: "planning".to_string(),
            name: "planning".to_string(),
            description: "planner".to_string(),
            model: "m".to_string(),
            user_invokable: false,
            invoke_default: false,
            tools: vec![
                "search".to_string(),
                "fetch".to_string(),
                "read_fs".to_string(),
            ],
            handoffs: vec![],
            instructions: "i".to_string(),
            tool_name: "handoff_planning".to_string(),
            relative_path_from_cwd: ".agents/planning.md".to_string(),
        }];
        let allowed = resolve_allowed_runtime_tools(Some("planning"), &agents).expect("allowed");
        assert!(allowed.allows("websearch"));
        assert!(allowed.allows("webfetch"));
        assert!(allowed.allows("file_read"));
        assert!(allowed.allows("list_dir"));
    }

    #[test]
    fn supports_wildcard_tool_patterns() {
        let policy = ToolAccessPolicy::new(vec!["*".to_string(), "file/*".to_string()]);
        assert!(policy.allows("websearch"));
        assert!(policy.allows("file_read"));
        assert!(policy.allows("file_edit"));
    }

    #[test]
    fn supports_explicit_regex_tool_patterns() {
        let policy = ToolAccessPolicy::new(vec!["re:^file_(read|write)$".to_string()]);
        assert!(policy.allows("file_read"));
        assert!(policy.allows("file_write"));
        assert!(!policy.allows("file_edit"));
    }
}
