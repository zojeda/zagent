mod platform;
mod session_store;
mod session_store_json;
mod tools;
mod tracing_setup;

use std::sync::Arc;

use clap::Parser;
use colored::Colorize;
use tracing::warn;
use tracing::{Instrument, info, info_span};

use zagent_core::agent::{AgentConfig, run_agent_loop};
use zagent_core::config::load_config;
use zagent_core::provider::configured::{
    build_configured_providers, ensure_requested_provider_available, resolve_default_model,
    resolve_workspace_default_model, select_initial_provider, split_provider_model,
};
use zagent_core::provider::{ProviderResolver, StaticProviderResolver};
use zagent_core::session::{SessionState, SessionStore};

/// zAgent — Observable multi-LLM coding agent
#[derive(Parser, Debug)]
#[command(name = "zagent", version, about = "Observable multi-LLM coding agent")]
struct Cli {
    /// Single-shot prompt (if provided, skip REPL)
    #[arg(short, long)]
    prompt: Option<String>,

    /// Model to use (e.g., "anthropic/claude-sonnet-4", "openai/gpt-4o")
    #[arg(short, long)]
    model: Option<String>,

    /// Custom system prompt
    #[arg(short, long)]
    system: Option<String>,

    /// Working directory for tool execution
    #[arg(short, long)]
    working_dir: Option<String>,

    /// JSON log directory
    #[arg(long, default_value = "./logs")]
    log_dir: String,

    /// Session database directory
    #[arg(long)]
    session_dir: Option<String>,

    /// Resume session by name or ID
    #[arg(short = 'S', long)]
    session: Option<String>,

    /// Start a new named session
    #[arg(long)]
    new_session: Option<String>,

    /// List all sessions and exit
    #[arg(long)]
    list_sessions: bool,

    /// Delete a session by name or ID
    #[arg(long)]
    delete_session: Option<String>,

    /// Verbose mode (TRACE-level terminal output)
    #[arg(short, long)]
    verbose: bool,

    /// Maximum agent turns per invocation
    #[arg(long, default_value = "50")]
    max_turns: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_env_files();

    let cli = Cli::parse();

    // Initialize tracing
    let _guard = tracing_setup::init_tracing(&cli.log_dir, cli.verbose);

    info!(
        model = ?cli.model,
        "zAgent starting"
    );

    let working_dir = cli.working_dir.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    });

    let app_config = load_config(&working_dir)?;
    let providers = build_configured_providers(&app_config, &working_dir)?;
    if providers.is_empty() {
        return Err(zagent_core::Error::config(
            "No providers configured. Add providers to zagent-config.yaml or set OPENROUTER_API_KEY / OPENAI_API_KEY.",
        ).into());
    }
    ensure_requested_provider_available(
        app_config.default_provider.as_deref(),
        cli.model.as_deref(),
        &app_config,
        &providers,
    )?;
    let provider_name = select_initial_provider(
        app_config.default_provider.as_deref(),
        cli.model.as_deref(),
        &providers,
    )?;
    if !providers.contains_key(&provider_name) {
        return Err(zagent_core::Error::config(format!(
            "Provider '{provider_name}' is not configured"
        ))
        .into());
    }
    let provider_resolver = StaticProviderResolver::new(&provider_name, &providers);

    // Set up session store early (needed for --list-sessions, --delete-session)
    let session_dir = cli.session_dir.clone().unwrap_or_else(dirs_session_default);
    let session_store = build_session_store(&session_dir).await?;

    // Handle session management commands that don't need an API key
    if cli.list_sessions {
        return handle_list_sessions(session_store.as_ref()).await;
    }
    if let Some(ref name_or_id) = cli.delete_session {
        return handle_delete_session(session_store.as_ref(), name_or_id).await;
    }

    // Set up HTTP client
    let http_client = platform::NativeHttpClient::new();

    // Set up tools
    let tool_registry = tools::register_all_tools(&working_dir);

    info!(
        tools = ?tool_registry.tool_names(),
        working_dir = %working_dir,
        "Tools registered"
    );

    // Agent config
    let mut config = AgentConfig {
        model: String::new(),
        ..AgentConfig::default()
    };
    if let Some(model) = cli.model.clone() {
        config.model = model;
    }
    config.custom_agent_default_model = resolve_workspace_default_model(&app_config, &providers)?;
    if let Some((prefixed_provider, stripped_model)) = split_provider_model(&config.model)
        && prefixed_provider == provider_name
    {
        config.model = stripped_model.to_string();
    }
    if config.model.trim().is_empty() {
        config.model = resolve_default_model(&provider_name, &app_config)?;
    }
    config.max_turns = cli.max_turns;
    config.context_management_policy = app_config.resolved_context_management_policy();
    if let Some(ref system) = cli.system {
        config.system_prompt = system.clone();
    }

    // Load or create session
    let mut session = resolve_session(
        session_store.as_ref(),
        cli.session.as_deref(),
        cli.new_session.as_deref(),
        &provider_name,
        &config,
        &working_dir,
    )
    .await?;

    println!(
        "{}",
        format!(
            "━━━ zAgent ━━━ provider: {} ━━━ model: {} ━━━ session: {} ━━━",
            provider_name.cyan(),
            config.model.cyan(),
            session.meta.name.green()
        )
        .bold()
    );
    println!(
        "  Tools: {}",
        tool_registry.tool_names().join(", ").yellow()
    );
    println!("  Working dir: {}", working_dir.dimmed());
    println!();

    let session_span = info_span!(
        "agent_session",
        session_id = %session.meta.id,
        session_name = %session.meta.name,
        model = %config.model
    );

    if let Some(prompt) = cli.prompt {
        // Single-shot mode
        run_single_prompt(
            &http_client,
            &provider_resolver,
            &tool_registry,
            &mut session,
            session_store.as_ref(),
            &config,
            &prompt,
        )
        .instrument(session_span.clone())
        .await?;
    } else {
        // REPL mode
        run_repl(
            &http_client,
            &provider_resolver,
            &tool_registry,
            &mut session,
            session_store.as_ref(),
            &config,
        )
        .instrument(session_span)
        .await?;
    }

    Ok(())
}

fn load_env_files() {
    dotenvy::from_filename(".env").ok();
    dotenvy::from_filename_override(".env-auth").ok();
}

async fn run_single_prompt(
    http_client: &platform::NativeHttpClient,
    providers: &dyn ProviderResolver,
    tools: &zagent_core::tools::ToolRegistry,
    session: &mut SessionState,
    session_store: &dyn SessionStore,
    config: &AgentConfig,
    prompt: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{} {}", "You:".blue().bold(), prompt);
    println!();

    let result = run_agent_loop(
        http_client,
        providers,
        tools,
        None,
        session,
        Some(session_store),
        config,
        prompt,
    )
    .await?;

    println!();
    println!("{}", "━".repeat(60).dimmed());
    println!("{}", result.response);
    println!("{}", "━".repeat(60).dimmed());
    println!(
        "{}",
        format!(
            "Turns: {} | Tools: {} | Tokens: {}↑ {}↓",
            result.turns,
            result.tool_calls_count,
            result.total_prompt_tokens,
            result.total_completion_tokens,
        )
        .dimmed()
    );

    Ok(())
}

async fn run_repl(
    http_client: &platform::NativeHttpClient,
    providers: &dyn ProviderResolver,
    tools: &zagent_core::tools::ToolRegistry,
    session: &mut SessionState,
    session_store: &dyn SessionStore,
    config: &AgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| zagent_core::Error::custom(format!("Failed to create REPL: {e}")))?;

    // Load REPL history if available
    let history_path = format!("{}/.zagent_history", dirs_home());
    let _ = rl.load_history(&history_path);

    println!(
        "{}",
        "Type your prompt (Ctrl+D to exit, /help for commands)".dimmed()
    );
    println!();

    loop {
        let readline = rl.readline(&format!("{} ", ">>".green().bold()));
        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                // Handle REPL commands
                if input.starts_with('/') {
                    match handle_repl_command(input, session, session_store).await {
                        Ok(should_continue) => {
                            if !should_continue {
                                break;
                            }
                            continue;
                        }
                        Err(e) => {
                            eprintln!("{} {e}", "Error:".red().bold());
                            continue;
                        }
                    }
                }

                let _ = rl.add_history_entry(input);

                println!();

                match run_agent_loop(
                    http_client,
                    providers,
                    tools,
                    None,
                    session,
                    Some(session_store),
                    config,
                    input,
                )
                .await
                {
                    Ok(result) => {
                        println!();
                        println!("{}", "━".repeat(60).dimmed());
                        println!("{}", result.response);
                        println!("{}", "━".repeat(60).dimmed());
                        println!(
                            "{}",
                            format!(
                                "Turns: {} | Tools: {} | Tokens: {}↑ {}↓",
                                result.turns,
                                result.tool_calls_count,
                                result.total_prompt_tokens,
                                result.total_completion_tokens,
                            )
                            .dimmed()
                        );
                        println!();
                    }
                    Err(e) => {
                        eprintln!();
                        eprintln!("{} {e}", "Agent error:".red().bold());
                        eprintln!();
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("{}", "Goodbye!".dimmed());
                break;
            }
            Err(e) => {
                eprintln!("{} {e}", "REPL error:".red().bold());
                break;
            }
        }
    }

    // Save history
    let _ = rl.save_history(&history_path);

    Ok(())
}

async fn handle_repl_command(
    cmd: &str,
    session: &SessionState,
    session_store: &dyn SessionStore,
) -> Result<bool, Box<dyn std::error::Error>> {
    match cmd {
        "/help" | "/h" => {
            println!("{}", "Commands:".bold());
            println!("  /help, /h       — Show this help");
            println!("  /session, /s    — Show current session info");
            println!("  /sessions       — List all sessions");
            println!("  /clear          — Clear conversation history");
            println!("  /quit, /q       — Exit");
            Ok(true)
        }
        "/session" | "/s" => {
            println!("{}", "Current session:".bold());
            println!("  ID:       {}", session.meta.id);
            println!("  Name:     {}", session.meta.name.green());
            println!("  Model:    {}", session.meta.model.cyan());
            println!("  Messages: {}", session.meta.message_count);
            println!(
                "  Tokens:   {}↑ {}↓",
                session.meta.total_prompt_tokens, session.meta.total_completion_tokens
            );
            println!("  Created:  {}", session.meta.created_at);
            Ok(true)
        }
        "/sessions" => {
            handle_list_sessions(session_store).await?;
            Ok(true)
        }
        "/quit" | "/q" => Ok(false),
        "/clear" => {
            println!("{}", "Conversation cleared.".yellow());
            // Note: we'd need &mut session to actually clear — for now just inform
            println!(
                "{}",
                "(Start a new session with --new-session to get a clean slate)".dimmed()
            );
            Ok(true)
        }
        _ => {
            println!("{} Unknown command: {cmd}", "Error:".red().bold());
            println!("Type /help for available commands.");
            Ok(true)
        }
    }
}

async fn handle_list_sessions(
    session_store: &dyn SessionStore,
) -> Result<(), Box<dyn std::error::Error>> {
    let sessions = session_store.list_sessions().await?;

    if sessions.is_empty() {
        println!("{}", "No sessions found.".dimmed());
        return Ok(());
    }

    println!("{}", "Sessions:".bold());
    println!(
        "  {:<36}  {:<20}  {:<30}  {:>5}  {:>8}  {:>8}",
        "ID", "Name", "Model", "Msgs", "Tok↑", "Tok↓"
    );
    println!("  {}", "─".repeat(110));

    for s in &sessions {
        println!(
            "  {:<36}  {:<20}  {:<30}  {:>5}  {:>8}  {:>8}",
            s.id.dimmed(),
            s.name.green(),
            s.model.cyan(),
            s.message_count,
            s.total_prompt_tokens,
            s.total_completion_tokens,
        );
    }
    println!();

    Ok(())
}

async fn handle_delete_session(
    session_store: &dyn SessionStore,
    name_or_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Try by name first
    if let Some(session) = session_store.find_session_by_name(name_or_id).await? {
        session_store.delete_session(&session.meta.id).await?;
        println!(
            "Deleted session '{}' ({})",
            session.meta.name, session.meta.id
        );
    } else {
        // Try by ID
        session_store.delete_session(name_or_id).await?;
        println!("Deleted session {name_or_id}");
    }
    Ok(())
}

async fn resolve_session(
    session_store: &dyn SessionStore,
    resume: Option<&str>,
    new_name: Option<&str>,
    provider_name: &str,
    config: &AgentConfig,
    working_dir: &str,
) -> Result<SessionState, Box<dyn std::error::Error>> {
    if let Some(name_or_id) = resume {
        // Try to find by name first
        if let Some(session) = session_store.find_session_by_name(name_or_id).await? {
            info!(
                session_id = %session.meta.id,
                session_name = %session.meta.name,
                messages = session.meta.message_count,
                "Resuming session"
            );
            println!(
                "{}",
                format!(
                    "Resuming session '{}' ({} messages)",
                    session.meta.name.green(),
                    session.meta.message_count
                )
                .dimmed()
            );
            return Ok(session);
        }
        // Try by ID
        match session_store.load_session(name_or_id).await {
            Ok(session) => {
                info!(
                    session_id = %session.meta.id,
                    session_name = %session.meta.name,
                    messages = session.meta.message_count,
                    "Resuming session"
                );
                println!(
                    "{}",
                    format!(
                        "Resuming session '{}' ({} messages)",
                        session.meta.name.green(),
                        session.meta.message_count
                    )
                    .dimmed()
                );
                return Ok(session);
            }
            Err(e) => {
                return Err(zagent_core::Error::session(format!(
                    "Session '{name_or_id}' not found: {e}"
                ))
                .into());
            }
        }
    }

    // Create new session
    let name = new_name.map(String::from).unwrap_or_else(|| {
        chrono::Utc::now()
            .format("session-%Y%m%d-%H%M%S")
            .to_string()
    });

    let session = SessionState::new(
        &name,
        &config.model,
        provider_name,
        &config.system_prompt,
        working_dir,
    );

    info!(
        session_id = %session.meta.id,
        session_name = %session.meta.name,
        "Created new session"
    );

    // Save initial session
    session_store.save_session(&session).await?;

    Ok(session)
}

fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

fn dirs_session_default() -> String {
    format!("{}/.zagent", dirs_home())
}

async fn build_session_store(
    session_dir_or_endpoint: &str,
) -> Result<Arc<dyn SessionStore>, Box<dyn std::error::Error>> {
    if session_dir_or_endpoint.contains("://") {
        let store = session_store::SurrealSessionStore::new(session_dir_or_endpoint).await?;
        return Ok(Arc::new(store));
    }

    if let Ok(endpoint) = std::env::var("SURREALDB_URL")
        && !endpoint.trim().is_empty()
    {
        match session_store::SurrealSessionStore::new(&endpoint).await {
            Ok(store) => return Ok(Arc::new(store)),
            Err(err) => {
                warn!(
                    endpoint = %endpoint,
                    error = %err,
                    "Failed to connect to SurrealDB from SURREALDB_URL; falling back to JSON session store"
                );
            }
        }
    }

    let path = format!("{session_dir_or_endpoint}/native-sessions.json");
    warn!(
        path = %path,
        "SURREALDB_URL not set; using JSON session store for native CLI"
    );
    let store = session_store_json::JsonSessionStore::new(path)?;
    Ok(Arc::new(store))
}
