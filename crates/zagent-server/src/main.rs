mod auth;

use clap::Parser;
use zagent_backend::api;
use zagent_backend::engine::{BackendEngine, BackendOptions, RuntimeTarget, SessionStoreTarget};

#[derive(Debug, Parser)]
#[command(name = "zagent-server", version, about = "zAgent backend server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 8787)]
    port: u16,

    #[arg(long, default_value = "./logs")]
    log_dir: String,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long)]
    model: Option<String>,

    #[arg(short, long)]
    system: Option<String>,

    #[arg(short, long)]
    working_dir: Option<String>,

    #[arg(long)]
    session_dir: Option<String>,

    #[arg(short = 'S', long)]
    session: Option<String>,

    #[arg(long)]
    new_session: Option<String>,

    #[arg(long, default_value = "native")]
    runtime: String,

    #[arg(long, default_value = "surreal")]
    session_store: String,

    #[arg(long, default_value_t = 50)]
    max_turns: u32,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    Auth(AuthCommand),
}

#[derive(Debug, clap::Args)]
struct AuthCommand {
    #[command(subcommand)]
    provider: AuthProvider,
}

#[derive(Debug, clap::Subcommand)]
enum AuthProvider {
    Openai {
        #[arg(long, alias = "env-file", default_value = "auth.json")]
        auth_file: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_env_files();
    let cli = Cli::parse();

    if let Some(command) = cli.command {
        return run_command(command).await;
    }

    let _guard = zagent_backend::tracing_setup::init_tracing(&cli.log_dir, cli.verbose);

    let runtime = RuntimeTarget::parse(&cli.runtime)?;
    let session_store = SessionStoreTarget::parse(&cli.session_store)?;

    let working_dir = cli.working_dir.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    });

    let engine = match BackendEngine::new(BackendOptions {
        runtime,
        session_store,
        model: cli.model,
        system_prompt: cli.system,
        working_dir,
        session_dir: cli.session_dir,
        resume_session: cli.session,
        new_session: cli.new_session,
        max_turns: cli.max_turns,
    })
    .await
    {
        Ok(engine) => engine,
        Err(err) => {
            maybe_print_openai_auth_hint(&err);
            return Err(Box::new(err));
        }
    };

    let app = api::router(engine);
    let addr: std::net::SocketAddr = format!("{}:{}", cli.host, cli.port).parse()?;
    println!("zagent-server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn load_env_files() {
    dotenvy::from_filename(".env").ok();
    dotenvy::from_filename_override(".env-auth").ok();
}

async fn run_command(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Auth(AuthCommand {
            provider: AuthProvider::Openai { auth_file },
        }) => {
            let output = auth::run_openai_auth(std::path::Path::new(&auth_file)).await?;
            println!();
            println!("Saved ChatGPT subscription credentials to {auth_file}");
            println!("Account ID: {}", output.account_id);
            println!("The access token was saved to auth.json format.");
            println!(
                "zAgent will look for `auth.json`, `.zagent/auth.json`, and `~/.zagent/auth.json`."
            );
            Ok(())
        }
    }
}

fn maybe_print_openai_auth_hint(error: &impl std::fmt::Display) {
    let message = error.to_string();
    if message.contains("Configured default provider 'openai' is not available")
        || message.contains("missing a ChatGPT access token")
        || message.contains("missing a ChatGPT account id")
        || message.contains("missing a ChatGPT account/workspace id")
    {
        eprintln!();
        eprintln!("OpenAI ChatGPT subscription auth is configured but not ready.");
        eprintln!("Run `cargo run -p zagent-server -- auth openai` to create `auth.json`.");
    }
}
