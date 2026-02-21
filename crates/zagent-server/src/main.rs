use clap::Parser;
use zagent_backend::api;
use zagent_backend::engine::{BackendEngine, BackendOptions, RuntimeTarget};

#[derive(Debug, Parser)]
#[command(name = "zagent-server", version, about = "zAgent backend server")]
struct Cli {
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

    #[arg(long, default_value_t = 50)]
    max_turns: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let _guard = zagent_backend::tracing_setup::init_tracing(&cli.log_dir, cli.verbose);

    let runtime = RuntimeTarget::parse(&cli.runtime)?;

    let working_dir = cli.working_dir.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    });

    let engine = BackendEngine::new(BackendOptions {
        runtime,
        model: cli.model,
        system_prompt: cli.system,
        working_dir,
        session_dir: cli.session_dir,
        resume_session: cli.session,
        new_session: cli.new_session,
        max_turns: cli.max_turns,
    })
    .await?;

    let app = api::router(engine);
    let addr: std::net::SocketAddr = format!("{}:{}", cli.host, cli.port).parse()?;
    println!("zagent-server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
