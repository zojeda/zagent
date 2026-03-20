pub mod conversation;
mod custom_agents;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{Instrument, info, info_span, trace, warn};

use crate::Result;
use crate::provider::configured::split_provider_model;
use crate::provider::types::{ChatRequest, ChatResponse, Message, ToolCall};
use crate::provider::{HttpClient, ProviderResolver};
use crate::session::{SessionEvent, SessionState, SessionStore, ToolExecutionRecord};
use crate::tools::ToolRegistry;
use custom_agents::{
    CustomAgentDefinition, CustomAgentHandoffDefinition, ToolAccessPolicy, collect_custom_agents,
    custom_agent_name_key, custom_agent_tool_definition, push_custom_agents_prompt_section,
    resolve_allowed_runtime_tools, resolve_handoff_scope, resolve_user_invocation,
};

/// Configuration for the agent loop
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub custom_agent_default_model: String,
    pub max_turns: u32,
    pub max_tool_output_chars: usize,
    pub system_prompt: String,
    pub active_custom_agent_id: Option<String>,
    pub handoff_depth: u32,
    pub visible_mcp_tools: Vec<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "minimax/minimax-m2.5".to_string(),
            custom_agent_default_model: "minimax/minimax-m2.5".to_string(),
            max_turns: 50,
            max_tool_output_chars: 50_000,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            active_custom_agent_id: None,
            handoff_depth: 0,
            visible_mcp_tools: Vec::new(),
        }
    }
}

const DEFAULT_SYSTEM_PROMPT: &str = r#"You are zAgent, a powerful coding assistant. You have access to tools that let you interact with the filesystem and execute shell commands.

When the user asks you to create, modify, or work with code:
1. Use the available tools to accomplish the task directly
2. Use shell_exec to run commands, compile code, or test programs
3. Use file_write to create or modify files
4. Use file_edit to apply targeted unified diffs to existing files
5. Use file_read to inspect existing files
6. Use list_dir to explore directory structures

Always prefer using tools to accomplish tasks rather than just describing what to do.
Think step by step and verify your work by reading back files or running tests.
When you encounter errors, debug them by reading error output and fixing issues.
Always tell the user what you're doing and why."#;

const AGENTS_FILE_NAME: &str = "AGENTS.md";
const MAX_AGENTS_FILES: usize = 64;
const MAX_AGENTS_FILE_BYTES: usize = 32_000;
const SKILL_FILE_NAME: &str = "SKILL.md";
const MAX_SKILL_FILES: usize = 128;
const MAX_SKILL_FILE_BYTES: usize = 32_000;
const WALK_SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", "dist", "logs"];
const MAX_HANDOFF_DEPTH: u32 = 16;

#[derive(Debug, Clone)]
struct AgentsInstructionFile {
    relative_path_from_cwd: String,
    content: String,
}

#[derive(Debug, Clone)]
struct SkillDefinition {
    name: String,
    description: String,
    relative_path_from_cwd: String,
}

fn build_effective_system_prompt(
    base_system_prompt: &str,
    working_dir: &str,
    custom_agents: &[CustomAgentDefinition],
) -> String {
    let instruction_files = collect_agents_instruction_files(working_dir);
    let skills = collect_skill_definitions(working_dir);
    let mut out = String::with_capacity(base_system_prompt.len() + 4096);
    out.push_str(base_system_prompt);

    if !instruction_files.is_empty() {
        out.push_str("\n\n# Rules\n");
        out.push_str(
            "The following always-on workspace rules were discovered from AGENTS.md files. \
Each block includes the path relative to the current working directory.\n\n",
        );

        for file in &instruction_files {
            out.push_str(&format!(
                "## Rule source: {}\n",
                file.relative_path_from_cwd
            ));
            out.push_str(file.content.trim());
            out.push_str("\n\n");
        }

        out.push_str(
            "Precedence rules: explicit user chat instructions override AGENTS.md instructions. \
When AGENTS.md files conflict, prioritize the file closest to the file being updated or \
processed. More specific (deeper, nearer) files override broader ones.",
        );
    }

    if !skills.is_empty() {
        out.push_str("\n\n# Available Skills\n");
        out.push_str(
            "Task-specific skills are available as external prompt files. Keep the base prompt \
lean: only read a skill with file_read when it is directly relevant to the task or the user \
explicitly asks for it. When you load a skill, follow its instructions in addition to the rules \
above.\n\n",
        );

        for skill in &skills {
            out.push_str(&format!(
                "- {}: {} [source: {}]\n",
                skill.name, skill.description, skill.relative_path_from_cwd
            ));
        }
    }

    push_custom_agents_prompt_section(&mut out, custom_agents);

    out
}

fn collect_agents_instruction_files(working_dir: &str) -> Vec<AgentsInstructionFile> {
    let cwd = resolve_path(working_dir);
    let root = find_git_root(&cwd);
    let mut discovered = Vec::new();
    let mut seen = HashSet::new();

    for path in collect_ancestor_agents_paths(&cwd, &root) {
        if seen.insert(path.clone()) {
            discovered.push(path);
        }
    }
    for path in collect_descendant_agents_paths(&cwd) {
        if seen.insert(path.clone()) {
            discovered.push(path);
        }
    }
    discovered.sort();
    discovered.truncate(MAX_AGENTS_FILES);

    discovered
        .into_iter()
        .filter_map(|path| {
            let bytes = fs::read(&path).ok()?;
            let clipped = if bytes.len() > MAX_AGENTS_FILE_BYTES {
                &bytes[..MAX_AGENTS_FILE_BYTES]
            } else {
                &bytes
            };
            let mut content = String::from_utf8_lossy(clipped).to_string();
            if bytes.len() > MAX_AGENTS_FILE_BYTES {
                content.push_str(&format!(
                    "\n\n[truncated at {} bytes]",
                    MAX_AGENTS_FILE_BYTES
                ));
            }

            let relative = path
                .strip_prefix(&cwd)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            Some(AgentsInstructionFile {
                relative_path_from_cwd: relative,
                content,
            })
        })
        .collect()
}

fn collect_ancestor_agents_paths(cwd: &Path, root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut cursor = cwd.to_path_buf();
    loop {
        let candidate = cursor.join(AGENTS_FILE_NAME);
        if candidate.is_file() {
            out.push(candidate);
        }
        if cursor == root {
            break;
        }
        if !cursor.pop() {
            break;
        }
    }
    out.reverse();
    out
}

fn collect_descendant_agents_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![cwd.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_AGENTS_FILES {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if file_type.is_file() && name_str == AGENTS_FILE_NAME {
                out.push(path);
                if out.len() >= MAX_AGENTS_FILES {
                    break;
                }
                continue;
            }
            if !file_type.is_dir() {
                continue;
            }
            if file_type.is_symlink() {
                continue;
            }
            if WALK_SKIP_DIRS.iter().any(|skip| name_str == *skip) {
                continue;
            }
            stack.push(path);
        }
    }

    out
}

fn collect_skill_definitions(working_dir: &str) -> Vec<SkillDefinition> {
    let cwd = resolve_path(working_dir);
    let mut discovered = collect_descendant_skill_paths(&cwd);
    discovered.sort();
    discovered.truncate(MAX_SKILL_FILES);

    discovered
        .into_iter()
        .filter_map(|path| load_skill_definition(&cwd, &path))
        .collect()
}

fn collect_descendant_skill_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![cwd.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_SKILL_FILES {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if file_type.is_file() && name_str == SKILL_FILE_NAME {
                out.push(path);
                if out.len() >= MAX_SKILL_FILES {
                    break;
                }
                continue;
            }
            if !file_type.is_dir() {
                continue;
            }
            if file_type.is_symlink() {
                continue;
            }
            if WALK_SKIP_DIRS.iter().any(|skip| name_str == *skip) {
                continue;
            }
            stack.push(path);
        }
    }

    out
}

fn load_skill_definition(cwd: &Path, path: &Path) -> Option<SkillDefinition> {
    let bytes = fs::read(path).ok()?;
    let clipped = if bytes.len() > MAX_SKILL_FILE_BYTES {
        &bytes[..MAX_SKILL_FILE_BYTES]
    } else {
        &bytes
    };
    let content = String::from_utf8_lossy(clipped).to_string();
    let (manifest, body) = parse_skill_frontmatter(&content);

    let name = manifest
        .name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| infer_skill_name(path));
    let description = manifest
        .description
        .filter(|value| !value.trim().is_empty())
        .or_else(|| infer_skill_description(&body))
        .unwrap_or_else(|| format!("Task-specific instructions for {name}"));

    let relative = path
        .strip_prefix(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string());

    Some(SkillDefinition {
        name,
        description,
        relative_path_from_cwd: relative,
    })
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SkillManifest {
    name: Option<String>,
    description: Option<String>,
}

fn parse_skill_frontmatter(content: &str) -> (SkillManifest, String) {
    let Some(rest) = content.strip_prefix("---\n") else {
        return (SkillManifest::default(), content.to_string());
    };
    let Some(end) = rest.find("\n---\n") else {
        return (SkillManifest::default(), content.to_string());
    };
    let meta_block = &rest[..end];
    let body = &rest[end + "\n---\n".len()..];
    let parsed = serde_yaml::from_str::<SkillManifest>(meta_block).unwrap_or_default();
    (parsed, body.to_string())
}

fn infer_skill_name(path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.file_name())
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Unnamed Skill".to_string())
}

fn infer_skill_description(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            line.trim_start_matches('#')
                .trim()
                .trim_end_matches('.')
                .to_string()
        })
        .filter(|line| !line.is_empty())
}

fn find_git_root(start: &Path) -> PathBuf {
    let mut cursor = start.to_path_buf();
    loop {
        if cursor.join(".git").exists() {
            return cursor;
        }
        if !cursor.pop() {
            return start.to_path_buf();
        }
    }
}

fn resolve_path(path: &str) -> PathBuf {
    let raw = PathBuf::from(path);
    if raw.is_absolute() {
        return raw;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(raw))
        .unwrap_or_else(|_| PathBuf::from(path))
}

/// Result of a single agent invocation
#[derive(Debug)]
pub struct AgentResult {
    pub response: String,
    pub turns: u32,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub last_prompt_tokens: u64,
    pub total_cached_prompt_tokens: u64,
    pub total_cost_usd: f64,
    pub credits_remaining: Option<f64>,
    pub tool_calls_count: u32,
}

#[derive(Debug, Clone)]
pub enum AgentProgressEvent {
    ModelRequestStarted {
        agent: String,
        handoff_depth: u32,
        turn: u32,
        provider: String,
        model: String,
        message_count: usize,
        tool_count: usize,
        request_payload: serde_json::Value,
    },
    ModelResponseReceived {
        agent: String,
        handoff_depth: u32,
        turn: u32,
        finish_reason: Option<String>,
        latency_ms: u64,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        total_tokens: Option<u64>,
        cached_prompt_tokens: Option<u64>,
        cost_usd: Option<f64>,
        credits_remaining: Option<f64>,
        response_payload: serde_json::Value,
    },
    ToolCallStarted {
        agent: String,
        handoff_depth: u32,
        tool_name: String,
        arguments: String,
    },
    ToolCallFinished {
        agent: String,
        handoff_depth: u32,
        tool_name: String,
        success: bool,
        latency_ms: u64,
        result: String,
    },
}

#[derive(Clone, Copy)]
struct ProgressEmitter<'a>(*mut (dyn FnMut(AgentProgressEvent) + Send + 'a));

impl<'a> ProgressEmitter<'a> {
    fn new(cb: &mut (dyn FnMut(AgentProgressEvent) + Send + 'a)) -> Self {
        Self(cb as *mut _)
    }

    fn emit(self, event: AgentProgressEvent) {
        // SAFETY: pointer originates from the callback reference passed to this
        // run and is invoked synchronously on the same task.
        unsafe { (*self.0)(event) };
    }
}

unsafe impl<'a> Send for ProgressEmitter<'a> {}
unsafe impl<'a> Sync for ProgressEmitter<'a> {}

struct ToolExecutionOutcome {
    output: String,
    latency_ms: u64,
    delegated_cost_usd: f64,
    delegated_credits_remaining: Option<f64>,
    forwarded_events: Vec<AgentProgressEvent>,
}

fn resolve_model_and_provider(
    configured_model: &str,
    providers: &dyn ProviderResolver,
    fallback_provider_name: &str,
) -> Result<(String, String)> {
    if let Some((provider_name, model_name)) = split_provider_model(configured_model) {
        if providers.get(provider_name).is_none() {
            return Err(crate::Error::config(format!(
                "Provider '{provider_name}' is not configured for model '{configured_model}'"
            )));
        }
        return Ok((provider_name.to_string(), model_name.to_string()));
    }

    if providers.get(fallback_provider_name).is_none() {
        return Err(crate::Error::config(format!(
            "Provider '{fallback_provider_name}' is not configured"
        )));
    }

    Ok((
        fallback_provider_name.to_string(),
        configured_model.to_string(),
    ))
}

/// Run the agentic loop: send messages to LLM, execute tool calls, repeat until done.
pub async fn run_agent_loop(
    http_client: &dyn HttpClient,
    providers: &dyn ProviderResolver,
    tools: &ToolRegistry,
    session: &mut SessionState,
    session_store: Option<&dyn SessionStore>,
    config: &AgentConfig,
    user_message: &str,
) -> Result<AgentResult> {
    run_agent_loop_with_progress(
        http_client,
        providers,
        tools,
        session,
        session_store,
        config,
        user_message,
        None,
    )
    .await
}

pub async fn run_agent_loop_with_progress(
    http_client: &dyn HttpClient,
    providers: &dyn ProviderResolver,
    tools: &ToolRegistry,
    session: &mut SessionState,
    session_store: Option<&dyn SessionStore>,
    config: &AgentConfig,
    user_message: &str,
    progress: Option<&mut (dyn FnMut(AgentProgressEvent) + Send)>,
) -> Result<AgentResult> {
    let progress_emitter = progress.map(ProgressEmitter::new);
    let custom_agents_all =
        collect_custom_agents(&session.working_dir, &config.custom_agent_default_model);
    let mut effective_config = config.clone();
    let mut effective_user_message = user_message.to_string();
    let mut effective_provider_name = providers.default_provider_name().to_string();
    if effective_config.active_custom_agent_id.is_none() {
        let (selected, routed_message, _explicit) =
            resolve_user_invocation(user_message, &custom_agents_all);
        effective_user_message = routed_message;
        if let Some(agent) = selected {
            effective_config.active_custom_agent_id = Some(agent.id.clone());
            let (provider_name, model_name) =
                resolve_model_and_provider(&agent.model, providers, &effective_provider_name)?;
            effective_provider_name = provider_name;
            effective_config.model = model_name;

            let mut routed_prompt = format!(
                "{}\n\n# Invoked Agent Role\nName: {}\nDescription: {}\n\n{}",
                config.system_prompt, agent.name, agent.description, agent.instructions
            );
            if !agent.tools.is_empty() {
                routed_prompt.push_str("\n\n# Declared Tools\n");
                routed_prompt
                    .push_str("The agent manifest declares preferred tool groups/capabilities:\n");
                for tool in &agent.tools {
                    routed_prompt.push_str(&format!("- {tool}\n"));
                }
            }
            effective_config.system_prompt = routed_prompt;
        }
    }

    let session_message_count_before = session.messages.len();
    let is_follow_up = session_message_count_before > 0;
    let custom_agents: Vec<CustomAgentDefinition> = resolve_handoff_scope(
        effective_config.active_custom_agent_id.as_deref(),
        &custom_agents_all,
    )
    .into_iter()
    .cloned()
    .collect();
    let allowed_runtime_tools = resolve_allowed_runtime_tools(
        effective_config.active_custom_agent_id.as_deref(),
        &custom_agents_all,
    );
    let mut handoff_defaults_by_tool: HashMap<String, CustomAgentHandoffDefinition> =
        HashMap::new();
    let active_agent_name = effective_config
        .active_custom_agent_id
        .as_deref()
        .and_then(|id| custom_agents_all.iter().find(|a| a.id == id))
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "root".to_string());
    if let Some(active_id) = effective_config.active_custom_agent_id.as_deref()
        && let Some(active_agent) = custom_agents_all.iter().find(|a| a.id == active_id)
    {
        for handoff in &active_agent.handoffs {
            if let Some(target) = custom_agents_all
                .iter()
                .find(|a| custom_agent_name_key(&a.name) == custom_agent_name_key(&handoff.agent))
            {
                handoff_defaults_by_tool.insert(target.tool_name.clone(), handoff.clone());
            }
        }
    }
    let custom_agent_tools: HashMap<String, CustomAgentDefinition> = custom_agents
        .iter()
        .cloned()
        .map(|agent| (agent.tool_name.clone(), agent))
        .collect();
    let loop_span = info_span!(
        "agent_loop",
        session_id = %session.meta.id,
        provider = %effective_provider_name,
        model = %effective_config.model,
        active_custom_agent = %effective_config.active_custom_agent_id.as_deref().unwrap_or("root"),
        handoff_depth = effective_config.handoff_depth,
        is_handoff_child = effective_config.handoff_depth > 0,
        user_input_len = effective_user_message.len(),
        session_message_count_before = session_message_count_before,
        is_follow_up = is_follow_up
    );
    async move {
        let user_request_span = info_span!(
            "user_initiated",
            session_id = %session.meta.id,
            provider = %effective_provider_name,
            model = %effective_config.model,
            active_custom_agent = %effective_config.active_custom_agent_id.as_deref().unwrap_or("root"),
            handoff_depth = effective_config.handoff_depth,
            user_message_len = effective_user_message.len(),
            session_message_count_before = session_message_count_before,
            is_follow_up = is_follow_up
        );

        async {
            trace!(
                target: "zagent::agent_loop_start",
                session_id = %session.meta.id,
                model = %effective_config.model,
                session_message_count_before = session_message_count_before,
                is_follow_up = is_follow_up,
                user_message_len = effective_user_message.len(),
                user_message = %effective_user_message,
                "User initiated agent loop"
            );

            let mut effective_system_prompt = build_effective_system_prompt(
                &effective_config.system_prompt,
                &session.working_dir,
                &custom_agents,
            );
            if let Some(allowed) = &allowed_runtime_tools {
                let resolved_runtime_tools = allowed.resolve_allowed_names(tools.tool_names());
                effective_system_prompt.push_str("\n\n# Available Runtime Tools\n");
                effective_system_prompt
                    .push_str("Tool access is restricted by the active agent policy.\n");
                effective_system_prompt.push_str("Allowed patterns:\n");
                for pattern in allowed.patterns() {
                    effective_system_prompt.push_str(&format!("- {pattern}\n"));
                }
                effective_system_prompt.push_str("Resolved runtime tools:\n");
                for tool_name in resolved_runtime_tools {
                    effective_system_prompt.push_str(&format!("- {tool_name}\n"));
                }
            } else if !effective_config.visible_mcp_tools.is_empty() {
                effective_system_prompt.push_str("\n\n# Available MCP Tools\n");
                effective_system_prompt
                    .push_str("The following MCP tools are currently connected and available:\n");
                for tool_name in &effective_config.visible_mcp_tools {
                    effective_system_prompt.push_str(&format!("- {tool_name}\n"));
                }
            }

            // Add user message to session
            let user_msg = Message::user(&effective_user_message);
            session.add_message(user_msg.clone());
            append_canonical_message_event(
                session_store,
                &session.meta.id,
                "user_message_added",
                &active_agent_name,
                effective_config.handoff_depth,
                None,
                &user_msg,
            )
            .await;

            let mut total_prompt_tokens: u64 = 0;
            let mut total_completion_tokens: u64 = 0;
            let mut last_prompt_tokens: u64 = 0;
            let mut total_cached_prompt_tokens: u64 = 0;
            let mut total_cost_usd: f64 = 0.0;
            let mut credits_remaining: Option<f64> = None;
            let mut total_tool_calls: u32 = 0;
            let mut turn = 0;

            loop {
                turn += 1;
                if turn > effective_config.max_turns {
                    warn!(
                        max_turns = effective_config.max_turns,
                        "Agent reached maximum turns limit"
                    );
                    return Err(crate::Error::custom(format!(
                        "Agent reached maximum turns limit ({})",
                        effective_config.max_turns
                    )));
                }

                let turn_span = info_span!("agent_turn", turn = turn);

                let response = async {
                    // Build the chat request
                    let mut messages = vec![Message::system(&effective_system_prompt)];
                    messages.extend(session.messages.clone());

                    let mut tool_defs = tools.definitions();
                    if let Some(allowed) = &allowed_runtime_tools {
                        tool_defs.retain(|td| allowed.allows(&td.function.name));
                    }
                    tool_defs.extend(custom_agents.iter().map(custom_agent_tool_definition));
                    let request =
                        ChatRequest::new(&effective_config.model, messages).with_tools(tool_defs);
                    let tool_count = request.tools.as_ref().map(|t| t.len()).unwrap_or(0);
                    let request_payload = serde_json::to_value(&request)?;
                    let active_provider =
                        providers
                            .get(&effective_provider_name)
                            .ok_or_else(|| {
                                crate::Error::config(format!(
                                    "Provider '{}' is not configured",
                                    effective_provider_name
                                ))
                            })?;
                    let model_call_span = info_span!(
                        "model_call",
                        provider = %active_provider.name(),
                        model = %effective_config.model,
                        turn = turn,
                        message_count = request.messages.len(),
                        tool_count = tool_count
                    );

                    // Log the full request at TRACE level for observability
                    let request_json = serde_json::to_string_pretty(&request_payload)?;
                    trace!(
                        target: "zagent::llm_request",
                        request_body = %request_json,
                        "Full LLM request payload"
                    );

                    async {
                        // Call the model provider.
                        let llm_start = Instant::now();
                        let http_req = active_provider.build_http_request(&request)?;
                        emit_progress_event(
                            progress_emitter,
                            session_store,
                            &session.meta.id,
                            AgentProgressEvent::ModelRequestStarted {
                                agent: active_agent_name.clone(),
                                handoff_depth: effective_config.handoff_depth,
                                turn,
                                provider: active_provider.name().to_string(),
                                model: effective_config.model.clone(),
                                message_count: request.messages.len(),
                                tool_count,
                                request_payload: request_payload.clone(),
                            },
                        )
                        .await;

                        info!("→ Model request");

                        let http_resp = http_client.send(http_req).await?;
                        let llm_latency_ms = llm_start.elapsed().as_millis() as u64;
                        let response_payload =
                            serde_json::from_str::<serde_json::Value>(&http_resp.body)
                                .unwrap_or_else(|_| {
                                    serde_json::json!({
                                        "raw_text": http_resp.body.clone()
                                    })
                                });

                        // Log the full response at TRACE level
                        trace!(
                            target: "zagent::llm_response",
                            response_body = %http_resp.body,
                            "Full LLM response payload"
                        );

                        // Check for HTTP errors
                        if http_resp.status >= 400 {
                            // Try to parse as API error
                            if let Ok(api_err) = serde_json::from_str::<
                                crate::provider::types::ApiErrorResponse,
                            >(&http_resp.body)
                            {
                                return Err(crate::Error::api(
                                    http_resp.status,
                                    api_err.error.message,
                                ));
                            }
                            return Err(crate::Error::api(
                                http_resp.status,
                                format!(
                                    "HTTP {} — {}",
                                    http_resp.status,
                                    &http_resp.body[..http_resp.body.len().min(500)]
                                ),
                            ));
                        }

                        // Parse response
                        let chat_response: ChatResponse =
                            active_provider.parse_response(&http_resp.body)?;
                        let response_credits = extract_credits_remaining(&http_resp.headers);
                        credits_remaining = response_credits.or(credits_remaining);

                        // Log usage
                        if let Some(ref usage) = chat_response.usage {
                            let cached_tokens = usage.cached_tokens();
                            let usage_cost = usage.cost.unwrap_or(0.0);
                            info!(
                                prompt_tokens = usage.prompt_tokens,
                                completion_tokens = usage.completion_tokens,
                                total_tokens = usage.total_tokens,
                                cached_prompt_tokens = cached_tokens,
                                cost_usd = usage_cost,
                                latency_ms = llm_latency_ms,
                                finish_reason = ?chat_response.finish_reason(),
                                "← Model response"
                            );
                            total_prompt_tokens += usage.prompt_tokens;
                            total_completion_tokens += usage.completion_tokens;
                            last_prompt_tokens = usage.prompt_tokens;
                            total_cached_prompt_tokens += cached_tokens;
                            total_cost_usd += usage_cost;
                            session
                                .update_token_usage(usage.prompt_tokens, usage.completion_tokens);
                            emit_progress_event(
                                progress_emitter,
                                session_store,
                                &session.meta.id,
                                AgentProgressEvent::ModelResponseReceived {
                                    agent: active_agent_name.clone(),
                                    handoff_depth: effective_config.handoff_depth,
                                    turn,
                                    finish_reason: chat_response
                                        .finish_reason()
                                        .map(str::to_string),
                                    latency_ms: llm_latency_ms,
                                    prompt_tokens: Some(usage.prompt_tokens),
                                    completion_tokens: Some(usage.completion_tokens),
                                    total_tokens: Some(usage.total_tokens),
                                    cached_prompt_tokens: Some(cached_tokens),
                                    cost_usd: Some(usage_cost),
                                    credits_remaining,
                                    response_payload: response_payload.clone(),
                                },
                            )
                            .await;
                        } else {
                            info!(
                                latency_ms = llm_latency_ms,
                                finish_reason = ?chat_response.finish_reason(),
                                "← Model response (no usage data)"
                            );
                            emit_progress_event(
                                progress_emitter,
                                session_store,
                                &session.meta.id,
                                AgentProgressEvent::ModelResponseReceived {
                                    agent: active_agent_name.clone(),
                                    handoff_depth: effective_config.handoff_depth,
                                    turn,
                                    finish_reason: chat_response
                                        .finish_reason()
                                        .map(str::to_string),
                                    latency_ms: llm_latency_ms,
                                    prompt_tokens: None,
                                    completion_tokens: None,
                                    total_tokens: None,
                                    cached_prompt_tokens: None,
                                    cost_usd: None,
                                    credits_remaining,
                                    response_payload: response_payload.clone(),
                                },
                            )
                            .await;
                        }

                        Ok(chat_response)
                    }
                    .instrument(model_call_span)
                    .await
                }
                .instrument(turn_span)
                .await?;

                // Check if the model wants to use tools
                if response.has_tool_calls() {
                    let tool_calls = response.tool_calls().unwrap().clone();
                    let reasoning_details = response
                        .choices
                        .first()
                        .and_then(|c| c.message.reasoning_details.clone());

                    // Add assistant message with tool calls to session
                    let assistant_msg = Message::assistant_with_tool_calls(
                        response.content().map(|s| s.to_string()),
                        tool_calls.clone(),
                    )
                    .with_reasoning_details(reasoning_details);
                    session.add_message(assistant_msg.clone());
                    append_canonical_message_event(
                        session_store,
                        &session.meta.id,
                        "assistant_message_added",
                        &active_agent_name,
                        effective_config.handoff_depth,
                        Some(turn),
                        &assistant_msg,
                    )
                    .await;

                    // If the assistant also had text content, print it
                    if let Some(text) = response.content()
                        && !text.trim().is_empty()
                    {
                        info!(content = %text, "Assistant (thinking)");
                    }

                    // Execute each tool call
                    for tc in &tool_calls {
                        total_tool_calls += 1;
                        let is_handoff_tool_call = custom_agent_tools.contains_key(&tc.function.name);
                        emit_progress_event(
                            progress_emitter,
                            session_store,
                            &session.meta.id,
                            AgentProgressEvent::ToolCallStarted {
                                agent: active_agent_name.clone(),
                                handoff_depth: effective_config.handoff_depth,
                                tool_name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            },
                        )
                        .await;
                        let tool_result = execute_tool_call(
                            http_client,
                            providers,
                            tools,
                            tc,
                            effective_config.max_tool_output_chars,
                            &custom_agent_tools,
                            allowed_runtime_tools.as_ref(),
                            &handoff_defaults_by_tool,
                            progress_emitter,
                            &effective_config,
                            &effective_provider_name,
                            &session.working_dir,
                            session_store,
                            &session.meta.id,
                        )
                        .await;

                        let result_str = match &tool_result {
                            Ok(outcome) => {
                                for event in &outcome.forwarded_events {
                                    emit_progress_event(
                                        progress_emitter,
                                        session_store,
                                        &session.meta.id,
                                        event.clone(),
                                    )
                                    .await;
                                }
                                // Record tool execution
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc.function.arguments)
                                        .unwrap_or(serde_json::Value::Null);
                                let tool_execution = ToolExecutionRecord {
                                    id: tc.id.clone(),
                                    tool_name: tc.function.name.clone(),
                                    arguments: args,
                                    result: outcome.output.clone(),
                                    success: true,
                                    latency_ms: outcome.latency_ms,
                                    created_at: Utc::now(),
                                };
                                session.record_tool_execution(tool_execution.clone());
                                total_cost_usd += outcome.delegated_cost_usd;
                                credits_remaining =
                                    outcome.delegated_credits_remaining.or(credits_remaining);
                                emit_progress_event(
                                    progress_emitter,
                                    session_store,
                                    &session.meta.id,
                                    AgentProgressEvent::ToolCallFinished {
                                        agent: active_agent_name.clone(),
                                        handoff_depth: effective_config.handoff_depth,
                                        tool_name: tc.function.name.clone(),
                                        success: true,
                                        latency_ms: outcome.latency_ms,
                                        result: outcome.output.clone(),
                                    },
                                )
                                .await;
                                append_tool_result_event(
                                    session_store,
                                    &session.meta.id,
                                    &active_agent_name,
                                    effective_config.handoff_depth,
                                    Some(turn),
                                    &Message::tool_result(&tc.id, &outcome.output),
                                    &tool_execution,
                                )
                                .await;
                                outcome.output.clone()
                            }
                            Err(e) => {
                                let err_msg = format!("Error: {e}");
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc.function.arguments)
                                        .unwrap_or(serde_json::Value::Null);
                                let tool_execution = ToolExecutionRecord {
                                    id: tc.id.clone(),
                                    tool_name: tc.function.name.clone(),
                                    arguments: args,
                                    result: err_msg.clone(),
                                    success: false,
                                    latency_ms: 0,
                                    created_at: Utc::now(),
                                };
                                session.record_tool_execution(tool_execution.clone());
                                emit_progress_event(
                                    progress_emitter,
                                    session_store,
                                    &session.meta.id,
                                    AgentProgressEvent::ToolCallFinished {
                                        agent: active_agent_name.clone(),
                                        handoff_depth: effective_config.handoff_depth,
                                        tool_name: tc.function.name.clone(),
                                        success: false,
                                        latency_ms: 0,
                                        result: err_msg.clone(),
                                    },
                                )
                                .await;
                                append_tool_result_event(
                                    session_store,
                                    &session.meta.id,
                                    &active_agent_name,
                                    effective_config.handoff_depth,
                                    Some(turn),
                                    &Message::tool_result(&tc.id, &err_msg),
                                    &tool_execution,
                                )
                                .await;
                                err_msg
                            }
                        };

                        // Add tool result message to conversation
                        if is_handoff_tool_call {
                            let handoff_visibility_span = tracing::trace_span!(
                                target: "zagent::handoff_visibility",
                                "handoff_visibility",
                                parent_agent = %active_agent_name,
                                parent_handoff_depth = effective_config.handoff_depth,
                                handoff_tool = %tc.function.name,
                                tool_call_id = %tc.id,
                                output_len = result_str.len()
                            );
                            let _handoff_visibility_guard = handoff_visibility_span.enter();
                            trace!(
                                target: "zagent::handoff_visibility",
                                output = %truncate_for_log(&result_str, 2_000),
                                "Parent agent consumed handoff output and appended it as a tool_result message for the next model turn"
                            );
                        }
                        let tool_msg = Message::tool_result(&tc.id, &result_str);
                        session.add_message(tool_msg);
                    }

                    append_conversation_checkpoint(
                        session_store,
                        &session.meta.id,
                        &active_agent_name,
                        effective_config.handoff_depth,
                        Some(turn),
                        session,
                    )
                    .await;

                    // Save session after tool executions
                    if let Some(store) = session_store
                        && let Err(e) = store.save_session(session).await
                    {
                        warn!(error = %e, "Failed to save session");
                    }

                    // Continue the loop — send tool results back to LLM
                    continue;
                }

                // No tool calls — we have the final response
                let final_response = response
                    .content()
                    .unwrap_or("[No response content]")
                    .to_string();
                let reasoning_details = response
                    .choices
                    .first()
                    .and_then(|c| c.message.reasoning_details.clone());

                // Add assistant message to session
                let assistant_msg =
                    Message::assistant(&final_response).with_reasoning_details(reasoning_details);
                session.add_message(assistant_msg.clone());
                append_canonical_message_event(
                    session_store,
                    &session.meta.id,
                    "assistant_message_added",
                    &active_agent_name,
                    effective_config.handoff_depth,
                    Some(turn),
                    &assistant_msg,
                )
                .await;

                append_conversation_checkpoint(
                    session_store,
                    &session.meta.id,
                    &active_agent_name,
                    effective_config.handoff_depth,
                    Some(turn),
                    session,
                )
                .await;

                // Save session
                if let Some(store) = session_store
                    && let Err(e) = store.save_session(session).await
                {
                    warn!(error = %e, "Failed to save session");
                }

                info!(
                    turns = turn,
                    total_prompt_tokens = total_prompt_tokens,
                    total_completion_tokens = total_completion_tokens,
                    total_cached_prompt_tokens = total_cached_prompt_tokens,
                    total_cost_usd = total_cost_usd,
                    credits_remaining = ?credits_remaining,
                    tool_calls = total_tool_calls,
                    "Agent loop complete"
                );

                return Ok(AgentResult {
                    response: final_response,
                    turns: turn,
                    total_prompt_tokens,
                    total_completion_tokens,
                    last_prompt_tokens,
                    total_cached_prompt_tokens,
                    total_cost_usd,
                    credits_remaining,
                    tool_calls_count: total_tool_calls,
                });
            }
        }
        .instrument(user_request_span)
        .await
    }
    .instrument(loop_span)
    .await
}

/// Execute a single tool call and return (output, latency_ms) or error
async fn execute_tool_call(
    http_client: &dyn HttpClient,
    providers: &dyn ProviderResolver,
    tools: &ToolRegistry,
    tool_call: &ToolCall,
    max_output_chars: usize,
    custom_agent_tools: &HashMap<String, CustomAgentDefinition>,
    allowed_runtime_tools: Option<&ToolAccessPolicy>,
    handoff_defaults_by_tool: &HashMap<String, CustomAgentHandoffDefinition>,
    _progress_emitter: Option<ProgressEmitter<'_>>,
    parent_config: &AgentConfig,
    parent_provider_name: &str,
    working_dir: &str,
    parent_session_store: Option<&dyn SessionStore>,
    parent_session_id: &str,
) -> Result<ToolExecutionOutcome> {
    let tool_name = &tool_call.function.name;
    let is_handoff_tool = custom_agent_tools.contains_key(tool_name);
    let tool_call_span_name = if is_handoff_tool {
        format!("tool_call_handoff <{tool_name}>")
    } else {
        format!("tool_call <{tool_name}>")
    };
    let tool_call_span = info_span!(
        "tool_call",
        span_name = %tool_call_span_name,
        tool = %tool_name,
        tool_type = if is_handoff_tool { "handoff" } else { "runtime" },
        parent_agent = %parent_config.active_custom_agent_id.as_deref().unwrap_or("root"),
        handoff_depth = parent_config.handoff_depth,
        is_handoff_child = parent_config.handoff_depth > 0
    );
    async {
        if let Some(child_agent) = custom_agent_tools.get(tool_name) {
            if parent_config.handoff_depth >= MAX_HANDOFF_DEPTH {
                return Err(crate::Error::tool(
                    tool_name,
                    format!(
                        "Maximum handoff depth reached ({MAX_HANDOFF_DEPTH}). Possible delegation loop."
                    ),
                ));
            }

            let args: serde_json::Value =
                serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
                    crate::Error::tool(tool_name, format!("Invalid JSON arguments: {e}"))
                })?;

            let task = args
                .get("task")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    crate::Error::tool(tool_name, "Missing required 'task' string argument")
                })?;
            let context = args
                .get("context")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let handoff_defaults = handoff_defaults_by_tool.get(tool_name);

            let mut child_request = task.to_string();
            let send_context = handoff_defaults.and_then(|h| h.send).unwrap_or(true);
            if send_context && let Some(ctx) = context {
                child_request.push_str("\n\nAdditional context from parent agent:\n");
                child_request.push_str(ctx);
            }
            if let Some(prompt) = handoff_defaults.and_then(|h| h.prompt.as_deref()) {
                child_request.push_str("\n\nHandoff prompt:\n");
                child_request.push_str(prompt);
            }

            let mut child_prompt = format!(
                "{}\n\n# Child Agent Role\nName: {}\nDescription: {}\n\n{}",
                parent_config.system_prompt,
                child_agent.name,
                child_agent.description,
                child_agent.instructions
            );
            if !child_agent.tools.is_empty() {
                child_prompt.push_str("\n\n# Declared Tools\n");
                child_prompt.push_str(
                    "The agent manifest declares preferred tool groups/capabilities:\n",
                );
                for tool in &child_agent.tools {
                    child_prompt.push_str(&format!("- {tool}\n"));
                }
            }
            let child_config = AgentConfig {
                model: handoff_defaults
                    .and_then(|h| h.model.clone())
                    .unwrap_or_else(|| child_agent.model.clone()),
                custom_agent_default_model: parent_config.custom_agent_default_model.clone(),
                max_turns: parent_config.max_turns,
                max_tool_output_chars: parent_config.max_tool_output_chars,
                system_prompt: child_prompt,
                active_custom_agent_id: Some(child_agent.id.clone()),
                handoff_depth: parent_config.handoff_depth.saturating_add(1),
                visible_mcp_tools: parent_config.visible_mcp_tools.clone(),
            };
            let (child_provider_name, child_model_name) =
                resolve_model_and_provider(&child_config.model, providers, parent_provider_name)?;
            let child_config = AgentConfig {
                model: child_model_name,
                ..child_config
            };

            let handoff_span_name = format!("agent_handoff <{}>", child_agent.name);
            let handoff_span = info_span!(
                "agent_handoff",
                span_name = %handoff_span_name,
                handoff_tool = %tool_name,
                parent_agent = %parent_config.active_custom_agent_id.as_deref().unwrap_or("root"),
                parent_handoff_depth = parent_config.handoff_depth,
                child_agent_id = %child_agent.id,
                child_agent_name = %child_agent.name,
                child_provider = %child_provider_name,
                child_model = %child_config.model
            );

            let mut child_session = SessionState::new(
                format!("handoff-{}", child_agent.id),
                child_config.model.clone(),
                child_provider_name.clone(),
                child_config.system_prompt.clone(),
                working_dir.to_string(),
            );

            let start = Instant::now();
            let mut child_events = Vec::new();
            let persist_child_events_inline = parent_session_store.is_some();
            let mut child_progress = |event: AgentProgressEvent| {
                if let Some(emitter) = _progress_emitter {
                    emitter.emit(event.clone());
                }
                if !persist_child_events_inline {
                    child_events.push(event);
                }
            };
            let forwarding_store = parent_session_store.map(|store| ChildEventForwardingStore {
                inner: store,
                parent_session_id: parent_session_id.to_string(),
            });
            let child_result = async {
                Box::pin(run_agent_loop_with_progress(
                    http_client,
                    providers,
                    tools,
                    &mut child_session,
                    forwarding_store
                        .as_ref()
                        .map(|s| s as &dyn SessionStore),
                    &child_config,
                    &child_request,
                    Some(&mut child_progress),
                ))
                .await
            }
            .instrument(handoff_span)
            .await?;
            let latency_ms = start.elapsed().as_millis() as u64;

            let output = format!(
                "{}\n\n[handoff agent={} turns={} tools={} prompt_tokens={} completion_tokens={} cached_prompt_tokens={} cost_usd={:.6}]",
                child_result.response,
                child_agent.name,
                child_result.turns,
                child_result.tool_calls_count,
                child_result.total_prompt_tokens,
                child_result.total_completion_tokens,
                child_result.total_cached_prompt_tokens,
                child_result.total_cost_usd
            );
            return Ok(ToolExecutionOutcome {
                output,
                latency_ms,
                delegated_cost_usd: child_result.total_cost_usd,
                delegated_credits_remaining: child_result.credits_remaining,
                forwarded_events: child_events,
            });
        }

        let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
            .map_err(|e| crate::Error::tool(tool_name, format!("Invalid JSON arguments: {e}")))?;
        if let Some(allowed) = allowed_runtime_tools
            && !allowed.allows(tool_name)
        {
            return Err(crate::Error::tool(
                tool_name,
                format!("Tool not allowed for active agent: {tool_name}"),
            ));
        }

        info!(
            arguments = %serde_json::to_string_pretty(&args).unwrap_or_else(|_| args.to_string()),
            "⚡ Tool call"
        );

        let start = Instant::now();
        let result = match tools.execute(tool_name, args).await {
            Ok(result) => result,
            Err(e) => {
                warn!(error = %e, "Tool execution failed");
                return Err(e);
            }
        };
        let latency_ms = start.elapsed().as_millis() as u64;

        // Truncate output for display/logging if too large
        let display_result = if result.chars().count() > max_output_chars {
            format!(
                "{}...\n[truncated — {} total chars]",
                truncate_to_chars(&result, max_output_chars),
                result.chars().count()
            )
        } else {
            result.clone()
        };

        info!(
            latency_ms = latency_ms,
            output_len = result.len(),
            output = %truncate_for_log(&display_result, 500),
            "✓ Tool result"
        );

        Ok(ToolExecutionOutcome {
            output: result,
            latency_ms,
            delegated_cost_usd: 0.0,
            delegated_credits_remaining: None,
            forwarded_events: Vec::new(),
        })
    }
    .instrument(tool_call_span)
    .await
}

async fn emit_progress_event(
    progress_emitter: Option<ProgressEmitter<'_>>,
    session_store: Option<&dyn SessionStore>,
    session_id: &str,
    event: AgentProgressEvent,
) {
    if let Some(emitter) = progress_emitter {
        emitter.emit(event.clone());
    }

    if let Some(store) = session_store {
        let persisted = progress_event_to_session_event(session_id, &event);
        if let Err(err) = store.append_event(&persisted).await {
            warn!(error = %err, "Failed to append session event");
        }
    }
}

fn progress_event_to_session_event(session_id: &str, event: &AgentProgressEvent) -> SessionEvent {
    match event {
        AgentProgressEvent::ModelRequestStarted {
            agent,
            handoff_depth,
            turn,
            provider,
            model,
            message_count,
            tool_count,
            request_payload,
        } => {
            let mut event = SessionEvent::new(
                session_id.to_string(),
                "model_request_started",
                agent.clone(),
                *handoff_depth,
                Some(*turn),
                serde_json::json!({
                    "provider": provider,
                    "model": model,
                    "message_count": message_count,
                    "tool_count": tool_count,
                    "json_detail": {
                        "request_payload": request_payload
                    }
                }),
            );
            event.phase = Some("request_started".to_string());
            event.provider = Some(provider.clone());
            event.model = Some(model.clone());
            event
        }
        AgentProgressEvent::ModelResponseReceived {
            agent,
            handoff_depth,
            turn,
            finish_reason,
            latency_ms,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_prompt_tokens,
            cost_usd,
            credits_remaining,
            response_payload,
        } => {
            let mut event = SessionEvent::new(
                session_id.to_string(),
                "model_response_received",
                agent.clone(),
                *handoff_depth,
                Some(*turn),
                serde_json::json!({
                    "finish_reason": finish_reason,
                    "latency_ms": latency_ms,
                    "prompt_tokens": prompt_tokens,
                    "completion_tokens": completion_tokens,
                    "total_tokens": total_tokens,
                    "cached_prompt_tokens": cached_prompt_tokens,
                    "cost_usd": cost_usd,
                    "credits_remaining": credits_remaining,
                    "json_detail": {
                        "response_payload": response_payload
                    }
                }),
            );
            event.phase = Some("response_received".to_string());
            event.finish_reason = finish_reason.clone();
            event.latency_ms = Some(*latency_ms);
            event.prompt_tokens = *prompt_tokens;
            event.completion_tokens = *completion_tokens;
            event.total_tokens = *total_tokens;
            event.cached_prompt_tokens = *cached_prompt_tokens;
            event.cost_usd = *cost_usd;
            event.credits_remaining = *credits_remaining;
            event
        }
        AgentProgressEvent::ToolCallStarted {
            agent,
            handoff_depth,
            tool_name,
            arguments,
        } => {
            let mut event = SessionEvent::new(
                session_id.to_string(),
                "tool_call_started",
                agent.clone(),
                *handoff_depth,
                None,
                serde_json::json!({
                    "tool_name": tool_name,
                    "arguments": arguments,
                    "json_detail": {
                        "tool_name": tool_name,
                        "arguments": arguments
                    }
                }),
            );
            event.phase = Some("start".to_string());
            event.tool_name = Some(tool_name.clone());
            event.arguments = Some(arguments.clone());
            event
        }
        AgentProgressEvent::ToolCallFinished {
            agent,
            handoff_depth,
            tool_name,
            success,
            latency_ms,
            result,
        } => {
            let mut event = SessionEvent::new(
                session_id.to_string(),
                "tool_call_finished",
                agent.clone(),
                *handoff_depth,
                None,
                serde_json::json!({
                    "tool_name": tool_name,
                    "success": success,
                    "latency_ms": latency_ms,
                    "result": result,
                    "json_detail": {
                        "tool_name": tool_name,
                        "success": success,
                        "latency_ms": latency_ms,
                        "result": result
                    }
                }),
            );
            event.phase = Some("finish".to_string());
            event.tool_name = Some(tool_name.clone());
            event.success = Some(*success);
            event.latency_ms = Some(*latency_ms);
            event.result = Some(result.clone());
            event
        }
    }
}

async fn append_canonical_message_event(
    session_store: Option<&dyn SessionStore>,
    session_id: &str,
    kind: &str,
    agent: &str,
    handoff_depth: u32,
    turn: Option<u32>,
    message: &Message,
) {
    let Some(store) = session_store else {
        return;
    };
    let event = SessionEvent::new(
        session_id.to_string(),
        kind.to_string(),
        agent.to_string(),
        handoff_depth,
        turn,
        serde_json::json!({
            "message": message,
            "json_detail": {
                "message": message
            }
        }),
    );
    if let Err(err) = store.append_event(&event).await {
        warn!(error = %err, kind, "Failed to append canonical message event");
    }
}

async fn append_tool_result_event(
    session_store: Option<&dyn SessionStore>,
    session_id: &str,
    agent: &str,
    handoff_depth: u32,
    turn: Option<u32>,
    message: &Message,
    tool_execution: &ToolExecutionRecord,
) {
    let Some(store) = session_store else {
        return;
    };
    let event = SessionEvent::new(
        session_id.to_string(),
        "tool_result_added".to_string(),
        agent.to_string(),
        handoff_depth,
        turn,
        serde_json::json!({
            "message": message,
            "tool_execution": tool_execution,
            "json_detail": {
                "message": message,
                "tool_execution": tool_execution
            }
        }),
    );
    if let Err(err) = store.append_event(&event).await {
        warn!(error = %err, "Failed to append tool result event");
    }
}

async fn append_conversation_checkpoint(
    session_store: Option<&dyn SessionStore>,
    session_id: &str,
    agent: &str,
    handoff_depth: u32,
    turn: Option<u32>,
    session: &SessionState,
) {
    let Some(store) = session_store else {
        return;
    };
    let event = SessionEvent::new(
        session_id.to_string(),
        "conversation_checkpoint".to_string(),
        agent.to_string(),
        handoff_depth,
        turn,
        serde_json::json!({
            "message_count": session.meta.message_count,
            "total_prompt_tokens": session.meta.total_prompt_tokens,
            "total_completion_tokens": session.meta.total_completion_tokens,
            "messages": &session.messages,
            "tool_executions": &session.tool_executions,
            "json_detail": {
                "message_count": session.meta.message_count,
                "total_prompt_tokens": session.meta.total_prompt_tokens,
                "total_completion_tokens": session.meta.total_completion_tokens
            }
        }),
    );
    if let Err(err) = store.append_event(&event).await {
        warn!(error = %err, "Failed to append conversation checkpoint");
    }
}

struct ChildEventForwardingStore<'a> {
    inner: &'a dyn SessionStore,
    parent_session_id: String,
}

#[async_trait]
impl SessionStore for ChildEventForwardingStore<'_> {
    async fn save_session(&self, _session: &SessionState) -> Result<()> {
        Ok(())
    }

    async fn load_session(&self, _id: &str) -> Result<SessionState> {
        Err(crate::Error::session(
            "Child forwarding store cannot load sessions",
        ))
    }

    async fn list_sessions(&self) -> Result<Vec<crate::session::SessionMeta>> {
        Err(crate::Error::session(
            "Child forwarding store cannot list sessions",
        ))
    }

    async fn delete_session(&self, _id: &str) -> Result<()> {
        Err(crate::Error::session(
            "Child forwarding store cannot delete sessions",
        ))
    }

    async fn find_session_by_name(&self, _name: &str) -> Result<Option<SessionState>> {
        Err(crate::Error::session(
            "Child forwarding store cannot find sessions",
        ))
    }

    async fn append_event(&self, event: &SessionEvent) -> Result<()> {
        let mut forwarded = event.clone();
        forwarded.session_id = self.parent_session_id.clone();
        self.inner.append_event(&forwarded).await
    }
}

/// Truncate a string for log display
fn truncate_for_log(s: &str, max: usize) -> String {
    let total_chars = s.chars().count();
    if total_chars > max {
        format!(
            "{}...[{} chars total]",
            truncate_to_chars(s, max),
            total_chars
        )
    } else {
        s.to_string()
    }
}

fn truncate_to_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn extract_credits_remaining(headers: &[(String, String)]) -> Option<f64> {
    headers.iter().find_map(|(k, v)| {
        let key = k.to_ascii_lowercase();
        if key.contains("credit") && key.contains("remaining") {
            return v.parse::<f64>().ok();
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, StaticProviderResolver};
    use async_trait::async_trait;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestProvider {
        name: &'static str,
    }

    #[async_trait]
    impl Provider for TestProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn base_url(&self) -> &str {
            "https://example.invalid"
        }

        fn api_key(&self) -> &str {
            "test-key"
        }
    }

    fn make_temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("zagent-{label}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn collects_skill_definitions_from_workspace() {
        let cwd = make_temp_dir("skills");
        let skill_dir = cwd.join(".skills").join("rust-refactor");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            r#"---
name: Rust Refactor
description: Apply the house Rust refactor workflow.
---
# Rust Refactor
Use this skill when doing multi-file Rust cleanup.
"#,
        )
        .expect("write skill file");

        let discovered = collect_skill_definitions(cwd.to_str().expect("cwd utf8"));
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].name, "Rust Refactor");
        assert_eq!(
            discovered[0].description,
            "Apply the house Rust refactor workflow."
        );
        assert_eq!(
            discovered[0].relative_path_from_cwd,
            ".skills/rust-refactor/SKILL.md"
        );

        fs::remove_dir_all(cwd).expect("remove temp dir");
    }

    #[test]
    fn effective_prompt_includes_rules_and_skill_catalog() {
        let cwd = make_temp_dir("prompt");
        fs::write(cwd.join(AGENTS_FILE_NAME), "Root rule: keep diffs small.\n")
            .expect("write agents file");
        let skill_dir = cwd.join("skills").join("release");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            "# Release Skill\nFollow the release checklist.\n",
        )
        .expect("write skill file");

        let prompt =
            build_effective_system_prompt("Base prompt", cwd.to_str().expect("cwd utf8"), &[]);
        assert!(prompt.contains("# Rules"));
        assert!(prompt.contains("Root rule: keep diffs small."));
        assert!(prompt.contains("# Available Skills"));
        assert!(prompt.contains("release: Release Skill [source: skills/release/SKILL.md]"));
        assert!(prompt.contains("only read a skill with file_read"));

        fs::remove_dir_all(cwd).expect("remove temp dir");
    }

    #[test]
    fn provider_prefixed_agent_models_use_configured_provider() {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            "openrouter".to_string(),
            Arc::new(TestProvider { name: "openrouter" }),
        );
        providers.insert(
            "openai".to_string(),
            Arc::new(TestProvider { name: "openai" }),
        );
        let resolver = StaticProviderResolver::new("openai", &providers);

        let (provider_name, model_name) =
            resolve_model_and_provider("openrouter:minimax/minimax-m2.5", &resolver, "openai")
                .expect("provider-prefixed model should resolve");

        assert_eq!(provider_name, "openrouter");
        assert_eq!(model_name, "minimax/minimax-m2.5");
    }

    #[test]
    fn provider_prefixed_agent_models_require_configured_provider() {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            "openai".to_string(),
            Arc::new(TestProvider { name: "openai" }),
        );
        let resolver = StaticProviderResolver::new("openai", &providers);

        let err =
            resolve_model_and_provider("openrouter:minimax/minimax-m2.5", &resolver, "openai")
                .expect_err("missing provider should be rejected");

        assert!(
            err.to_string()
                .contains("Provider 'openrouter' is not configured")
        );
    }
}
