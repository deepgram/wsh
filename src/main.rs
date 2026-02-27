//! wsh - The Web Shell
//!
//! A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
//!
//! ## Modes
//!
//! **Default** (no subcommand): Connects to an existing server (or auto-spawns
//! an ephemeral one), creates a session, and attaches — acting as a thin
//! terminal client.
//!
//! **Server mode** (`wsh server`): Starts a headless daemon with HTTP/WS and
//! Unix socket listeners. Runs in persistent mode by default (stays alive when
//! sessions end). Use `--ephemeral` to exit when the last session ends.

use clap::{Parser as ClapParser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wsh::{
    api, client, protocol,
    protocol::{AttachSessionMsg, ScrollbackRequest},
    server,
    session::SessionRegistry,
    shutdown::ShutdownCoordinator,
    terminal,
};

/// wsh - The Web Shell
///
/// A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
/// Run your shell inside wsh to access it from web browsers, agents, and other tools.
#[derive(ClapParser, Debug)]
#[command(name = "wsh", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Address to bind the HTTP/WebSocket API server
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,

    /// Command string to execute (like sh -c)
    #[arg(short = 'c')]
    cmd: Option<String>,

    /// Force interactive mode
    #[arg(short = 'i')]
    interactive: bool,

    /// Authentication token for non-localhost bindings
    #[arg(long, env = "WSH_TOKEN")]
    token: Option<String>,

    /// Shell to spawn (overrides $SHELL)
    #[arg(long)]
    shell: Option<String>,

    /// Name for the initial session
    #[arg(long)]
    name: Option<String>,

    /// Tags for the initial session (can be specified multiple times)
    #[arg(long = "tag")]
    tags: Vec<String>,

    /// Use alternate screen buffer (restores previous screen on exit, but
    /// disables native terminal scrollback while wsh is running)
    #[arg(long)]
    alt_screen: bool,

    /// Server instance name (like tmux -L). Each instance gets its own socket.
    #[arg(short = 'L', long = "server-name", env = "WSH_SERVER_NAME", default_value = "default", global = true)]
    server_name: String,

    /// Path to the Unix domain socket (overrides -L)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the wsh server daemon (headless, no local terminal)
    Server {
        /// Address to bind the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token for non-localhost bindings
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,

        /// Run in ephemeral mode (exit when last session ends).
        /// By default, `wsh server` runs in persistent mode.
        #[arg(long)]
        ephemeral: bool,

        /// Maximum number of concurrent sessions (no limit if omitted)
        #[arg(long)]
        max_sessions: Option<usize>,

        /// Allowed CORS origins (can be specified multiple times)
        #[arg(long = "cors-origin")]
        cors_origins: Vec<String>,

        /// Rate limit in requests per second (disabled if omitted)
        #[arg(long)]
        rate_limit: Option<u32>,

        /// Path to federation config file (TOML)
        #[arg(long, env = "WSH_CONFIG")]
        config: Option<PathBuf>,

        /// Override system hostname for server identity
        #[arg(long, env = "WSH_HOSTNAME")]
        hostname: Option<String>,
    },

    /// Attach to an existing session on the server
    Attach {
        /// Session name to attach to
        name: String,

        /// Scrollback to replay: "all", "none", or a number of lines
        #[arg(long, default_value = "all")]
        scrollback: String,

        /// Use alternate screen buffer (restores previous screen on exit, but
        /// disables native terminal scrollback while wsh is running)
        #[arg(long)]
        alt_screen: bool,
    },

    /// List active sessions on the server
    List {},

    /// Kill (destroy) a session on the server
    Kill {
        /// Session name to kill
        name: String,
    },

    /// Detach all clients from a session (session stays alive)
    Detach {
        /// Session name to detach
        name: String,
    },

    /// Query or set server persistence mode.
    ///
    /// With no argument, prints the current persistence state.
    /// `wsh persist on` — server stays alive when all sessions end.
    /// `wsh persist off` — server exits when the last session ends.
    Persist {
        /// "on" or "off". Omit to query without changing.
        value: Option<String>,

        /// Address of the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
    },

    /// Print the server's auth token (retrieved via Unix socket)
    Token {},

    /// Manage tags on a session
    Tag {
        /// Session name
        name: String,

        /// Tags to add
        #[arg(long = "add")]
        add: Vec<String>,

        /// Tags to remove
        #[arg(long = "remove")]
        remove: Vec<String>,
    },

    /// Stop the running wsh server
    Stop {},

    /// Start an MCP server over stdio (for AI hosts like Claude Desktop)
    Mcp {
        /// Address to bind the HTTP/WebSocket API server (for auto-spawn)
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
    },
}

#[derive(Error, Debug)]
pub enum WshError {
    #[error("pty error: {0}")]
    Pty(#[from] wsh::pty::PtyError),

    #[error("terminal error: {0}")]
    Terminal(#[from] terminal::TerminalError),

    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("configuration error: {0}")]
    Config(String),
}

fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Resolve the Unix socket path from explicit `--socket` or `-L` server name.
///
/// `--socket` takes priority; if absent, derives from the server name.
fn resolve_socket_path(socket: Option<PathBuf>, server_name: &str) -> PathBuf {
    socket.unwrap_or_else(|| server::socket_path_for_instance(server_name))
}

/// Minimum token length for non-localhost bindings.  Tokens shorter than this
/// are rejected to prevent accidental auth bypass (e.g. `WSH_TOKEN=""`).
const MIN_TOKEN_LENGTH: usize = 16;

fn resolve_token(bind: &SocketAddr, user_token: &Option<String>) -> Result<Option<String>, WshError> {
    if is_loopback(bind) {
        return Ok(None);
    }
    match user_token {
        Some(token) if token.len() >= MIN_TOKEN_LENGTH => Ok(Some(token.clone())),
        Some(token) => Err(WshError::Config(format!(
            "auth token too short ({} chars, minimum {}). \
             Use a strong token or omit --token to auto-generate one.",
            token.len(),
            MIN_TOKEN_LENGTH,
        ))),
        None => {
            use rand::Rng;
            let token: String = rand::thread_rng()
                .sample_iter(&rand::distributions::Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();
            eprintln!("wsh: API token (required for non-localhost): {}", token);
            Ok(Some(token))
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), WshError> {
    let cli = Cli::parse();

    // MCP mode: tracing must use stderr since stdout is for MCP protocol
    let is_mcp = matches!(cli.command, Some(Commands::Mcp { .. }));
    if is_mcp {
        init_tracing_stderr();
    } else {
        init_tracing();
    }

    // Global args: defined on Cli with `global = true` so they can be
    // passed before or after the subcommand (e.g. `wsh -L foo list`).
    let socket = cli.socket.clone();
    let server_name = cli.server_name.clone();

    match cli.command {
        Some(Commands::Server { bind, token, ephemeral, max_sessions, cors_origins, rate_limit, config, hostname }) => {
            run_server(bind, token, socket, ephemeral, max_sessions, server_name, cors_origins, rate_limit, config, hostname).await
        }
        Some(Commands::Attach { name, scrollback, alt_screen }) => {
            run_attach(name, scrollback, socket, alt_screen, server_name).await
        }
        Some(Commands::List {}) => {
            run_list(socket, server_name).await
        }
        Some(Commands::Kill { name }) => {
            run_kill(name, socket, server_name).await
        }
        Some(Commands::Detach { name }) => {
            run_detach(name, socket, server_name).await
        }
        Some(Commands::Token {}) => {
            run_token(socket, server_name).await
        }
        Some(Commands::Persist { value, bind, token }) => {
            run_persist(value, bind, token).await
        }
        Some(Commands::Tag { name, add, remove }) => {
            run_tag(name, add, remove, socket, server_name).await
        }
        Some(Commands::Stop {}) => {
            run_stop(socket, server_name).await
        }
        Some(Commands::Mcp { bind, token }) => {
            run_mcp(bind, socket, token, server_name).await
        }
        None => {
            run_default(cli).await
        }
    }
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "wsh=info,tower_http=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Initialize tracing with stderr output.
///
/// MCP mode uses stdout for the JSON-RPC protocol, so all tracing MUST go
/// to stderr to avoid corrupting the protocol stream.
fn init_tracing_stderr() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "wsh=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
}

// ── Server mode ────────────────────────────────────────────────────

/// Run the wsh server daemon: HTTP/WS + Unix socket, no local terminal.
async fn run_server(
    bind: SocketAddr,
    token: Option<String>,
    socket: Option<PathBuf>,
    ephemeral: bool,
    max_sessions: Option<usize>,
    server_name: String,
    cors_origins: Vec<String>,
    rate_limit: Option<u32>,
    config_arg: Option<PathBuf>,
    hostname_arg: Option<String>,
) -> Result<(), WshError> {
    tracing::info!(instance = %server_name, "wsh server starting");

    let token = resolve_token(&bind, &token)?;
    if token.is_some() {
        tracing::info!("auth token configured");
    }

    let rate_limit = match rate_limit {
        Some(rps) => Some(rps),
        None if !is_loopback(&bind) => {
            tracing::info!("applying default rate limit (100 req/s per IP) for non-localhost binding");
            Some(100)
        }
        None => None,
    };

    // Resolve config path: CLI arg, else platform config dir
    let config_path = config_arg.unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| {
                // dirs::config_dir() returns None only in minimal environments
                // (no HOME, no XDG_CONFIG_HOME). Fall back to $HOME/.config.
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".config"))
                    .unwrap_or_else(|_| PathBuf::from("/etc"))
            })
            .join("wsh")
            .join("config.toml")
    });

    // Load federation config (optional — missing file is fine)
    let fed_config = wsh::config::FederationConfig::load(&config_path)
        .map_err(|e| eprintln!("Warning: {}", e))
        .ok()
        .flatten();

    // Resolve hostname: CLI arg > config file > system hostname
    let hostname = hostname_arg
        .or_else(|| fed_config.as_ref()?.server.as_ref()?.hostname.clone())
        .unwrap_or_else(|| wsh::config::resolve_hostname(None));

    let fed_config = fed_config.unwrap_or_default();
    tracing::info!(hostname = %hostname, config = %config_path.display(), "server identity resolved");

    // Save default_token before fed_config is consumed by FederationManager.
    let fed_default_token = fed_config.default_token.clone();

    // Create the FederationManager: spawns persistent WebSocket connections
    // for each configured backend server.
    let federation_manager = Arc::new(tokio::sync::Mutex::new(
        wsh::federation::manager::FederationManager::from_config(
            fed_config,
            token.clone(),
            fed_default_token.clone(),
        ),
    ));

    let persistent = !ephemeral;
    // When --max-sessions is explicitly provided, use that value.
    // Otherwise, the registry uses its built-in default (256).
    let sessions = match max_sessions {
        Some(max) => {
            tracing::info!(max_sessions = max, "session limit configured");
            SessionRegistry::with_max_sessions(Some(max))
        }
        None => SessionRegistry::new(),
    };
    let shutdown = ShutdownCoordinator::new();
    let server_config = std::sync::Arc::new(api::ServerConfig::new(persistent));
    let state = api::AppState {
        sessions: sessions.clone(),
        shutdown: shutdown.clone(),
        server_config: server_config.clone(),
        server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: Arc::new(api::ticket::TicketStore::new()),
        backends: federation_manager.lock().await.registry().clone(),
        federation: federation_manager.clone(),
        hostname,
        federation_config_path: if config_path.exists() { Some(config_path) } else { None },
        local_token: token.clone(),
        default_backend_token: fed_default_token,
    };

    if !cors_origins.is_empty() {
        tracing::info!(origins = ?cors_origins, "CORS origins configured");
    }
    if let Some(rps) = rate_limit {
        tracing::info!(rps, "rate limiting configured");
    }

    let socket_token = token.clone();
    let socket_hostname = state.hostname.clone();
    let app = api::router(state, api::RouterConfig { token, bind, cors_origins, rate_limit });

    // Cancellation token for HTTP server shutdown (supports multiple listeners)
    let http_cancel = tokio_util::sync::CancellationToken::new();

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(WshError::Io)?;
    tracing::info!(addr = %bind, "HTTP/WS server listening");

    // When binding to IPv4 loopback, also listen on IPv6 loopback.
    // Browsers (especially Firefox) may resolve "localhost" to ::1 and
    // wait ~30-60s for a TCP timeout before falling back to 127.0.0.1.
    let ipv6_listener = if bind.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
        let v6_addr = std::net::SocketAddr::new(
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            bind.port(),
        );
        match tokio::net::TcpListener::bind(v6_addr).await {
            Ok(l) => {
                tracing::info!(addr = %v6_addr, "HTTP/WS server listening (IPv6 loopback)");
                Some(l)
            }
            Err(e) => {
                tracing::debug!(?e, addr = %v6_addr, "IPv6 loopback bind failed (non-fatal)");
                None
            }
        }
    } else {
        None
    };

    let app_v6 = app.clone();
    let cancel4 = http_cancel.clone();
    let http_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(cancel4.cancelled_owned())
            .await
        {
            tracing::error!(?e, "HTTP server error");
        }
    });

    let http6_handle = ipv6_listener.map(|l| {
        let cancel6 = http_cancel.clone();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(l, app_v6)
                .with_graceful_shutdown(cancel6.cancelled_owned())
                .await
            {
                tracing::error!(?e, "HTTP server error (IPv6)");
            }
        })
    });

    // Acquire instance lock (flock) before binding the socket.
    // The lock file is held for the server's lifetime and released on exit.
    let socket_path = resolve_socket_path(socket, &server_name);
    let lock_path = server::lock_path_for_instance(&server_name);
    let _instance_lock = server::acquire_instance_lock(&lock_path)
        .map_err(WshError::Io)?;

    let socket_path_for_cleanup = socket_path.clone();
    let socket_sessions = sessions.clone();
    let socket_cancel = tokio_util::sync::CancellationToken::new();
    let socket_cancel_clone = socket_cancel.clone();
    let shutdown_request = tokio_util::sync::CancellationToken::new();
    let shutdown_request_clone = shutdown_request.clone();
    let socket_handle = tokio::spawn(async move {
        if let Err(e) = server::serve(socket_sessions, &socket_path, socket_cancel_clone, socket_token, shutdown_request_clone, socket_hostname).await {
            tracing::error!(?e, "Unix socket server error");
        }
    });

    tracing::info!("wsh server ready");

    // Ephemeral shutdown monitor: when the last session exits in non-persistent
    // mode, shut down the server automatically.  Also includes an idle timeout
    // so that an orphaned ephemeral server (client crashed before creating a
    // session) doesn't run forever.
    let config_for_monitor = server_config.clone();
    let sessions_for_monitor = sessions.clone();
    let ephemeral_handle = tokio::spawn(async move {
        let mut events = sessions_for_monitor.subscribe_events();

        if !config_for_monitor.is_persistent() {
            // Give the client 30 seconds to create its first session.
            // If nothing happens, the daemon was likely orphaned.
            let idle_timeout = tokio::time::sleep(std::time::Duration::from_secs(30));
            tokio::pin!(idle_timeout);

            // Wait for either the first event or the idle timeout
            tokio::select! {
                result = events.recv() => {
                    match result {
                        Ok(_) => {} // Got an event, enter normal monitoring
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {} // Lost events, enter normal monitoring
                    }
                }
                _ = &mut idle_timeout => {
                    if sessions_for_monitor.is_empty() {
                        tracing::info!("no sessions created within idle timeout, ephemeral server shutting down");
                        return true;
                    }
                    // Sessions exist somehow, enter normal monitoring
                }
            }
        }

        // Normal monitoring: wait for all sessions to end
        loop {
            match events.recv().await {
                Ok(event) => {
                    let is_removal = matches!(
                        event,
                        wsh::session::SessionEvent::Destroyed { .. }
                    );
                    if is_removal
                        && !config_for_monitor.is_persistent()
                        && sessions_for_monitor.is_empty()
                    {
                        tracing::info!(
                            "last session ended, ephemeral server shutting down"
                        );
                        return true;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "ephemeral monitor lagged on session events");
                    if !config_for_monitor.is_persistent() && sessions_for_monitor.is_empty() {
                        // ── Grace period after lag ────────────────────────
                        //
                        // During rapid session churn (e.g., AI orchestration
                        // creating/destroying many sessions), the registry
                        // may appear empty in the gap between a destroy and
                        // the next create. Wait briefly before committing to
                        // shutdown so a racing create has time to land.
                        // ─────────────────────────────────────────────────
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        if sessions_for_monitor.is_empty() {
                            tracing::info!("last session ended (detected after lag), ephemeral server shutting down");
                            return true;
                        }
                        tracing::debug!("new session appeared during lag grace period, continuing");
                    }
                    continue;
                }
            }
        }
    });

    // Wait for Ctrl+C, SIGTERM, ephemeral shutdown, or `wsh stop` request
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received SIGINT");
        }
        _ = sigterm.recv() => {
            tracing::info!("received SIGTERM");
        }
        result = ephemeral_handle => {
            if let Ok(true) = result {
                tracing::debug!("ephemeral shutdown triggered");
            }
        }
        _ = shutdown_request.cancelled() => {
            tracing::info!("shutdown requested via 'wsh stop'");
        }
    }

    // 1. Stop accepting new connections
    http_cancel.cancel();
    socket_cancel.cancel();

    // Remove the socket file immediately. Once the socket listener is
    // cancelled it will never accept again, so the file is just a stale
    // marker. Removing it early prevents a new spawn attempt from seeing
    // the file, deleting it, failing to bind TCP (still held), and
    // orphaning this server in an unreachable zombie state.
    if socket_path_for_cleanup.exists() {
        let _ = std::fs::remove_file(&socket_path_for_cleanup);
        tracing::debug!(path = %socket_path_for_cleanup.display(), "removed socket file");
    }

    // 2. Signal existing WS handlers to close
    shutdown.shutdown();

    // 3. Wait for all WS connections to close (with timeout)
    let shutdown_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async {
            shutdown.wait_for_all_closed().await;
            // Minimum grace period for non-WS connections (MCP HTTP, etc.)
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    ).await;
    if shutdown_result.is_err() {
        tracing::warn!("shutdown timed out waiting for connections to close");
    }

    // 4. Shut down federation backend connections
    federation_manager.lock().await.shutdown_all().await;

    // 5. Drain sessions (detach clients, SIGHUP children, schedule SIGKILL)
    let kill_handle = sessions.drain();

    // 6. Await server tasks with a timeout. axum's graceful shutdown
    //    waits for all in-flight connections to complete, which can block
    //    indefinitely if a WebSocket or SSE connection is stuck (half-open
    //    TCP, unresponsive client, etc.). Without this timeout the server
    //    holds the TCP port forever in a zombie state — unreachable (socket
    //    gone) but undying (port still bound).
    if tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async {
            if let Err(e) = socket_handle.await {
                tracing::warn!(?e, "socket server task panicked");
            }
            if let Err(e) = http_handle.await {
                tracing::warn!(?e, "HTTP server task panicked");
            }
            if let Some(h) = http6_handle {
                if let Err(e) = h.await {
                    tracing::warn!(?e, "HTTP server task (IPv6) panicked");
                }
            }
        },
    ).await.is_err() {
        tracing::warn!("server tasks did not exit within 5s, abandoning");
    }

    // 7. Wait for SIGKILL escalation to complete (if any sessions were drained)
    if let Some(handle) = kill_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    tracing::info!("wsh server exiting");
    Ok(())
}

// ── MCP stdio mode ─────────────────────────────────────────────────

/// Run the MCP stdio bridge: connect to (or spawn) a server, then bridge
/// stdin/stdout JSON-RPC ↔ the server's `/mcp` Streamable HTTP endpoint.
async fn run_mcp(
    bind: SocketAddr,
    socket: Option<PathBuf>,
    token: Option<String>,
    server_name: String,
) -> Result<(), WshError> {
    tracing::info!("wsh mcp stdio bridge starting");

    let socket_path = resolve_socket_path(socket, &server_name);

    // Connect to existing server or spawn one (with file lock to prevent races)
    match client::Client::connect(&socket_path).await {
        Ok(_) => {
            tracing::debug!("connected to existing server");
        }
        Err(_) => {
            let lock_path = server::spawn_lock_path_for_instance(&server_name);
            let lp = lock_path.clone();
            let _lock = tokio::task::spawn_blocking(move || acquire_spawn_lock(&lp))
                .await
                .map_err(WshError::TaskJoin)??;
            // Re-check after lock
            match client::Client::connect(&socket_path).await {
                Ok(_) => {
                    tracing::debug!("connected to server (spawned by another client)");
                }
                Err(_) => {
                    tracing::debug!("no server running, spawning daemon");
                    spawn_server_daemon(&socket_path, &bind, token.as_deref(), &server_name)?;
                    wait_for_socket(&socket_path).await?;
                }
            }
        }
    }

    let mcp_url = format!("http://{}/mcp", bind);
    // ── Design decision: no total HTTP timeout ────────────────────────
    //
    // The reqwest client has NO per-request timeout here. MCP tools can
    // run for up to MAX_WAIT_CEILING_MS (5 minutes), so any fixed HTTP
    // timeout shorter than that would cause spurious failures for long
    // tool calls. Server-side tools already enforce their own bounded
    // timeouts, so the bridge does not need an additional one.
    //
    // connect_timeout remains at 10s to fail fast if the server is down.
    // ──────────────────────────────────────────────────────────────────
    let http_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client");
    let session_id: Arc<tokio::sync::Mutex<Option<String>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    // Stdout writes are serialized through an Arc<Mutex> so concurrent
    // response tasks can write without interleaving.
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));

    // ── Design decision: concurrent request dispatch ─────────────
    //
    // MCP hosts (e.g., Claude Desktop) may pipeline multiple requests
    // before the first one completes. A sequential bridge would block
    // fast queries (list_sessions, get_screen) behind slow tools
    // (run_command with a 30s wait). We spawn each request into its
    // own task and write responses to stdout as they arrive. JSON-RPC
    // response correlation is handled by the `id` field, so ordering
    // does not matter.
    //
    // Tasks are tracked in a JoinSet so we can drain in-flight requests
    // on EOF, and bounded by a semaphore to prevent unbounded memory
    // growth under sustained pipelining with a slow/unresponsive server.
    // ─────────────────────────────────────────────────────────────
    let mut in_flight = tokio::task::JoinSet::new();
    let concurrency = Arc::new(tokio::sync::Semaphore::new(64));

    let mut line = String::new();
    loop {
        line.clear();
        let n = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
            .await
            .map_err(WshError::Io)?;
        if n == 0 {
            // EOF on stdin
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let body_str = trimmed.to_string();
        let client = http_client.clone();
        let url = mcp_url.clone();
        let sid = session_id.clone();
        let tok = token.clone();
        let out = stdout.clone();
        let sem = concurrency.clone();

        in_flight.spawn(async move {
            // Acquire permit before dispatching; dropped when the task completes.
            let _permit = sem.acquire().await;
            mcp_bridge_dispatch(body_str, client, url, sid, tok, out).await;
        });
    }

    // ── Drain in-flight requests ─────────────────────────────────────
    //
    // Wait for dispatched tasks to finish so their responses reach the
    // MCP host before we tear down stdout. Bounded by a timeout to
    // avoid hanging indefinitely if the server is unresponsive.
    // ─────────────────────────────────────────────────────────────────
    if !in_flight.is_empty() {
        tracing::debug!(
            count = in_flight.len(),
            "draining in-flight MCP bridge tasks"
        );
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while in_flight.join_next().await.is_some() {}
        })
        .await;
    }

    // ── Cleanup: terminate server-side MCP session ───────────────────
    //
    // Send HTTP DELETE to the /mcp endpoint with the session ID so the
    // server's LocalSessionManager can clean up. Without this, each
    // `wsh mcp` invocation leaks a session on the server.
    // ─────────────────────────────────────────────────────────────────
    let sid_guard = session_id.lock().await;
    if let Some(ref sid) = *sid_guard {
        let mut req = http_client
            .delete(&mcp_url)
            .header("Mcp-Session-Id", sid.as_str());
        if let Some(ref t) = token {
            req = req.bearer_auth(t);
        }
        let _ = req.send().await;
        tracing::debug!("sent MCP session cleanup DELETE");
    }
    drop(sid_guard);

    tracing::info!("wsh mcp stdio bridge exiting");
    Ok(())
}

/// Dispatch a single MCP JSON-RPC request to the server and write the
/// response to stdout. Called from a spawned task for concurrency.
async fn mcp_bridge_dispatch(
    body_str: String,
    http_client: reqwest::Client,
    mcp_url: String,
    session_id: Arc<tokio::sync::Mutex<Option<String>>>,
    token: Option<String>,
    stdout: Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
) {
    // Extract the JSON-RPC request ID so we can echo it in error responses
    let request_id = serde_json::from_str::<serde_json::Value>(&body_str)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::Value::Null);

    // Build HTTP request
    let mut req = http_client
        .post(&mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");

    {
        let sid = session_id.lock().await;
        if let Some(ref s) = *sid {
            req = req.header("Mcp-Session-Id", s.as_str());
        }
    }
    if let Some(ref t) = token {
        req = req.bearer_auth(t);
    }

    req = req.body(body_str);

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(?e, "HTTP request to /mcp failed");
            let err_json = serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32603,
                    "message": format!("HTTP request failed: {e}")
                },
                "id": request_id
            });
            let err_line = format!("{}\n", err_json);
            let mut out = stdout.lock().await;
            let _ = tokio::io::AsyncWriteExt::write_all(&mut *out, err_line.as_bytes()).await;
            let _ = tokio::io::AsyncWriteExt::flush(&mut *out).await;

            // ── Stale session recovery ────────────────────────────────
            //
            // If the server restarted, our session ID is stale. On any
            // connection/transport error, clear the session ID so the
            // next request re-initializes.
            // ──────────────────────────────────────────────────────────
            *session_id.lock().await = None;
            return;
        }
    };

    // Capture headers before consuming the body
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Capture mcp-session-id from response headers
    if let Some(sid) = resp.headers().get("mcp-session-id") {
        if let Ok(s) = sid.to_str() {
            *session_id.lock().await = Some(s.to_string());
        }
    }

    let status = resp.status();

    // ── Stale session recovery ────────────────────────────────────────
    //
    // If the server returns 404 or another 4xx with our session ID, the
    // session has expired or the server restarted. Clear the session ID
    // so the next request starts a fresh MCP session.
    // ──────────────────────────────────────────────────────────────────
    if status.as_u16() == 404 || status.as_u16() == 400 {
        let mut sid = session_id.lock().await;
        if sid.is_some() {
            tracing::warn!(status = %status, "server rejected session ID, clearing for re-init");
            *sid = None;
        }
    }

    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() && !status.is_informational() {
        tracing::warn!(status = %status, "MCP endpoint returned error");
        if !body.trim().is_empty() {
            let out_line = format!("{}\n", body.trim());
            let mut out = stdout.lock().await;
            let _ = tokio::io::AsyncWriteExt::write_all(&mut *out, out_line.as_bytes()).await;
            let _ = tokio::io::AsyncWriteExt::flush(&mut *out).await;
        }
        return;
    }

    // Parse SSE response based on content-type only.
    let mut out = stdout.lock().await;
    if content_type.contains("text/event-stream") {
        for event in body.split("\n\n") {
            let event = event.trim();
            if event.is_empty() {
                continue;
            }
            for event_line in event.lines() {
                if let Some(data) = event_line.strip_prefix("data:") {
                    let json_str = data.trim();
                    if !json_str.is_empty() {
                        let out_line = format!("{}\n", json_str);
                        let _ = tokio::io::AsyncWriteExt::write_all(
                            &mut *out,
                            out_line.as_bytes(),
                        )
                        .await;
                    }
                }
            }
        }
    } else {
        let trimmed_body = body.trim();
        if !trimmed_body.is_empty() {
            let out_line = format!("{}\n", trimmed_body);
            let _ = tokio::io::AsyncWriteExt::write_all(&mut *out, out_line.as_bytes()).await;
        }
    }
    let _ = tokio::io::AsyncWriteExt::flush(&mut *out).await;
}

// ── Default mode (no subcommand) ───────────────────────────────────

/// Acquire an advisory file lock to serialize connect-or-spawn sequences.
///
/// Returns a `File` that holds the lock (lock released on drop). Uses
/// `LOCK_EX` (blocking) with a short timeout via `LOCK_NB` + retry to
/// avoid infinite waits.
fn acquire_spawn_lock(lock_path: &std::path::Path) -> Result<std::fs::File, WshError> {
    use std::os::unix::io::AsRawFd;

    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(WshError::Io)?;

    // Try non-blocking first, then retry with short sleeps (up to 5s)
    for _ in 0..50 {
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Final blocking attempt
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if ret != 0 {
        return Err(WshError::Io(std::io::Error::last_os_error()));
    }
    Ok(file)
}

/// Spawn a wsh server daemon as a background process.
///
/// The spawned server runs in ephemeral mode (exits when last session ends).
fn spawn_server_daemon(
    socket_path: &std::path::Path,
    bind: &SocketAddr,
    token: Option<&str>,
    server_name: &str,
) -> Result<(), WshError> {
    let exe = std::env::current_exe().map_err(WshError::Io)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("server")
        .arg("--ephemeral")
        .arg("--bind")
        .arg(bind.to_string())
        .arg("--socket")
        .arg(socket_path)
        .arg("--server-name")
        .arg(server_name);

    if let Some(t) = token {
        cmd.arg("--token").arg(t);
    }

    // Detach from parent: redirect stdio, start new session
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // On Unix, create a new process group so the server survives if the
    // parent exits.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn().map_err(WshError::Io)?;
    tracing::debug!("spawned wsh server daemon");

    // Reap the child in a background thread to prevent zombie accumulation.
    std::thread::spawn(move || {
        let _ = child.wait_with_output();
    });

    Ok(())
}

/// Wait for the Unix socket to become connectable.
async fn wait_for_socket(socket_path: &std::path::Path) -> Result<(), WshError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(WshError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for server socket at {}",
                    socket_path.display()
                ),
            )));
        }
        match client::Client::connect(socket_path).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
}

/// Run the default mode (no subcommand): connect to (or spawn) a server, then attach.
async fn run_default(cli: Cli) -> Result<(), WshError> {
    tracing::info!("wsh starting");

    let server_name = &cli.server_name;
    let socket_path = resolve_socket_path(cli.socket.clone(), server_name);

    // Try connecting to an existing server; if none, spawn one.
    // Uses an advisory file lock to prevent two clients from racing to spawn
    // duplicate daemons (TOCTOU between connect-fail and spawn).
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => {
            tracing::debug!("connected to existing server");
            c
        }
        Err(_) => {
            tracing::debug!("no server running, acquiring spawn lock");
            let lock_path = server::spawn_lock_path_for_instance(server_name);
            let lp = lock_path.clone();
            let _lock = tokio::task::spawn_blocking(move || acquire_spawn_lock(&lp))
                .await
                .map_err(WshError::TaskJoin)??;

            // Re-check after acquiring the lock — another client may have
            // spawned the server while we waited.
            match client::Client::connect(&socket_path).await {
                Ok(c) => {
                    tracing::debug!("connected to server (spawned by another client)");
                    c
                }
                Err(_) => {
                    tracing::debug!("spawning daemon");
                    spawn_server_daemon(&socket_path, &cli.bind, cli.token.as_deref(), server_name)?;
                    wait_for_socket(&socket_path).await?;

                    // If binding to a non-loopback address, retrieve and print the token
                    // so the user knows it before we enter the terminal session.
                    if !is_loopback(&cli.bind) {
                        if let Ok(mut token_client) = client::Client::connect(&socket_path).await {
                            if let Ok(Some(token)) = token_client.get_token().await {
                                eprintln!("wsh: API token: {}", token);
                            }
                        }
                    }

                    client::Client::connect(&socket_path).await.map_err(|e| {
                        eprintln!("wsh: failed to connect to server after spawn: {}", e);
                        WshError::Io(e)
                    })?
                }
            }
        }
    };

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));
    tracing::debug!(rows, cols, "terminal size");

    // Determine what command to pass to the server
    let command = match &cli.cmd {
        Some(cmd) => Some(cmd.clone()),
        None => cli.shell.clone(),
    };

    let msg = protocol::CreateSessionMsg {
        name: cli.name.clone(),
        command,
        cwd: None,
        env: None,
        rows,
        cols,
        tags: cli.tags.clone(),
    };

    let resp = c.create_session(msg).await.map_err(|e| {
        eprintln!("wsh: failed to create session: {}", e);
        WshError::Io(e)
    })?;

    tracing::info!(session = %resp.name, "session created");

    // Enter raw mode for the local terminal
    let raw_guard = terminal::RawModeGuard::new()?;

    // Clear the screen (or enter alternate screen) so the local view
    // starts clean.
    let screen_mode = if cli.alt_screen {
        terminal::ScreenMode::AltScreen
    } else {
        terminal::ScreenMode::Clear
    };
    let screen_guard = terminal::ScreenGuard::new(screen_mode)?;

    // Enter the streaming I/O loop
    let result = c.run_streaming().await;

    // Restore terminal
    drop(screen_guard);
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    eprintln!("[detached from session '{}']", resp.name);
    tracing::info!("wsh exiting");
    Ok(())
}

// ── Client subcommands ─────────────────────────────────────────────

async fn run_attach(
    name: String,
    scrollback: String,
    socket: Option<PathBuf>,
    alt_screen: bool,
    server_name: String,
) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);

    let scrollback_req = match scrollback.as_str() {
        "none" => ScrollbackRequest::None,
        "all" => ScrollbackRequest::All,
        s => match s.parse::<usize>() {
            Ok(n) => ScrollbackRequest::Lines(n),
            Err(_) => {
                eprintln!("wsh attach: invalid scrollback value: {}", s);
                std::process::exit(1);
            }
        },
    };

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));

    let mut c = client::Client::connect(&socket_path).await.map_err(|e| {
        eprintln!("wsh attach: failed to connect to server at {}: {}", socket_path.display(), e);
        WshError::Io(e)
    })?;

    let msg = AttachSessionMsg {
        name: name.clone(),
        scrollback: scrollback_req,
        rows,
        cols,
    };

    let resp = c.attach(msg).await.map_err(|e| {
        eprintln!("wsh attach: {}", e);
        WshError::Io(e)
    })?;

    // Enter raw mode for the local terminal
    let raw_guard = terminal::RawModeGuard::new()?;

    // Clear the screen (or enter alternate screen) so the local view
    // starts clean before replaying scrollback.
    let screen_mode = if alt_screen {
        terminal::ScreenMode::AltScreen
    } else {
        terminal::ScreenMode::Clear
    };
    let screen_guard = terminal::ScreenGuard::new(screen_mode)?;

    // Replay scrollback and screen data before entering the streaming loop
    {
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        if !resp.scrollback.is_empty() {
            let _ = stdout.write_all(&resp.scrollback);
        }
        if !resp.screen.is_empty() {
            let _ = stdout.write_all(&resp.screen);
        }
        let _ = stdout.flush();
    }

    // Enter the streaming I/O loop
    let result = c.run_streaming().await;

    // Restore terminal
    drop(screen_guard);
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh attach: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    eprintln!("[detached from session '{}']", resp.name);
    Ok(())
}

async fn run_list(socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh list: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    let sessions = match c.list_sessions().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("wsh list: {}", e);
            std::process::exit(1);
        }
    };

    if sessions.is_empty() {
        println!("No active sessions.");
    } else {
        println!(
            "{:<20} {:<8} {:<20} {:<12} {:<8} {}",
            "NAME", "PID", "COMMAND", "SIZE", "CLIENTS", "TAGS"
        );
        for s in &sessions {
            let pid_str = match s.pid {
                Some(pid) => pid.to_string(),
                None => "-".to_string(),
            };
            let size = format!("{}x{}", s.cols, s.rows);
            let tags_str = s.tags.join(", ");
            println!(
                "{:<20} {:<8} {:<20} {:<12} {:<8} {}",
                s.name, pid_str, s.command, size, s.clients, tags_str
            );
        }
    }

    Ok(())
}

async fn run_kill(name: String, socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh kill: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = c.kill_session(&name).await {
        eprintln!("wsh kill: {}", e);
        std::process::exit(1);
    }

    println!("Session '{}' killed.", name);
    Ok(())
}

async fn run_detach(name: String, socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh detach: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = c.detach_session(&name).await {
        eprintln!("wsh detach: {}", e);
        std::process::exit(1);
    }

    println!("Session '{}' detached.", name);
    Ok(())
}

async fn run_tag(
    name: String,
    add: Vec<String>,
    remove: Vec<String>,
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh tag: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    match c.manage_tags(&name, add, remove).await {
        Ok(tags) => {
            if tags.is_empty() {
                println!("Session '{}': no tags", name);
            } else {
                println!("Session '{}': {}", name, tags.join(", "));
            }
        }
        Err(e) => {
            eprintln!("wsh tag: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn run_stop(socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            match e.kind() {
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound => {
                    println!("No server running.");
                    return Ok(());
                }
                _ => {
                    eprintln!(
                        "wsh stop: failed to connect to server at {}: {}",
                        socket_path.display(),
                        e
                    );
                    std::process::exit(1);
                }
            }
        }
    };

    if let Err(e) = c.shutdown_server().await {
        eprintln!("wsh stop: {}", e);
        std::process::exit(1);
    }

    // Wait for the socket file to disappear (server cleanup)
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while socket_path.exists() {
        if tokio::time::Instant::now() > deadline {
            eprintln!("wsh stop: server acknowledged shutdown but socket file still exists after 10s");
            std::process::exit(1);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    println!("Server stopped.");
    Ok(())
}

async fn run_token(socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let socket_path = resolve_socket_path(socket, &server_name);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh token: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    match c.get_token().await {
        Ok(Some(token)) => {
            println!("{}", token);
        }
        Ok(None) => {
            eprintln!("wsh token: no auth token configured (server is on localhost)");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("wsh token: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn run_persist(
    value: Option<String>,
    bind: SocketAddr,
    token: Option<String>,
) -> Result<(), WshError> {
    let url = format!("http://{}/server/persist", bind);
    let client = reqwest::Client::new();

    // Determine whether to GET (query) or PUT (set)
    let persistent_value = match value.as_deref() {
        None => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => {
            eprintln!("wsh persist: expected 'on' or 'off', got '{}'", other);
            std::process::exit(1);
        }
    };

    let resp = match persistent_value {
        None => {
            // Query current state
            let mut req = client.get(&url);
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    if e.is_connect() {
                        eprintln!("wsh persist: could not connect to wsh server at {} — is the server running?", bind);
                    } else {
                        eprintln!("wsh persist: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
        Some(val) => {
            // Set new state
            let mut req = client.put(&url).json(&serde_json::json!({"persistent": val}));
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    if e.is_connect() {
                        eprintln!("wsh persist: could not connect to wsh server at {} — is the server running?", bind);
                    } else {
                        eprintln!("wsh persist: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
    };

    if !resp.status().is_success() {
        eprintln!("wsh persist: server returned status {}", resp.status());
        std::process::exit(1);
    }

    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let is_persistent = body["persistent"].as_bool().unwrap_or(false);
    if is_persistent {
        println!("Server is in persistent mode (will stay alive when sessions end).");
    } else {
        println!("Server is in ephemeral mode (will exit when last session ends).");
    }
    Ok(())
}

