use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Runtime {
    Native,
    Wasi,
}

#[derive(Debug, Parser)]
#[command(
    name = "zagent-web-dev",
    about = "Run backend + web frontend with API proxy"
)]
struct Args {
    /// Backend runtime mode.
    #[arg(long, value_enum, default_value_t = Runtime::Native)]
    runtime: Runtime,

    /// Backend API port.
    #[arg(long, default_value_t = 8787)]
    backend_port: u16,

    /// Web dev server port.
    #[arg(long, default_value_t = 8080)]
    web_port: u16,

    /// Open browser when trunk is ready.
    #[arg(long, default_value_t = false)]
    open: bool,
}

fn workspace_root() -> Result<PathBuf, String> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = here
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "failed to resolve workspace root".to_string())?;
    Ok(root.to_path_buf())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let root = match workspace_root() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    let web_dir = root.join("crates").join("zagent-web");
    let backend_addr = format!("http://127.0.0.1:{}", args.backend_port);
    let proxy_backend_addr = format!("{backend_addr}/api");

    let mut backend_cmd = Command::new("cargo");
    backend_cmd
        .arg("run")
        .arg("-p")
        .arg("zagent-server")
        .arg("--")
        .arg("--port")
        .arg(args.backend_port.to_string())
        .current_dir(&root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if matches!(args.runtime, Runtime::Wasi) {
        backend_cmd.arg("--runtime").arg("wasi");
    }

    let mut backend = match backend_cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            eprintln!("failed to start backend server: {err}");
            return ExitCode::FAILURE;
        }
    };

    thread::sleep(Duration::from_millis(400));

    let mut trunk_cmd = Command::new("trunk");
    trunk_cmd
        .arg("serve")
        .arg("--address")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(args.web_port.to_string())
        .arg("--proxy-backend")
        .arg(proxy_backend_addr)
        .arg("--proxy-rewrite")
        .arg("/api")
        .current_dir(&web_dir)
        .env_remove("NO_COLOR")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if args.open {
        trunk_cmd.arg("--open");
    }

    let mut trunk = match trunk_cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = backend.kill();
            let _ = backend.wait();
            eprintln!("failed to start trunk dev server: {err}");
            return ExitCode::FAILURE;
        }
    };

    loop {
        match backend.try_wait() {
            Ok(Some(status)) => {
                let _ = trunk.kill();
                let _ = trunk.wait();
                eprintln!("backend server exited: {status}");
                return ExitCode::from(status.code().unwrap_or(1) as u8);
            }
            Ok(None) => {}
            Err(err) => {
                let _ = trunk.kill();
                let _ = trunk.wait();
                eprintln!("failed to monitor backend process: {err}");
                return ExitCode::FAILURE;
            }
        }

        match trunk.try_wait() {
            Ok(Some(status)) => {
                let _ = backend.kill();
                let _ = backend.wait();
                return ExitCode::from(status.code().unwrap_or(1) as u8);
            }
            Ok(None) => {}
            Err(err) => {
                let _ = backend.kill();
                let _ = backend.wait();
                eprintln!("failed to monitor trunk process: {err}");
                return ExitCode::FAILURE;
            }
        }

        thread::sleep(Duration::from_millis(250));
    }
}
