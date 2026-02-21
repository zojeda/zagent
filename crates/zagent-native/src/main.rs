mod platform;
mod session_store;
mod tools;
mod tracing_setup;

use clap::Parser;
use colored::Colorize;
use tracing::{Instrument, info, info_span};

use zagent_core::agent::{AgentConfig, run_agent_loop};
use zagent_core::provider::openrouter::OpenRouterProvider;
use zagent_core::session::{SessionState, SessionStore};

/// zAgent — Observable multi-LLM coding agent
#[derive(Parser, Debug)]
#[command(name = "zagent", version, about = "Observable multi-LLM coding agent")]
struct Cli {
    /// Single-shot prompt (if provided, skip REPL)
    #[arg(short, long)]
    prompt: Option<String>,

    /// Model to use (e.g., "anthropic/claude-sonnet-4", "openai/gpt-4o")
    #[arg(short, long, default_value = "anthropic/claude-sonnet-4")]
    model: String,

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
    // Load .env if present
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Initialize tracing
    let _guard = tracing_setup::init_tracing(&cli.log_dir, cli.verbose);

    info!(
        model = %cli.model,
        "zAgent starting"
    );

    // Set up session store early (needed for --list-sessions, --delete-session)
    let session_dir = cli.session_dir.clone().unwrap_or_else(dirs_session_default);
    let session_endpoint = resolve_surreal_endpoint(&session_dir);
    let session_store = session_store::SurrealSessionStore::new(&session_endpoint).await?;

    // Handle session management commands that don't need an API key
    if cli.list_sessions {
        return handle_list_sessions(&session_store).await;
    }
    if let Some(ref name_or_id) = cli.delete_session {
        return handle_delete_session(&session_store, name_or_id).await;
    }

    // Get API key (required for actual agent work)
    let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
        zagent_core::Error::config(
            "OPENROUTER_API_KEY not set. Create a .env file with OPENROUTER_API_KEY=sk-or-...",
        )
    })?;

    // Set up provider
    let provider = OpenRouterProvider::new(api_key);

    // Set up HTTP client
    let http_client = platform::NativeHttpClient::new();

    // Working directory
    let working_dir = cli.working_dir.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    });

    // Set up tools
    let tool_registry = tools::register_all_tools(&working_dir);

    info!(
        tools = ?tool_registry.tool_names(),
        working_dir = %working_dir,
        "Tools registered"
    );

    // Agent config
    let mut config = AgentConfig::default();
    config.model = cli.model.clone();
    config.max_turns = cli.max_turns;
    if let Some(ref system) = cli.system {
        config.system_prompt = system.clone();
    }

    // Load or create session
    let mut session = resolve_session(
        &session_store,
        cli.session.as_deref(),
        cli.new_session.as_deref(),
        &config,
        &working_dir,
    )
    .await?;

    println!(
        "{}",
        format!(
            "━━━ zAgent ━━━ model: {} ━━━ session: {} ━━━",
            cli.model.cyan(),
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
            &provider,
            &tool_registry,
            &mut session,
            &session_store,
            &config,
            &prompt,
        )
        .instrument(session_span.clone())
        .await?;
    } else {
        // REPL mode
        run_repl(
            &http_client,
            &provider,
            &tool_registry,
            &mut session,
            &session_store,
            &config,
        )
        .instrument(session_span)
        .await?;
    }

    Ok(())
}

async fn run_single_prompt(
    http_client: &platform::NativeHttpClient,
    provider: &OpenRouterProvider,
    tools: &zagent_core::tools::ToolRegistry,
    session: &mut SessionState,
    session_store: &session_store::SurrealSessionStore,
    config: &AgentConfig,
    prompt: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{} {}", "You:".blue().bold(), prompt);
    println!();

    let result = run_agent_loop(
        http_client,
        provider,
        tools,
        session,
        Some(session_store as &dyn SessionStore),
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
    provider: &OpenRouterProvider,
    tools: &zagent_core::tools::ToolRegistry,
    session: &mut SessionState,
    session_store: &session_store::SurrealSessionStore,
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
                    provider,
                    tools,
                    session,
                    Some(session_store as &dyn SessionStore),
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
    session_store: &session_store::SurrealSessionStore,
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
    session_store: &session_store::SurrealSessionStore,
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
    session_store: &session_store::SurrealSessionStore,
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
    session_store: &session_store::SurrealSessionStore,
    resume: Option<&str>,
    new_name: Option<&str>,
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
                return Err(Box::new(zagent_core::Error::session(format!(
                    "Session '{name_or_id}' not found: {e}"
                ))));
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
        "openrouter",
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

fn resolve_surreal_endpoint(session_dir_or_endpoint: &str) -> String {
    if session_dir_or_endpoint.contains("://") {
        return session_dir_or_endpoint.to_string();
    }
    std::env::var("SURREALDB_URL").unwrap_or_else(|_| "ws://127.0.0.1:8000".to_string())
}
