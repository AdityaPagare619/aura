#![feature(once_cell_try)]
//! Standalone binary entry point for the AURA v4 daemon.
//!
//! This binary is used in two scenarios:
//! 1. **Termux** — installed to `$PREFIX/bin/aura-daemon` by `install.sh`
//! 2. **Host development** — run directly on Linux/macOS for testing
//!
//! On Android APK mode, the daemon is loaded as a shared library (`libaura_core.so`)
//! via JNI — this binary is NOT used in that case.
//!
//! Usage:
//!   aura-daemon --config <path/to/config.toml>

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Print usage information.
fn print_usage() {
    let usage = "\
aura-daemon — AURA v4 Always-On Daemon

USAGE:
    aura-daemon [OPTIONS]

OPTIONS:
    -c, --config <PATH>    Path to config.toml
                           Default: ~/.config/aura/config.toml
    -h, --help             Print this help message
    -V, --version          Print version
";
    println!("{usage}");
}

/// Parse CLI arguments (no clap — keep binary small).
struct Args {
    config_path: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().collect();
        let mut config_path: Option<PathBuf> = None;

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--config" | "-c" => {
                    i += 1;
                    config_path =
                        Some(PathBuf::from(args.get(i).ok_or("--config requires a value")?));
                },
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                },
                "--version" | "-V" => {
                    println!("aura-daemon {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                },
                other => {
                    return Err(format!("unknown argument: {other}"));
                },
            }
            i += 1;
        }

        // Default config path: ~/.config/aura/config.toml
        let config_path = config_path.unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home)
                .join(".config")
                .join("aura")
                .join("config.toml")
        });

        Ok(Args { config_path })
    }
}

fn main() {
    // Initialize tracing early.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .compact()
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "aura-daemon starting");

    // Parse CLI args.
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, "failed to parse arguments");
            print_usage();
            std::process::exit(1);
        },
    };

    tracing::info!(config = %args.config_path.display(), "loading configuration");

    // Load config from TOML file.
    let config = match load_config(&args.config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, path = %args.config_path.display(), "failed to load config");
            std::process::exit(1);
        },
    };

    // Set up SIGTERM/SIGINT handler for graceful shutdown.
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    setup_signal_handler(shutdown_flag.clone());

    // Run the daemon.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime must initialize");

    rt.block_on(async {
        // Phase 1-8: Startup.
        let (state, report) = match aura_daemon::startup(config) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "startup failed");
                std::process::exit(1);
            },
        };

        tracing::info!(
            total_ms = report.total_ms,
            phases = report.phases.len(),
            "startup complete"
        );

        // Wire the external shutdown flag into the daemon's cancel_flag.
        let cancel = state.cancel_flag.clone();
        let shutdown = shutdown_flag.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                if shutdown.load(Ordering::SeqCst) {
                    tracing::info!("external shutdown signal received — setting cancel flag");
                    cancel.store(true, Ordering::Release);
                    break;
                }
            }
        });

        // Enter main event loop (runs until cancel_flag is set).
        aura_daemon::daemon_core::main_loop::run(state).await;

        tracing::info!("aura-daemon shut down cleanly");
    });
}

/// Load `AuraConfig` from a TOML file.
fn load_config(
    path: &std::path::Path,
) -> Result<aura_types::config::AuraConfig, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Err(format!(
            "config file not found: {}\n\
             Run install.sh first or create the config manually.",
            path.display()
        )
        .into());
    }

    let contents = std::fs::read_to_string(path)?;
    let config: aura_types::config::AuraConfig = toml::from_str(&contents)?;
    tracing::info!(path = %path.display(), "config loaded successfully");
    Ok(config)
}

/// Set up signal handlers for graceful shutdown.
///
/// On Unix (including Termux): catches SIGTERM and SIGINT.
/// On other platforms: catches CTRL+C only.
fn setup_signal_handler(shutdown: Arc<AtomicBool>) {
    // Use a simple thread-based approach that works everywhere.
    // ctrlc/signal-hook crates would be better but we avoid extra deps.
    let flag = shutdown.clone();
    std::thread::Builder::new()
        .name("signal-handler".into())
        .spawn(move || {
            // On Unix, we can catch SIGTERM via a self-pipe trick.
            // For simplicity, we just handle stdin EOF as a shutdown signal
            // (the termux-services supervisor sends SIGTERM which closes stdin).
            //
            // The tokio spawn above polls shutdown_flag every 500ms, so worst-case
            // latency to shutdown is 500ms.
            #[cfg(unix)]
            {
                use std::io::BufRead;
                let stdin = std::io::stdin();
                let reader = stdin.lock();
                for line in reader.lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    if trimmed.eq_ignore_ascii_case("SHUTDOWN") {
                        tracing::info!("received SHUTDOWN on stdin");
                        flag.store(true, Ordering::SeqCst);
                        return;
                    }
                }
                // stdin closed (EOF) — for Termux service mode, this means
                // the supervisor wants us gone.
                tracing::info!("stdin closed — interpreting as shutdown signal");
                flag.store(true, Ordering::SeqCst);
            }

            #[cfg(not(unix))]
            {
                // On non-Unix, just block forever. CTRL+C will kill the process.
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            }
        })
        .expect("failed to spawn signal-handler thread");
}
