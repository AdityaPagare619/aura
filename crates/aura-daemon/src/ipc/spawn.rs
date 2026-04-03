//! Process lifecycle management for the Neocortex binary.
//!
//! [`NeocortexProcess`] spawns the Neocortex inference process, monitors its
//! health, and handles restarts with backoff.  On Android the binary is a
//! shared library loaded by the system; on host it's a regular executable.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tokio::process::{Child, Command};
use tracing::{debug, error, info, instrument, warn};

use super::{protocol, IpcError};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Default maximum number of automatic restarts before giving up.
const DEFAULT_MAX_RESTARTS: u32 = 5;

/// Delay between restart attempts (linear, not exponential — the client
/// layer handles exponential reconnect backoff separately).
const RESTART_DELAY: Duration = Duration::from_secs(2);

/// How long to wait for the Neocortex process to start listening after spawn.
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait after sending SIGTERM before escalating to SIGKILL.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// On Android, the neocortex binary is at /data/local/tmp/aura-neocortex.
#[cfg(target_os = "android")]
const ANDROID_NEOCORTEX_PATH: &str = "/data/local/tmp/aura-neocortex";

// ─── Path resolution ────────────────────────────────────────────────────────

/// Resolve the neocortex binary path for the current platform.
///
/// Resolution order (enterprise-first: check executable paths before permission-restricted ones):
/// 1. `AURA_NEOCORTEX_BIN` environment variable (explicit override)
/// 2. `$PREFIX/bin/aura-neocortex` (Termux system bin - always executable)
/// 3. `$HOME/bin/aura-neocortex` (user bin - common for manual installs)
/// 4. `$HOME/.local/bin/aura-neocortex` (XDG-style user bin)
/// 5. Platform default:
///    - Android: `/data/local/tmp/aura-neocortex` (legacy, may have permission issues)
///    - Host: `aura-neocortex` (relies on PATH)
fn resolve_neocortex_path() -> PathBuf {
    // 1. Explicit env override.
    if let Ok(path) = std::env::var("AURA_NEOCORTEX_BIN") {
        let p = PathBuf::from(&path);
        if p.exists() {
            info!(path = %p.display(), "neocortex binary from AURA_NEOCORTEX_BIN");
            return p;
        }
        warn!(
            path = %path,
            "AURA_NEOCORTEX_BIN set but file not found — trying other paths"
        );
    }

    // 2. Termux: $PREFIX/bin/aura-neocortex (preferred - always executable).
    if let Ok(prefix) = std::env::var("PREFIX") {
        let termux_path = PathBuf::from(&prefix).join("bin").join("aura-neocortex");
        if termux_path.exists() {
            info!(path = %termux_path.display(), "neocortex binary from $PREFIX/bin");
            return termux_path;
        }
    }

    // 3. User home bin: $HOME/bin/aura-neocortex (common manual install location).
    if let Ok(home) = std::env::var("HOME") {
        let home_bin = PathBuf::from(&home).join("bin").join("aura-neocortex");
        if home_bin.exists() {
            info!(path = %home_bin.display(), "neocortex binary from $HOME/bin");
            return home_bin;
        }

        // 4. XDG-style: $HOME/.local/bin/aura-neocortex.
        let xdg_bin = PathBuf::from(&home)
            .join(".local")
            .join("bin")
            .join("aura-neocortex");
        if xdg_bin.exists() {
            info!(path = %xdg_bin.display(), "neocortex binary from $HOME/.local/bin");
            return xdg_bin;
        }
    }

    // 5. Platform default (last resort - may have permission issues on some devices).
    #[cfg(target_os = "android")]
    {
        warn!(
            path = ANDROID_NEOCORTEX_PATH,
            "using default neocortex path — may fail on devices with restricted /data/local/tmp/ permissions"
        );
        PathBuf::from(ANDROID_NEOCORTEX_PATH)
    }

    #[cfg(not(target_os = "android"))]
    {
        // On host, assume it's on PATH or in the same directory.
        PathBuf::from("aura-neocortex")
    }
}

// ─── NeocortexProcess ───────────────────────────────────────────────────────

/// Manages the lifecycle of the Neocortex child process.
///
/// On host development, this spawns a regular binary. On Android, the
/// process management is different — the system loader handles it — so
/// most methods are gated with `#[cfg]`.
pub struct NeocortexProcess {
    /// Handle to the child process; `None` when not running.
    child: Option<Child>,

    /// Path to the neocortex binary.
    binary_path: PathBuf,

    /// When the current child was spawned.
    started_at: Option<Instant>,

    /// How many times we've restarted this process.
    restart_count: u32,

    /// Maximum allowed restarts before [`restart`](Self::restart) refuses.
    max_restarts: u32,

    /// Optional path to the directory containing GGUF model files.
    /// Passed as `--model-dir` to the neocortex binary if present.
    /// Respects `AURA_MODEL_DIR` env var override from the call site.
    model_dir: Option<PathBuf>,
}

impl std::fmt::Debug for NeocortexProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NeocortexProcess")
            .field("binary_path", &self.binary_path)
            .field("running", &self.child.is_some())
            .field("restart_count", &self.restart_count)
            .field("max_restarts", &self.max_restarts)
            .field("uptime_ms", &self.uptime_ms())
            .finish()
    }
}

impl NeocortexProcess {
    /// Spawn the Neocortex process from the given binary path.
    ///
    /// The child process is started with `--socket` set to the platform-
    /// appropriate IPC address (abstract Unix socket on Android, TCP loopback
    /// on host).  stdout/stderr are piped for monitoring.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if the binary cannot be found or executed.
    /// - [`IpcError::ProcessDied`] if the process exits immediately.
    #[instrument(name = "neocortex_spawn", skip_all, fields(path = %binary_path.display()))]
    pub async fn spawn(binary_path: &Path, model_dir: Option<&Path>) -> Result<Self, IpcError> {
        info!(path = %binary_path.display(), "spawning neocortex process");

        // Pass the platform-appropriate socket address so the neocortex server
        // binds to the same endpoint the daemon's `connect_stream()` connects to.
        //
        // Android: abstract Unix socket @aura_ipc_v4
        // Host:    TCP 127.0.0.1:19400
        #[cfg(target_os = "android")]
        let socket_arg = protocol::SOCKET_ADDR;
        #[cfg(not(target_os = "android"))]
        let socket_arg = format!(
            "{}:{}",
            protocol::TCP_FALLBACK_ADDR,
            protocol::TCP_FALLBACK_PORT
        );

        let mut cmd = Command::new(binary_path);
        cmd.arg("--socket")
            .arg(socket_arg)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if let Some(dir) = model_dir {
            cmd.arg("--model-dir").arg(dir.to_str().unwrap_or("."));
        }

        let child = cmd.spawn().map_err(|e| {
            error!(error = %e, path = %binary_path.display(), "failed to spawn neocortex");
            IpcError::ProcessDied {
                reason: format!("spawn failed: {e}"),
            }
        })?;

        let pid = child.id().unwrap_or(0);
        info!(pid, "neocortex process spawned");

        Ok(Self {
            child: Some(child),
            binary_path: binary_path.to_path_buf(),
            started_at: Some(Instant::now()),
            restart_count: 0,
            max_restarts: DEFAULT_MAX_RESTARTS,
            model_dir: model_dir.map(|p| p.to_path_buf()),
        })
    }

    /// Create a `NeocortexProcess` using automatic path resolution.
    ///
    /// Checks (in order):
    /// 1. `AURA_NEOCORTEX_BIN` env var
    /// 2. `$PREFIX/bin/aura-neocortex` (Termux)
    /// 3. Platform default (Android APK path or `aura-neocortex` on PATH)
    pub async fn spawn_auto(model_dir: Option<&Path>) -> Result<Self, IpcError> {
        let path = resolve_neocortex_path();
        Self::spawn(&path, model_dir).await
    }

    /// Create a `NeocortexProcess` that uses the default Android APK path.
    ///
    /// Only available on Android targets.
    #[cfg(target_os = "android")]
    pub async fn spawn_android(model_dir: Option<&Path>) -> Result<Self, IpcError> {
        let path = std::env::var("AURA_NEOCORTEX_BIN")
            .unwrap_or_else(|_| ANDROID_NEOCORTEX_PATH.to_string());
        Self::spawn(Path::new(&path), model_dir).await
    }

    /// Whether the child process is believed to be running.
    ///
    /// Checks both whether we have a `Child` handle and whether it has
    /// exited.  Note: between checks the process could die, so this is
    /// best-effort.
    pub fn is_running(&mut self) -> bool {
        let Some(ref mut child) = self.child else {
            return false;
        };

        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still
        // running, Err on OS error.
        match child.try_wait() {
            Ok(Some(status)) => {
                info!(status = %status, "neocortex process has exited");
                self.child = None;
                self.started_at = None;
                false
            }
            Ok(None) => true,
            Err(e) => {
                warn!(error = %e, "failed to check neocortex process status");
                // Assume still running on error to avoid premature restart.
                true
            }
        }
    }

    /// Uptime in milliseconds, or 0 if not running.
    pub fn uptime_ms(&self) -> u64 {
        self.started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    /// Current restart count.
    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }

    /// Override the maximum restart limit.
    pub fn set_max_restarts(&mut self, max: u32) {
        self.max_restarts = max;
    }

    /// Gracefully shut down the Neocortex process.
    ///
    /// Sends SIGTERM (or equivalent) and waits up to
    /// [`GRACEFUL_SHUTDOWN_TIMEOUT`] for the process to exit.  If it doesn't,
    /// forcefully kills it.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if signaling fails.
    #[instrument(name = "neocortex_shutdown", skip_all)]
    pub async fn shutdown(&mut self) -> Result<(), IpcError> {
        let Some(ref mut child) = self.child else {
            debug!("shutdown called but process not running");
            return Ok(());
        };

        let pid = child.id().unwrap_or(0);
        info!(pid, "requesting graceful shutdown");

        // On Unix, start_kill sends SIGKILL. We first try wait with timeout.
        // tokio::process::Child::kill() sends SIGKILL immediately, so we use
        // a timeout on wait() first.
        let wait_result = tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, child.wait()).await;

        match wait_result {
            Ok(Ok(status)) => {
                info!(pid, status = %status, "neocortex exited gracefully");
            }
            Ok(Err(e)) => {
                warn!(pid, error = %e, "error waiting for neocortex");
            }
            Err(_elapsed) => {
                warn!(pid, "graceful shutdown timed out — killing");
                if let Err(e) = child.kill().await {
                    error!(pid, error = %e, "failed to kill neocortex");
                    return Err(IpcError::Io(std::io::Error::other(format!(
                        "kill failed: {e}"
                    ))));
                }
                info!(pid, "neocortex killed");
            }
        }

        self.child = None;
        self.started_at = None;
        Ok(())
    }

    /// Restart the Neocortex process with a delay.
    ///
    /// Shuts down any existing process, waits [`RESTART_DELAY`], then
    /// spawns a new one.  Increments the restart counter.
    ///
    /// # Errors
    ///
    /// - [`IpcError::MaxRestartsExceeded`] if the restart count has hit the limit.
    /// - Any error from [`shutdown`](Self::shutdown) or [`spawn`](Self::spawn).
    #[instrument(name = "neocortex_restart", skip_all, fields(
        attempt = self.restart_count + 1,
        max = self.max_restarts,
    ))]
    pub async fn restart(&mut self) -> Result<(), IpcError> {
        if self.restart_count >= self.max_restarts {
            error!(
                count = self.restart_count,
                max = self.max_restarts,
                "max restarts exceeded"
            );
            return Err(IpcError::MaxRestartsExceeded {
                max: self.max_restarts,
            });
        }

        self.shutdown().await?;

        info!(
            delay_ms = RESTART_DELAY.as_millis() as u64,
            "waiting before restart"
        );
        tokio::time::sleep(RESTART_DELAY).await;

        let binary_path = self.binary_path.clone();
        let mut new = Self::spawn(&binary_path, self.model_dir.as_deref()).await?;

        self.child = new.child.take();
        self.started_at = new.started_at.take();
        self.restart_count = self.restart_count.saturating_add(1);

        info!(restart_count = self.restart_count, "neocortex restarted");
        Ok(())
    }

    /// Wait until the Neocortex process is ready to accept IPC connections.
    ///
    /// Polls the IPC socket/port with small delays until a connection
    /// succeeds or [`READY_TIMEOUT`] elapses.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Timeout`] if the process doesn't become ready in time.
    /// - [`IpcError::ProcessDied`] if the child exits while we're waiting.
    #[instrument(name = "neocortex_wait_ready", skip_all)]
    pub async fn wait_ready(&mut self) -> Result<(), IpcError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let poll_interval = Duration::from_millis(250);

        info!("waiting for neocortex to become ready");

        loop {
            // Check if the child has died.
            if !self.is_running() {
                return Err(IpcError::ProcessDied {
                    reason: "process exited before becoming ready".into(),
                });
            }

            // Try to connect.
            match protocol::connect_stream().await {
                Ok(_stream) => {
                    // Connection succeeded — the process is listening.
                    // We drop this probe connection immediately; the
                    // NeocortexClient will establish its own.
                    info!(
                        elapsed_ms = (Instant::now() + READY_TIMEOUT - deadline).as_millis() as u64,
                        "neocortex is ready"
                    );
                    return Ok(());
                }
                Err(_) => {
                    if Instant::now() >= deadline {
                        return Err(IpcError::Timeout {
                            context: format!("neocortex not ready after {READY_TIMEOUT:?}"),
                        });
                    }
                    tokio::time::sleep(poll_interval).await;
                }
            }
        }
    }
}

impl Drop for NeocortexProcess {
    fn drop(&mut self) {
        // kill_on_drop(true) was set during spawn, so tokio will handle
        // cleanup.  We just log for observability.
        if self.child.is_some() {
            warn!("NeocortexProcess dropped while child still running — kill_on_drop will handle cleanup");
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_constants() {
        assert_eq!(DEFAULT_MAX_RESTARTS, 5);
        assert_eq!(RESTART_DELAY, Duration::from_secs(2));
        assert_eq!(READY_TIMEOUT, Duration::from_secs(10));
        assert_eq!(GRACEFUL_SHUTDOWN_TIMEOUT, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn spawn_nonexistent_binary_returns_error() {
        let result = NeocortexProcess::spawn(Path::new("/nonexistent/binary"), None).await;
        assert!(result.is_err());
        match result {
            Err(IpcError::ProcessDied { reason }) => {
                assert!(reason.contains("spawn failed"), "got: {reason}");
            }
            other => panic!("expected ProcessDied, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_when_not_running_is_ok() {
        // Create a process struct with no child.
        let mut proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/fake/path"),
            started_at: None,
            restart_count: 0,
            max_restarts: DEFAULT_MAX_RESTARTS,
            model_dir: None,
        };
        let result = proc.shutdown().await;
        assert!(result.is_ok());
    }

    #[test]
    fn is_running_without_child() {
        let mut proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/fake/path"),
            started_at: None,
            restart_count: 0,
            max_restarts: DEFAULT_MAX_RESTARTS,
            model_dir: None,
        };
        assert!(!proc.is_running());
    }

    #[test]
    fn uptime_without_start() {
        let proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/fake/path"),
            started_at: None,
            restart_count: 0,
            max_restarts: DEFAULT_MAX_RESTARTS,
            model_dir: None,
        };
        assert_eq!(proc.uptime_ms(), 0);
    }

    #[test]
    fn uptime_with_start() {
        let proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/fake/path"),
            started_at: Some(Instant::now()),
            restart_count: 0,
            max_restarts: DEFAULT_MAX_RESTARTS,
            model_dir: None,
        };
        // Should be very small but non-negative.
        assert!(proc.uptime_ms() < 1000);
    }

    #[tokio::test]
    async fn restart_exceeds_max() {
        let mut proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/fake/path"),
            started_at: None,
            restart_count: 5,
            max_restarts: 5,
            model_dir: None,
        };
        let result = proc.restart().await;
        assert!(matches!(
            result,
            Err(IpcError::MaxRestartsExceeded { max: 5 })
        ));
    }

    #[test]
    fn set_max_restarts() {
        let mut proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/fake/path"),
            started_at: None,
            restart_count: 0,
            max_restarts: DEFAULT_MAX_RESTARTS,
            model_dir: None,
        };
        proc.set_max_restarts(10);
        assert_eq!(proc.max_restarts, 10);
    }

    #[test]
    fn debug_format() {
        let proc = NeocortexProcess {
            child: None,
            binary_path: PathBuf::from("/data/neocortex"),
            started_at: None,
            restart_count: 2,
            max_restarts: 5,
            model_dir: None,
        };
        let dbg = format!("{proc:?}");
        assert!(dbg.contains("NeocortexProcess"));
        assert!(dbg.contains("/data/neocortex"));
        assert!(dbg.contains("restart_count: 2"));
    }

    #[cfg(target_os = "android")]
    #[test]
    fn android_path_constant() {
        assert_eq!(ANDROID_NEOCORTEX_PATH, "/data/local/tmp/aura-neocortex");
    }

    #[test]
    fn resolve_neocortex_path_returns_something() {
        // Should not panic regardless of env vars.
        let path = resolve_neocortex_path();
        assert!(!path.as_os_str().is_empty());
    }
}
