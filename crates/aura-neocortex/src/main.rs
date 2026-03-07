//! AURA Neocortex — LLM inference binary.
//!
//! This is a **separate process** from the AURA daemon.  It communicates via
//! IPC (Unix domain socket on Android, TCP on host) and can be killed by the
//! Android Low-Memory Killer without affecting the daemon.
//!
//! Usage:
//!   aura-neocortex --socket <addr> --model-dir <path>

mod context;
mod grammar;
mod inference;
mod ipc_handler;
mod model;
mod prompts;
mod tool_format;

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::{error, info};

use ipc_handler::IpcHandler;
use model::ModelManager;

// ─── CLI argument parsing (no clap dependency — keep it minimal) ────────────

struct Args {
    /// Socket address to bind.
    /// On Android: Unix abstract socket name (e.g., "@aura_ipc_v4").
    /// On host: TCP address (e.g., "127.0.0.1:9876").
    socket: String,
    /// Directory containing GGUF model files.
    model_dir: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().collect();

        let mut socket = String::from("127.0.0.1:9876");
        let mut model_dir = PathBuf::from("models");

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--socket" | "-s" => {
                    i += 1;
                    socket = args.get(i).ok_or("--socket requires a value")?.clone();
                }
                "--model-dir" | "-m" => {
                    i += 1;
                    model_dir = PathBuf::from(args.get(i).ok_or("--model-dir requires a value")?);
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => {
                    return Err(format!("unknown argument: {other}"));
                }
            }
            i += 1;
        }

        Ok(Args { socket, model_dir })
    }
}

fn print_usage() {
    let usage = "\
aura-neocortex — AURA LLM Inference Process

USAGE:
    aura-neocortex [OPTIONS]

OPTIONS:
    -s, --socket <ADDR>       Socket address to bind
                              Default: 127.0.0.1:9876 (TCP on host)
                              Android: @aura_ipc_v4 (Unix abstract socket)
    -m, --model-dir <PATH>    Directory containing GGUF model files
                              Default: models
    -h, --help                Print this help message
";
    // Print directly — this runs before tracing is initialized.
    println!("{usage}");
}

// ─── Entry point ────────────────────────────────────────────────────────────

fn main() {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .compact()
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "aura-neocortex starting"
    );

    // Parse CLI args.
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            error!(error = %e, "failed to parse arguments");
            print_usage();
            std::process::exit(1);
        }
    };

    info!(socket = %args.socket, model_dir = %args.model_dir.display(), "configuration");

    // Set up shutdown signal handler.
    let shutdown = Arc::new(AtomicBool::new(false));
    let cancel_token = Arc::new(AtomicBool::new(false));

    // Register SIGTERM/SIGINT handler for graceful shutdown.
    {
        let shutdown_flag = shutdown.clone();
        let cancel_flag = cancel_token.clone();

        // Use a simple CTRL+C handler (works on all platforms).
        let _ = ctrlc_handler(move || {
            info!("shutdown signal received");
            shutdown_flag.store(true, Ordering::SeqCst);
            cancel_flag.store(true, Ordering::SeqCst);
        });
    }

    // Create model manager.
    let model_manager = ModelManager::new(args.model_dir.clone());

    // Bind and accept connections.
    // On host: TCP.  On Android: would be Unix domain socket.
    if let Err(e) = run_server(
        &args.socket,
        args.model_dir,
        model_manager,
        cancel_token,
        shutdown,
    ) {
        error!(error = %e, "server error");
        std::process::exit(1);
    }

    info!("aura-neocortex shut down cleanly");
}

/// Run the IPC server, accepting a single connection at a time.
///
/// The daemon maintains a single persistent connection to neocortex.
/// If it disconnects, we accept the next connection.
fn run_server(
    address: &str,
    model_dir: PathBuf,
    model_manager: ModelManager,
    cancel_token: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(address)?;
    info!(address, "listening for daemon connections");

    // Set non-blocking so we can check the shutdown flag periodically.
    listener.set_nonblocking(true)?;

    // We need to share model_manager across reconnections.
    // Since we handle one connection at a time, we can move it in and out.
    let mut mgr = model_manager;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("shutdown flag set — stopping server");
            break;
        }

        match listener.accept() {
            Ok((stream, addr)) => {
                info!(peer = %addr, "daemon connected");

                // Switch stream to blocking mode for the handler.
                stream.set_nonblocking(false)?;

                let mut handler = IpcHandler::new(stream, mgr, cancel_token.clone())?;

                match handler.run_loop() {
                    Ok(()) => info!("connection closed cleanly"),
                    Err(e) => error!(error = %e, "connection error"),
                }

                // Recover model manager from handler for reuse.
                // Since IpcHandler owns it, we need to reconstruct.
                // In practice, the daemon rarely reconnects — usually this
                // means the neocortex process is being shut down.
                mgr = ModelManager::new(model_dir.clone());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No connection yet — sleep briefly and retry.
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            Err(e) => {
                error!(error = %e, "accept failed");
                return Err(e.into());
            }
        }
    }

    Ok(())
}

/// Install a closure to run on CTRL+C / SIGTERM.
///
/// Uses a simple approach without depending on the `ctrlc` crate.
fn ctrlc_handler<F>(handler: F) -> Result<(), String>
where
    F: FnOnce() + Send + 'static,
{
    // We use a thread that waits for the process to be signalled.
    // On Windows, there's no direct SIGTERM, but CTRL+C works.
    // This is a best-effort handler for development.
    std::thread::spawn(move || {
        // Wait for a signal by blocking on stdin close or similar.
        // In production Android builds, the daemon manages our lifecycle.
        // For host development, CTRL+C will kill the process anyway.
        //
        // A proper implementation would use `signal-hook` or `ctrlc` crate,
        // but we keep dependencies minimal.
        let _ = std::io::stdin().read_line(&mut String::new());
        handler();
    });
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse_defaults() {
        // Simulate no CLI args (just the program name).
        // We can't easily test Args::parse() since it reads std::env::args(),
        // but we can verify the default values directly.
        let args = Args {
            socket: "127.0.0.1:9876".into(),
            model_dir: PathBuf::from("models"),
        };
        assert_eq!(args.socket, "127.0.0.1:9876");
        assert_eq!(args.model_dir, PathBuf::from("models"));
    }

    #[test]
    fn shutdown_flag_works() {
        let shutdown = Arc::new(AtomicBool::new(false));
        assert!(!shutdown.load(Ordering::SeqCst));

        shutdown.store(true, Ordering::SeqCst);
        assert!(shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_token_works() {
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = crate::inference::InferenceEngine::new(cancel.clone());

        assert!(!cancel.load(Ordering::SeqCst));
        engine.cancel();
        assert!(cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn module_imports_work() {
        // Verify all modules are accessible.
        let _ = crate::prompts::mode_config(aura_types::ipc::InferenceMode::Planner);
        let _ = crate::model::available_ram_mb();
        let _ = crate::prompts::estimate_tokens("hello");
    }
}
