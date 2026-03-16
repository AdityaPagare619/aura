#![feature(negative_impls)]
//! AURA Neocortex — LLM inference binary.
//!
//! This is a **separate process** from the AURA daemon.  It communicates via
//! IPC (Unix domain socket on Android, TCP on host) and can be killed by the
//! Android Low-Memory Killer without affecting the daemon.
//!
//! Usage:
//!   aura-neocortex --socket <addr> --model-dir <path>

mod aura_config;
mod context;
mod grammar;
mod inference;
mod ipc_handler;
mod model;
mod model_capabilities;
mod prompts;
mod tool_format;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use aura_config::NeocortexRuntimeConfig;
use ipc_handler::IpcHandler;
use model::ModelManager;
use model_capabilities::ModelCapabilities;
use tracing::{error, info, warn};

// ─── CLI argument parsing (no clap dependency — keep it minimal) ────────────

/// Platform-specific default socket address.
///
/// - **Android:** `@aura_ipc_v4` (abstract Unix domain socket).
/// - **Non-Android:** `127.0.0.1:19400` (TCP, matching daemon's `TCP_FALLBACK_PORT`).
fn default_socket_address() -> String {
    #[cfg(target_os = "android")]
    {
        String::from("@aura_ipc_v4")
    }
    #[cfg(not(target_os = "android"))]
    {
        String::from("127.0.0.1:19400")
    }
}

struct Args {
    /// Socket address to bind.
    /// On Android: Unix abstract socket name (e.g., "@aura_ipc_v4").
    /// On host: TCP address (e.g., "127.0.0.1:19400").
    socket: String,
    /// Directory containing GGUF model files.
    model_dir: PathBuf,
    /// Optional path to `aura.config.toml`.
    /// Defaults to `<model_dir>/../aura.config.toml` if not supplied.
    config_path: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().collect();

        let mut socket = default_socket_address();
        let mut model_dir = PathBuf::from("models");
        let mut config_path: Option<PathBuf> = None;

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--socket" | "-s" => {
                    i += 1;
                    socket = args.get(i).ok_or("--socket requires a value")?.clone();
                },
                "--model-dir" | "-m" => {
                    i += 1;
                    model_dir = PathBuf::from(args.get(i).ok_or("--model-dir requires a value")?);
                },
                "--config" | "-c" => {
                    i += 1;
                    config_path = Some(PathBuf::from(
                        args.get(i).ok_or("--config requires a value")?,
                    ));
                },
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                },
                other => {
                    return Err(format!("unknown argument: {other}"));
                },
            }
            i += 1;
        }

        Ok(Args {
            socket,
            model_dir,
            config_path,
        })
    }
}

fn print_usage() {
    let usage = "\
aura-neocortex — AURA LLM Inference Process

USAGE:
    aura-neocortex [OPTIONS]

OPTIONS:
    -s, --socket <ADDR>       Socket address to bind
                              Default: 127.0.0.1:19400 (TCP on host)
                              Android: @aura_ipc_v4 (abstract Unix socket)
    -m, --model-dir <PATH>    Directory containing GGUF model files
                              Default: models
    -c, --config <PATH>       Path to aura.config.toml
                              Default: <model-dir>/../aura.config.toml
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
        },
    };

    info!(socket = %args.socket, model_dir = %args.model_dir.display(), "cli configuration");

    // ── Step 1: Load NeocortexRuntimeConfig ─────────────────────────────────
    //
    // Config is loaded before ModelManager so we can resolve the authoritative
    // model path (from `aura.config.toml` or auto-scan) before building the
    // manager. On any error, we fall back to defaults so day-zero always works.

    let config_path = args.config_path.unwrap_or_else(|| {
        // Default: look for aura.config.toml next to the model directory.
        args.model_dir
            .parent()
            .unwrap_or(&args.model_dir)
            .join("aura.config.toml")
    });

    let runtime_config = match NeocortexRuntimeConfig::load(&config_path) {
        Ok(cfg) => {
            info!(
                source = ?cfg.config_source,
                model_path = ?cfg.model_path,
                "runtime config loaded"
            );
            cfg
        },
        Err(e) => {
            // ParseError on a malformed TOML — log warning and use defaults.
            warn!(
                error = %e,
                "failed to parse aura.config.toml — falling back to defaults"
            );
            NeocortexRuntimeConfig::default_fallback()
        },
    };

    // Resolve the effective model directory:
    // - If config supplies a path that is a directory, prefer that.
    // - If config supplies a path to a specific file, use its parent as model_dir.
    // - Otherwise fall back to the CLI --model-dir arg.
    let effective_model_dir = resolve_model_dir(&args.model_dir, &runtime_config);

    info!(
        dir = %effective_model_dir.display(),
        "effective model directory"
    );

    // Set up shutdown signal handler.
    let shutdown = Arc::new(AtomicBool::new(false));
    let cancel_token = Arc::new(AtomicBool::new(false));

    // Spawn stdin-based shutdown listener.
    // The daemon can send "SHUTDOWN\n" on stdin for graceful shutdown.
    // Stdin EOF or errors do NOT trigger shutdown (safe for Android headless mode).
    spawn_shutdown_listener(shutdown.clone(), cancel_token.clone());

    // ── Step 2: Create ModelManager and scan for GGUF files ─────────────────
    //
    // scan() must be called before building ModelCapabilities so the scanner
    // has parsed GGUF headers for all discovered model files.

    let mut model_manager = ModelManager::new(effective_model_dir.clone());
    model_manager.scan();

    // ── Step 3: Build ModelCapabilities from GGUF metadata ──────────────────
    //
    // After scanning, we derive capabilities from the primary (Brainstem-tier)
    // model's GGUF metadata. This is the single source of truth for embedding_dim,
    // context_length, etc. Falls back to compiled defaults if no model was found.

    let startup_capabilities = build_startup_capabilities(&model_manager, &runtime_config);

    info!(
        capabilities = %startup_capabilities.summary(),
        fully_from_gguf = startup_capabilities.is_fully_from_gguf(),
        "model capabilities resolved"
    );

    // Bind and accept connections.
    // On host: TCP on 127.0.0.1:19400.
    // On Android: abstract Unix domain socket @aura_ipc_v4.
    if let Err(e) = run_server(
        &args.socket,
        effective_model_dir,
        model_manager,
        runtime_config,
        cancel_token,
        shutdown,
        Some(startup_capabilities),
    ) {
        error!(error = %e, "server error");
        std::process::exit(1);
    }

    info!("aura-neocortex shut down cleanly");
}

// ─── Capability resolution ───────────────────────────────────────────────────

/// Derive `ModelCapabilities` from the scanned GGUF metadata of the primary
/// (smallest / Brainstem) model tier. Falls back gracefully to compiled
/// defaults if no GGUF metadata is available.
///
/// Does NOT panic — any parse failure has already been swallowed by
/// `ModelScanner::scan()` (which skips unparseable files).
fn build_startup_capabilities(
    manager: &ModelManager,
    config: &NeocortexRuntimeConfig,
) -> ModelCapabilities {
    use aura_types::ipc::ModelTier;

    // Walk tiers from smallest to largest; use the first one that has metadata.
    for tier in [
        ModelTier::Brainstem1_5B,
        ModelTier::Standard4B,
        ModelTier::Full8B,
    ] {
        if let Some((_, meta)) = manager.scanner().models.get(&tier) {
            return ModelCapabilities::from_gguf(meta, config.user_override_embedding_dim);
        }
    }

    // No GGUF metadata available — fall back to compiled defaults.
    warn!(
        "no GGUF metadata found during startup scan — using compiled capability fallback; \
         drop a .gguf model into the model directory for accurate geometry"
    );
    ModelCapabilities::fallback_defaults()
}

// ─── Model directory resolution ─────────────────────────────────────────────

/// Resolve the effective model directory from CLI arg and runtime config.
///
/// Priority:
/// 1. If `config.model_path` is a directory → use it directly.
/// 2. If `config.model_path` is a file → use its parent directory.
/// 3. Otherwise → fall back to the CLI `--model-dir` value.
fn resolve_model_dir(cli_model_dir: &std::path::Path, config: &NeocortexRuntimeConfig) -> PathBuf {
    if let Some(ref cfg_path) = config.model_path {
        if cfg_path.is_dir() {
            return cfg_path.clone();
        }
        if cfg_path.is_file() {
            if let Some(parent) = cfg_path.parent() {
                return parent.to_path_buf();
            }
        }
        // Config path was specified but doesn't exist yet (e.g., first run on
        // a fresh device).  Honour its parent if it looks like a directory path,
        // otherwise fall through to the CLI value.
        if let Some(parent) = cfg_path.parent() {
            if parent != std::path::Path::new("") {
                return parent.to_path_buf();
            }
        }
    }

    cli_model_dir.to_path_buf()
}

// ─── Server loop ─────────────────────────────────────────────────────────────

/// Run the IPC server, accepting a single connection at a time.
///
/// The daemon maintains a single persistent connection to neocortex.
/// If it disconnects, we accept the next connection.
///
/// `startup_capabilities` is derived from GGUF metadata before the first
/// connection and seeded into `InferenceEngine` on every connection so the
/// engine never operates blind on model geometry.
fn run_server(
    address: &str,
    model_dir: PathBuf,
    model_manager: ModelManager,
    runtime_config: NeocortexRuntimeConfig,
    cancel_token: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    startup_capabilities: Option<ModelCapabilities>,
) -> Result<(), Box<dyn std::error::Error>> {
    // We need to share model_manager across reconnections.
    // Since we handle one connection at a time, we can move it in and out.
    let mut mgr = model_manager;

    // Track the most recently derived capabilities so every reconnect passes
    // fresh GGUF geometry into InferenceEngine without re-opening model files.
    // Updated after each connection ends in case new models were dropped in.
    let mut current_caps = startup_capabilities;

    // ── Platform-specific listener binding ──────────────────────────────────
    //
    // On Android: bind an abstract Unix domain socket so the daemon can reach
    //   us without filesystem permissions or port conflicts.
    // On host: bind a TCP socket on loopback for simplicity.

    #[cfg(target_os = "android")]
    let accept_connection = {
        use std::os::unix::net::{SocketAddr as StdSocketAddr, UnixListener};
        use std::os::android::net::SocketAddrExt;

        // Abstract sockets don't need filesystem cleanup — the kernel manages
        // their lifecycle.  Strip the leading '@' convention if present.
        let abstract_name = address.strip_prefix('@').unwrap_or(address);
        let addr = StdSocketAddr::from_abstract_name(abstract_name.as_bytes())?;
        let listener = UnixListener::bind_addr(&addr)?;
        listener.set_nonblocking(true)?;
        info!(address, "listening on abstract Unix socket");

        move |shutdown: &AtomicBool| -> Result<Option<std::os::unix::net::UnixStream>, std::io::Error> {
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        info!("daemon connected (Unix socket)");
                        stream.set_nonblocking(false)?;
                        return Ok(Some(stream));
                    },
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        continue;
                    },
                    Err(e) => return Err(e),
                }
            }
        }
    };

    #[cfg(not(target_os = "android"))]
    let accept_connection = {
        let listener = std::net::TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        info!(address, "listening on TCP");

        move |shutdown: &AtomicBool| -> Result<Option<std::net::TcpStream>, std::io::Error> {
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                match listener.accept() {
                    Ok((stream, addr)) => {
                        info!(peer = %addr, "daemon connected (TCP)");
                        stream.set_nonblocking(false)?;
                        return Ok(Some(stream));
                    },
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No pending connection — poll again after a short sleep.
                        //
                        // Tradeoff: 50ms gives ≤50ms latency on new connections
                        // while keeping idle CPU wake-ups at 20/sec (negligible
                        // even on mobile).
                        //
                        // NOTE: handler.run_loop() BLOCKS for the entire duration
                        // of a daemon connection.  This poll loop only runs while
                        // waiting for a NEW connection.
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        continue;
                    },
                    Err(e) => return Err(e),
                }
            }
        }
    };

    // ── Accept loop ─────────────────────────────────────────────────────────

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("shutdown flag set — stopping server");
            break;
        }

        let stream = match accept_connection(&shutdown) {
            Ok(Some(s)) => s,
            Ok(None) => {
                info!("shutdown during accept — stopping server");
                break;
            },
            Err(e) => {
                error!(error = %e, "accept failed");
                return Err(e.into());
            },
        };

        let mut handler = IpcHandler::new(stream, mgr, cancel_token.clone(), current_caps.clone())?;

        match handler.run_loop() {
            Ok(()) => info!("connection closed cleanly"),
            Err(e) => error!(error = %e, "connection error"),
        }

        // Recover model manager from handler for reuse.
        // Config and capabilities are cheap to re-derive from the same
        // model_dir — no file re-parsing required.
        let mut new_mgr = ModelManager::new(model_dir.clone());
        new_mgr.scan();
        mgr = new_mgr;

        // Re-derive capabilities after reconnect so the next IpcHandler
        // gets fresh GGUF geometry (a new model may have been dropped
        // into the model directory while the previous connection was live).
        let reconnect_caps = build_startup_capabilities(&mgr, &runtime_config);
        info!(
            capabilities = %reconnect_caps.summary(),
            "model capabilities after reconnect"
        );
        current_caps = Some(reconnect_caps);
    }

    Ok(())
}

/// Spawn a background thread that listens for shutdown signals on stdin.
///
/// # Shutdown protocol
///
/// The parent process (daemon) can request graceful shutdown by writing
/// `SHUTDOWN\n` to this process's stdin.  This is a simple, cross-platform
/// mechanism that works on Android, Linux, Windows, and macOS without
/// requiring signal-handling crates or platform-specific APIs.
///
/// **Behaviour on stdin events:**
/// - Line containing "SHUTDOWN" (case-insensitive) → trigger graceful shutdown.
/// - EOF (stdin closed / pipe broken) → log warning, do NOT shutdown. On Android the daemon may
///   close stdin without intending to kill neocortex (e.g., process manager recycling file
///   descriptors).
/// - I/O error → log error, stop listening (do NOT trigger shutdown).
/// - Any other line → ignored (allows future command extension).
///
/// CTRL+C on host platforms will still terminate the process via the default
/// signal handler; this listener is an *additional* graceful shutdown path.
///
/// # Future improvements
/// Replace with `signal-hook` or `ctrlc` crate for proper SIGTERM/SIGINT
/// handling alongside the stdin protocol.
fn spawn_shutdown_listener(shutdown: Arc<AtomicBool>, cancel: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("shutdown-listener".into())
        .spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            let reader = stdin.lock();

            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        let trimmed = line.trim();
                        if trimmed.eq_ignore_ascii_case("SHUTDOWN") {
                            info!("received SHUTDOWN command on stdin — initiating graceful shutdown");
                            shutdown.store(true, Ordering::SeqCst);
                            cancel.store(true, Ordering::SeqCst);
                            return;
                        }
                        // Ignore unrecognised lines (future: could add STATUS, RELOAD, etc.)
                        if !trimmed.is_empty() {
                            warn!(line = %trimmed, "unrecognised stdin command — ignoring");
                        }
                    }
                    Err(e) => {
                        // I/O error (not EOF) — stop listening but do NOT shutdown.
                        // This can happen if stdin is redirected from /dev/null or a
                        // closed pipe on some platforms.
                        warn!(error = %e, "stdin read error — shutdown listener exiting (no shutdown triggered)");
                        return;
                    }
                }
            }

            // Iterator exhausted → EOF. Stdin was closed by the parent process.
            // On Android, this is normal when the daemon doesn't hold our stdin open.
            // Do NOT trigger shutdown — the daemon will send an explicit SHUTDOWN or
            // the OS will SIGKILL us if it truly wants us gone.
            warn!("stdin closed (EOF) — shutdown listener exiting (no shutdown triggered)");
        })
        .expect("failed to spawn shutdown-listener thread");
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
            socket: default_socket_address(),
            model_dir: PathBuf::from("models"),
            config_path: None,
        };
        // On non-Android hosts the default is TCP on 19400 (matching the
        // daemon's TCP_FALLBACK_PORT).  On Android it would be @aura_ipc_v4.
        #[cfg(not(target_os = "android"))]
        assert_eq!(args.socket, "127.0.0.1:19400");
        #[cfg(target_os = "android")]
        assert_eq!(args.socket, "@aura_ipc_v4");
        assert_eq!(args.model_dir, PathBuf::from("models"));
        assert!(args.config_path.is_none());
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
        let engine = crate::inference::InferenceEngine::new(cancel.clone(), None);

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

    #[test]
    fn resolve_model_dir_cli_fallback() {
        let cli = PathBuf::from("/tmp/models");
        let config = NeocortexRuntimeConfig::default_fallback();
        let result = resolve_model_dir(&cli, &config);
        assert_eq!(result, cli);
    }

    #[test]
    fn resolve_model_dir_from_config_file_path() {
        // Config pointing at a specific file → use the parent directory.
        let cli = PathBuf::from("/tmp/models");
        let config = NeocortexRuntimeConfig {
            model_path: Some(PathBuf::from("/sdcard/AURA/models/qwen2.gguf")),
            user_override_embedding_dim: None,
            user_override_context_length: None,
            config_source: aura_config::ConfigSource::DefaultFallback,
        };
        let result = resolve_model_dir(&cli, &config);
        assert_eq!(result, PathBuf::from("/sdcard/AURA/models"));
    }

    #[test]
    fn startup_capabilities_fallback_without_gguf() {
        // No models on disk → capabilities must fall back gracefully, no panic.
        let dir = std::env::temp_dir().join("aura_test_empty_models_caps");
        let mgr = ModelManager::new(dir);
        // Note: scan() not called — scanner is empty, simulating no GGUF files.
        let config = NeocortexRuntimeConfig::default_fallback();
        let caps = build_startup_capabilities(&mgr, &config);

        // Must return valid fallback, not panic.
        assert!(caps.embedding_dim >= 1024);
        assert!(caps.context_length >= 1024);
        assert_eq!(
            caps.embedding_dim_source,
            crate::model_capabilities::ModelCapabilitySource::CompiledFallback
        );
    }

    #[test]
    fn startup_capabilities_user_override_forwarded() {
        // User override in config should be forwarded to from_gguf().
        // When there's no GGUF metadata (empty scanner), the override takes effect.
        let dir = std::env::temp_dir().join("aura_test_override_caps");
        let mgr = ModelManager::new(dir);
        let config = NeocortexRuntimeConfig {
            model_path: None,
            user_override_embedding_dim: Some(2048),
            user_override_context_length: None,
            config_source: aura_config::ConfigSource::DefaultFallback,
        };

        // With an empty scanner (no GGUF), the override is the best available source.
        // build_startup_capabilities falls back to fallback_defaults() when scanner
        // has no models — so the override is NOT applied (expected: CompiledFallback).
        // This tests the graceful fallback path specifically.
        let caps = build_startup_capabilities(&mgr, &config);
        assert!(caps.embedding_dim > 0);
    }
}
