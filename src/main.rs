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

use clap::{CommandFactory, Parser as ClapParser, Subcommand};
use clap_complete::{generate, Shell};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wsh::{
    api,
    server,
    session::SessionRegistry,
    shutdown::ShutdownCoordinator,
    terminal,
    uds_client::UdsHttpClient,
    ws_client,
};

/// wsh - The Web Shell
///
/// A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
/// Run your shell inside wsh to access it from web browsers, agents, and other tools.
#[derive(ClapParser, Debug)]
#[command(name = "wsh", version = wsh::build_version(), about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Command string to execute (like sh -c)
    #[arg(short = 'c')]
    cmd: Option<String>,

    /// Force interactive mode
    #[arg(short = 'i')]
    interactive: bool,

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

    /// Start recording the session immediately after it is created.
    /// The cast file is stored on the server; use `wsh record stop` to finalize.
    #[arg(long)]
    record: bool,

    /// Title to embed in the recording header (implies --record)
    #[arg(long)]
    record_title: Option<String>,

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
        /// Address to bind TCP HTTP/WebSocket listener (optional; UDS always available)
        #[arg(long)]
        bind: Option<SocketAddr>,

        /// Authentication token for non-localhost TCP bindings
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,

        /// Disable authentication even on non-localhost bindings.
        /// WARNING: Anyone with network access can execute commands.
        #[arg(long, env = "WSH_NO_AUTH")]
        no_auth: bool,

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

        /// Base path prefix for all API routes (e.g. "/wsh" for reverse-proxy deployment).
        /// Must start with "/" and must NOT end with "/".
        #[arg(long, env = "WSH_BASE_PREFIX")]
        base_prefix: Option<String>,

        /// Path to TLS certificate file (PEM format). Requires --tls-key.
        #[arg(long, env = "WSH_TLS_CERT", requires = "tls_key")]
        tls_cert: Option<PathBuf>,

        /// Path to TLS private key file (PEM format). Requires --tls-cert.
        #[arg(long, env = "WSH_TLS_KEY", requires = "tls_cert")]
        tls_key: Option<PathBuf>,
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
    List {
        /// Target a specific federated server by hostname
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Kill (destroy) a session on the server
    Kill {
        /// Session name to kill
        name: String,

        /// Target a specific federated server by hostname
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Detach all clients from a session (session stays alive)
    Detach {
        /// Session name to detach
        name: String,

        /// Target a specific federated server by hostname
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Query or set server persistence mode.
    ///
    /// With no argument, prints the current persistence state.
    /// `wsh persist on` — server stays alive when all sessions end.
    /// `wsh persist off` — server exits when the last session ends.
    Persist {
        /// "on" or "off". Omit to query without changing.
        value: Option<String>,
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

        /// Target a specific federated server by hostname
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Stop the running wsh server
    Stop {},

    /// Manage federation hub
    Hub {
        /// Action to perform
        #[command(subcommand)]
        action: HubAction,
    },

    /// Show server information (hostname, version, socket, sessions)
    Info,

    /// Manage session recordings
    ///
    /// Record terminal sessions to asciinema v2 (.cast) files.
    /// Recordings persist after the session ends and can be played in any browser.
    ///
    /// Examples:
    ///   wsh record start build --title "CI Build"
    ///   wsh record stop build
    ///   wsh record list
    ///   wsh record download <id> --output build.cast
    Record {
        #[command(subcommand)]
        action: RecordAction,
    },

    /// Start an MCP server over stdio (for AI hosts like Claude Desktop)
    Mcp {},

    /// Generate shell completions for bash, zsh, or fish
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand, Debug)]
enum HubAction {
    /// List all servers (local + federated backends)
    List,

    /// Add a remote backend server
    Add {
        /// URL of the backend (e.g., "http://10.0.1.10:8080" or "https://proxy.example.com/wsh")
        address: String,

        /// Authentication token for the backend
        #[arg(long)]
        token: Option<String>,
    },

    /// Remove a remote backend server by hostname
    Remove {
        /// Hostname of the backend to remove
        hostname: String,
    },

    /// Reload federation config from file
    Reload,
}

#[derive(Subcommand, Debug)]
enum RecordAction {
    /// Start recording a session
    Start {
        /// Session name to record
        name: String,

        /// Title to embed in the asciinema header
        #[arg(long, short)]
        title: Option<String>,
    },

    /// Stop the active recording for a session
    Stop {
        /// Session name whose recording to stop
        name: String,
    },

    /// Show the active recording status for a session
    Status {
        /// Session name to check
        name: String,
    },

    /// List recordings
    ///
    /// With no options, lists all recordings on the server.
    List {
        /// Filter by session name
        #[arg(long, short)]
        session: Option<String>,

        /// Filter by status: recording, stopped, failed
        #[arg(long)]
        status: Option<String>,
    },

    /// Show details for a single recording
    Get {
        /// Recording ID
        id: String,
    },

    /// Delete a recording and its cast file
    Delete {
        /// Recording ID
        id: String,
    },

    /// Download a recording's cast file
    ///
    /// Writes the raw asciinema v2 cast file to a local path or stdout.
    /// Works for both active (partial) and completed recordings.
    ///
    /// Examples:
    ///   wsh record download <id> --output build.cast
    ///   wsh record download <id> | asciinema play /dev/stdin
    Download {
        /// Recording ID
        id: String,

        /// Output file path. Use "-" for stdout.
        #[arg(long, short, default_value = "-")]
        output: String,
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

/// Resolve the base socket path from explicit `--socket` or `-L` server name.
///
/// `--socket` takes priority; if absent, derives from the server name.
/// Used as the `--socket` argument when spawning server daemons; the server
/// derives the HTTP socket path from this base path.
fn resolve_socket_path(socket: Option<PathBuf>, server_name: &str) -> PathBuf {
    socket.unwrap_or_else(|| server::socket_path_for_instance(server_name))
}

/// Resolve the HTTP-over-UDS socket path from explicit `--socket` or `-L` server name.
///
/// When `--socket` is provided, the HTTP socket is derived by changing the extension
/// to `.http.sock`. Otherwise, uses the named-instance convention.
fn resolve_http_socket_path(socket: Option<PathBuf>, server_name: &str) -> PathBuf {
    socket
        .map(|p| p.with_extension("http.sock"))
        .unwrap_or_else(|| server::http_socket_path_for_instance(server_name))
}

/// Minimum token length for non-localhost bindings.  Tokens shorter than this
/// are rejected to prevent accidental auth bypass (e.g. `WSH_TOKEN=""`).
const MIN_TOKEN_LENGTH: usize = 16;

fn resolve_token(bind: &SocketAddr, user_token: &Option<String>, no_auth: bool) -> Result<Option<String>, WshError> {
    if is_loopback(bind) {
        return Ok(None);
    }
    if no_auth {
        tracing::warn!(
            "Authentication disabled (--no-auth). \
             Anyone with network access to this server can execute arbitrary commands."
        );
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
    let is_mcp = matches!(cli.command, Some(Commands::Mcp {}));
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
        Some(Commands::Server { bind, token, no_auth, ephemeral, max_sessions, cors_origins, rate_limit, config, hostname, base_prefix, tls_cert, tls_key }) => {
            run_server(bind, token, no_auth, socket, ephemeral, max_sessions, server_name, cors_origins, rate_limit, config, hostname, base_prefix, tls_cert, tls_key).await
        }
        Some(Commands::Attach { name, scrollback, alt_screen }) => {
            run_attach(name, scrollback, socket, alt_screen, server_name).await
        }
        Some(Commands::List { server }) => {
            run_list(socket, server_name, server).await
        }
        Some(Commands::Kill { name, server }) => {
            run_kill(name, socket, server_name, server).await
        }
        Some(Commands::Detach { name, server }) => {
            run_detach(name, socket, server_name, server).await
        }
        Some(Commands::Token {}) => {
            run_token(socket, server_name).await
        }
        Some(Commands::Persist { value }) => {
            run_persist(value, socket.clone(), server_name.clone()).await
        }
        Some(Commands::Tag { name, add, remove, server }) => {
            run_tag(name, add, remove, server, socket, server_name).await
        }
        Some(Commands::Stop {}) => {
            run_stop(socket, server_name).await
        }
        Some(Commands::Hub { action }) => {
            run_hub(action, socket, server_name).await
        }
        Some(Commands::Info) => {
            run_info(socket, server_name).await
        }
        Some(Commands::Record { action }) => {
            run_record(action, socket, server_name).await
        }
        Some(Commands::Mcp {}) => {
            run_mcp(socket, server_name).await
        }
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "wsh", &mut std::io::stdout());
            Ok(())
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
    bind: Option<SocketAddr>,
    token: Option<String>,
    no_auth: bool,
    socket: Option<PathBuf>,
    ephemeral: bool,
    max_sessions: Option<usize>,
    server_name: String,
    cors_origins: Vec<String>,
    rate_limit: Option<u32>,
    config_arg: Option<PathBuf>,
    hostname_arg: Option<String>,
    base_prefix: Option<String>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
) -> Result<(), WshError> {
    tracing::info!(instance = %server_name, "wsh server starting");

    // Validate base_prefix format.
    if let Some(ref prefix) = base_prefix {
        if !prefix.starts_with('/') {
            return Err(WshError::Config("--base-prefix must start with '/'".into()));
        }
        if prefix.ends_with('/') && prefix.len() > 1 {
            return Err(WshError::Config("--base-prefix must not end with '/'".into()));
        }
    }

    // Load TLS configuration if cert + key are provided and TCP is active.
    let tls_acceptor = match (&bind, tls_cert, tls_key) {
        (Some(_addr), Some(cert), Some(key)) => {
            let acceptor = wsh::tls::load_tls_config(&cert, &key)
                .map_err(|e| WshError::Config(e.to_string()))?;
            tracing::info!(cert = %cert.display(), key = %key.display(), "TLS configured");
            Some(acceptor)
        }
        (Some(addr), _, _) => {
            if !is_loopback(addr) {
                tracing::warn!(
                    "Binding to non-loopback address {} without TLS. \
                     Bearer tokens and terminal data will be transmitted in cleartext. \
                     Consider using --tls-cert and --tls-key, or a TLS-terminating reverse proxy.",
                    addr
                );
            }
            None
        }
        _ => None,
    };

    // Token and rate limiting are only relevant for TCP listeners.
    let token = match &bind {
        Some(addr) => {
            let t = resolve_token(addr, &token, no_auth)?;
            if t.is_some() {
                tracing::info!("auth token configured");
            }
            t
        }
        None => None,
    };

    let rate_limit = match (&bind, rate_limit) {
        (_, Some(rps)) => Some(rps),
        (Some(addr), None) if !is_loopback(addr) => {
            tracing::info!("applying default rate limit (100 req/s per IP) for non-localhost binding");
            Some(100)
        }
        _ => None,
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

    // Build IP access control from config (if configured).
    let is_remote_bind = bind.as_ref().map_or(false, |a| !is_loopback(a));
    let ip_access_control = fed_config.ip_access.as_ref().map(|cfg| {
        let ctrl = wsh::federation::ip_access::IpAccessControl::from_config(cfg);
        if ctrl.is_unconfigured() {
            tracing::debug!("IP access control config present but empty");
        } else {
            tracing::info!("IP access control configured");
        }
        if is_remote_bind && ctrl.is_unconfigured() {
            tracing::warn!(
                "Binding to non-loopback address without IP access control. \
                 Consider configuring [ip_access] blocklist/allowlist in the config file."
            );
        }
        Arc::new(ctrl)
    });

    // Warn if non-loopback with no ip_access config at all.
    if is_remote_bind && ip_access_control.is_none() {
        tracing::warn!(
            "Binding to non-loopback address without IP access control. \
             Consider adding [ip_access] to your federation config file."
        );
    }

    // Generate a unique server identity for federation loop prevention.
    let server_id = uuid::Uuid::new_v4().to_string();

    // Create the FederationManager: spawns persistent WebSocket connections
    // for each configured backend server.
    let federation_manager = Arc::new(tokio::sync::Mutex::new(
        wsh::federation::manager::FederationManager::from_config(
            fed_config,
            token.clone(),
            fed_default_token.clone(),
            server_id.clone(),
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
    let shutdown_request = tokio_util::sync::CancellationToken::new();
    let state = api::AppState {
        sessions: sessions.clone(),
        shutdown: shutdown.clone(),
        server_config: server_config.clone(),
        server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: Arc::new(api::ticket::TicketStore::new()),
        backends: federation_manager.lock().await.registry().clone(),
        federation: federation_manager.clone(),
        ip_access: ip_access_control,
        hostname,
        federation_config_path: if config_path.exists() { Some(config_path) } else { None },
        local_token: token.clone(),
        default_backend_token: fed_default_token,
        server_id: server_id.clone(),
        shutdown_notify: shutdown_request.clone(),
        tcp_addr: bind,
        instance_name: server_name.clone(),
        http_socket_path: socket.as_ref()
            .map(|p| p.with_extension("http.sock"))
            .unwrap_or_else(|| server::http_socket_path_for_instance(&server_name)),
        recordings: wsh::recording::RecordingRegistry::new(),
    };

    if !cors_origins.is_empty() {
        tracing::info!(origins = ?cors_origins, "CORS origins configured");
    }
    if let Some(rps) = rate_limit {
        tracing::info!(rps, "rate limiting configured");
    }

    if let Some(ref prefix) = base_prefix {
        tracing::info!(prefix = %prefix, "base path prefix configured");
    }
    let router_bind = bind.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap());
    let uds_state = state.clone();
    let app = api::router(state, api::RouterConfig { token: token.clone(), bind: router_bind, cors_origins, rate_limit, base_prefix: base_prefix.clone() });

    // Acquire instance lock (flock) before binding any sockets.
    // The lock file is held for the server's lifetime and released on exit.
    let lock_path = server::lock_path_for_instance(&server_name);
    let _instance_lock = server::acquire_instance_lock(&lock_path)
        .map_err(WshError::Io)?;

    // Cancellation token for HTTP server shutdown (supports multiple listeners)
    let http_cancel = tokio_util::sync::CancellationToken::new();

    // ── HTTP-over-UDS listener (always active) ─────────────────────
    let http_socket_path = socket.as_ref()
        .map(|p| p.with_extension("http.sock"))
        .unwrap_or_else(|| server::http_socket_path_for_instance(&server_name));

    // Remove stale socket file. The instance lock guarantees we own this name.
    if http_socket_path.exists() {
        std::fs::remove_file(&http_socket_path).map_err(WshError::Io)?;
    }
    if let Some(parent) = http_socket_path.parent() {
        std::fs::create_dir_all(parent).map_err(WshError::Io)?;
    }

    let uds_listener = tokio::net::UnixListener::bind(&http_socket_path)
        .map_err(WshError::Io)?;

    // Restrict socket permissions to owner only (0600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&http_socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(WshError::Io)?;
    }

    let uds_app = api::core_router(uds_state)
        .layer(axum::middleware::from_fn(api::transport::uds_transport_middleware));

    let uds_cancel = http_cancel.clone();
    let http_socket_path_for_cleanup = http_socket_path.clone();
    let uds_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(
            uds_listener,
            uds_app.into_make_service_with_connect_info::<api::transport::UdsConnectInfo>(),
        )
            .with_graceful_shutdown(uds_cancel.cancelled_owned())
            .await
        {
            tracing::error!(?e, "HTTP-over-UDS server error");
        }
    });

    tracing::info!(path = %http_socket_path.display(), "HTTP API available via Unix socket");

    // ── TCP listener (opt-in via --bind) ───────────────────────────
    let mut tcp_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let _actual_tcp_addr: Option<SocketAddr>;

    if let Some(bind_addr) = bind {
        let tcp_app = app.clone()
            .layer(axum::middleware::from_fn(api::transport::tcp_transport_middleware));

        let listener = tokio::net::TcpListener::bind(bind_addr)
            .await
            .map_err(WshError::Io)?;
        let actual_addr = listener.local_addr().map_err(WshError::Io)?;
        _actual_tcp_addr = Some(actual_addr);
        let scheme = if tls_acceptor.is_some() { "HTTPS/WSS" } else { "HTTP/WS" };
        tracing::info!(addr = %actual_addr, scheme, "TCP server listening");

        // When binding to IPv4 loopback, also listen on IPv6 loopback.
        let ipv6_listener = if bind_addr.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
            let v6_addr = std::net::SocketAddr::new(
                std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                actual_addr.port(),
            );
            match tokio::net::TcpListener::bind(v6_addr).await {
                Ok(l) => {
                    tracing::info!(addr = %v6_addr, scheme, "TCP server listening (IPv6 loopback)");
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

        if let Some(acceptor) = tls_acceptor {
            let cancel4 = http_cancel.clone();
            let app4 = tcp_app.clone();
            let acceptor4 = acceptor.clone();
            tcp_handles.push(tokio::spawn(serve_tls(listener, acceptor4, app4, cancel4)));

            if let Some(l) = ipv6_listener {
                let cancel6 = http_cancel.clone();
                let app6 = tcp_app.clone();
                let acceptor6 = acceptor.clone();
                tcp_handles.push(tokio::spawn(serve_tls(l, acceptor6, app6, cancel6)));
            }
        } else {
            let cancel4 = http_cancel.clone();
            let tcp_app_v6 = tcp_app.clone();
            tcp_handles.push(tokio::spawn(async move {
                if let Err(e) = axum::serve(
                    listener,
                    tcp_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                    .with_graceful_shutdown(cancel4.cancelled_owned())
                    .await
                {
                    tracing::error!(?e, "TCP HTTP server error");
                }
            }));

            if let Some(l) = ipv6_listener {
                let cancel6 = http_cancel.clone();
                tcp_handles.push(tokio::spawn(async move {
                    if let Err(e) = axum::serve(
                        l,
                        tcp_app_v6.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                    )
                        .with_graceful_shutdown(cancel6.cancelled_owned())
                        .await
                    {
                        tracing::error!(?e, "TCP HTTP server error (IPv6)");
                    }
                }));
            }
        }
    } else {
        _actual_tcp_addr = None;
        tracing::info!("HTTP API available via Unix socket only (use --bind for TCP)");
    };

    tracing::info!("wsh server ready");

    // Ephemeral shutdown monitor: when no interest remains (no sessions AND
    // no persistent connections), shut down the server automatically.
    let ephemeral_handle = tokio::spawn(ephemeral_monitor(
        server_config.clone(),
        sessions.clone(),
        shutdown.clone(),
    ));

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

    // Remove socket file immediately. Once the listener is cancelled it
    // will never accept again, so the file is just a stale marker.
    if http_socket_path_for_cleanup.exists() {
        let _ = std::fs::remove_file(&http_socket_path_for_cleanup);
        tracing::debug!(path = %http_socket_path_for_cleanup.display(), "removed HTTP socket file");
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
            if let Err(e) = uds_handle.await {
                tracing::warn!(?e, "HTTP-over-UDS server task panicked");
            }
            for h in tcp_handles {
                if let Err(e) = h.await {
                    tracing::warn!(?e, "TCP HTTP server task panicked");
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

/// Interest-aware ephemeral shutdown monitor.
///
/// Returns `true` when the server should shut down (no interest remaining in
/// non-persistent mode), or `false` if the event channel closed.
///
/// "Interest" means at least one of:
/// - Terminal sessions exist (`!sessions.is_empty()`)
/// - Persistent connections are active (`shutdown.active_count() > 0`),
///   e.g., WebSocket clients streaming a session, web UI, `/mcp/ws` bridge
///
/// HTTP MCP sessions (Streamable HTTP `/mcp`) are deliberately excluded from
/// interest: they are stateless and cannot reliably signal client departure.
async fn ephemeral_monitor(
    config: Arc<api::ServerConfig>,
    sessions: SessionRegistry,
    shutdown: ShutdownCoordinator,
) -> bool {
    let mut events = sessions.subscribe_events();

    /// Check if anything is keeping the server alive.
    fn has_interest(sessions: &SessionRegistry, shutdown: &ShutdownCoordinator) -> bool {
        !sessions.is_empty() || shutdown.active_count() > 0
    }

    // ── Phase 1: idle timeout ─────────────────────────────────────
    //
    // In non-persistent mode, give clients 30 seconds to establish
    // interest (create a session or open a persistent connection).
    // If nothing happens, the daemon was likely orphaned.
    if !config.is_persistent() {
        let idle_timeout = tokio::time::sleep(std::time::Duration::from_secs(30));
        tokio::pin!(idle_timeout);

        loop {
            // TOCTOU-safe: register notified BEFORE checking state
            let notified = shutdown.interest_changed();
            if has_interest(&sessions, &shutdown) {
                break; // interest established, enter normal monitoring
            }
            tokio::select! {
                result = events.recv() => {
                    match result {
                        Ok(_) => break, // event received, enter normal monitoring
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                    }
                }
                _ = notified => {
                    // Interest may have changed, re-check in next iteration
                    continue;
                }
                _ = &mut idle_timeout => {
                    if !has_interest(&sessions, &shutdown) {
                        tracing::info!("no interest within idle timeout, ephemeral server shutting down");
                        return true;
                    }
                    break; // interest appeared, enter normal monitoring
                }
            }
        }
    }

    // ── Phase 2: normal monitoring ────────────────────────────────
    //
    // Wait for all interest to drain (sessions empty AND active_count == 0).
    loop {
        let notified = shutdown.interest_changed();

        // Quick check: if persistent mode was toggled on, never shut down
        if config.is_persistent() {
            // Wait for events indefinitely (persistent mode)
            match events.recv().await {
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }

        if !has_interest(&sessions, &shutdown) {
            tracing::info!("no interest remaining, ephemeral server shutting down");
            return true;
        }

        tokio::select! {
            result = events.recv() => {
                match result {
                    Ok(event) => {
                        let is_removal = matches!(
                            event,
                            wsh::session::SessionEvent::Destroyed { .. }
                        );
                        if is_removal && !has_interest(&sessions, &shutdown) {
                            tracing::info!("last session ended, ephemeral server shutting down");
                            return true;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "ephemeral monitor lagged on session events");
                        if !has_interest(&sessions, &shutdown) {
                            // Grace period: rapid session churn may leave
                            // registry momentarily empty between destroy
                            // and the next create.
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            if !has_interest(&sessions, &shutdown) {
                                tracing::info!("no interest remaining (detected after lag), ephemeral server shutting down");
                                return true;
                            }
                            tracing::debug!("interest appeared during lag grace period, continuing");
                        }
                    }
                }
            }
            _ = notified => {
                // active_count changed, re-check in next iteration
            }
        }
    }
}

/// Manual TLS accept loop for HTTPS serving.
///
/// `axum::serve()` only accepts `TcpListener` (sealed `Listener` trait), so
/// TLS requires a manual loop: accept TCP → TLS handshake → hyper-util
/// `serve_connection`. Each connection is handled in its own spawned task.
async fn serve_tls(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    cancel: tokio_util::sync::CancellationToken,
) {
    use hyper_util::rt::TokioIo;

    loop {
        let (tcp_stream, peer_addr) = tokio::select! {
            _ = cancel.cancelled() => break,
            result = listener.accept() => {
                match result {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::debug!(?e, "TCP accept error");
                        continue;
                    }
                }
            }
        };

        let acceptor = acceptor.clone();
        let app = app.clone();
        let cancel = cancel.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(tcp_stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(?e, %peer_addr, "TLS handshake failed");
                    return;
                }
            };

            let io = TokioIo::new(tls_stream);
            let service = hyper_util::service::TowerToHyperService::new(app);
            let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
            let conn = builder.serve_connection_with_upgrades(io, service);
            tokio::pin!(conn);

            tokio::select! {
                result = &mut conn => {
                    if let Err(e) = result {
                        tracing::debug!(?e, %peer_addr, "connection error");
                    }
                }
                _ = cancel.cancelled() => {
                    // Graceful shutdown: signal the connection and give it time to finish
                    conn.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), conn).await;
                }
            }
        });
    }
}

// ── MCP stdio mode ─────────────────────────────────────────────────

/// Run the MCP stdio bridge: connect to (or spawn) a server, then bridge
/// stdin/stdout JSON-RPC ↔ the server's `/mcp/ws` WebSocket endpoint via UDS.
///
/// The WebSocket connection registers a `ConnectionGuard` on the server,
/// providing reliable interest signaling for ephemeral shutdown. No manual
/// DELETE cleanup is needed — the server detects disconnection automatically.
async fn run_mcp(
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    tracing::info!("wsh mcp stdio bridge starting");

    let socket_path = resolve_socket_path(socket.clone(), &server_name);
    let http_socket_path = resolve_http_socket_path(socket.clone(), &server_name);

    // Connect to existing server or spawn one (with file lock to prevent races)
    let http_client = UdsHttpClient::new(&http_socket_path);
    if !http_client.health_check().await {
        tracing::debug!("no server running, acquiring spawn lock");
        let lock_path = server::spawn_lock_path_for_instance(&server_name);
        let lp = lock_path.clone();
        let _lock = tokio::task::spawn_blocking(move || acquire_spawn_lock(&lp))
            .await
            .map_err(WshError::TaskJoin)??;

        // Re-check after acquiring the lock
        if !http_client.health_check().await {
            tracing::debug!("spawning daemon");
            spawn_server_daemon(&socket_path, &server_name)?;
            wait_for_http_socket(&http_socket_path).await?;
        } else {
            tracing::debug!("connected to server (spawned by another client)");
        }
    } else {
        tracing::debug!("connected to existing server");
    }

    // Connect WebSocket-over-UDS to /mcp/ws eagerly. This registers a
    // ConnectionGuard on the server immediately, preventing the ephemeral
    // idle timeout from firing while the MCP host is slow to send `initialize`.
    let stream = tokio::net::UnixStream::connect(&http_socket_path)
        .await
        .map_err(WshError::Io)?;
    let (mut ws, _response) =
        tokio_tungstenite::client_async("ws://localhost/mcp/ws", stream)
            .await
            .map_err(|e| WshError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e)))?;
    tracing::debug!("connected to /mcp/ws");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();

    let mut line = String::new();
    loop {
        line.clear();
        tokio::select! {
            // stdin → WS
            result = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line) => {
                match result {
                    Ok(0) => {
                        // EOF on stdin — MCP host closed
                        tracing::debug!("stdin EOF");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            if ws.send(Message::Text(trimmed.to_string().into())).await.is_err() {
                                tracing::debug!("WS send failed");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(?e, "stdin read error");
                        break;
                    }
                }
            }
            // WS → stdout
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        use tokio::io::AsyncWriteExt;
                        let text_ref: &str = &text;
                        let _ = stdout.write_all(text_ref.as_bytes()).await;
                        if !text_ref.ends_with('\n') {
                            let _ = stdout.write_all(b"\n").await;
                        }
                        let _ = stdout.flush().await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::debug!("WS close received");
                        break;
                    }
                    Some(Ok(_)) => {} // ignore binary, ping, pong
                    Some(Err(e)) => {
                        tracing::error!(?e, "WS recv error");
                        break;
                    }
                    None => {
                        tracing::debug!("WS stream ended");
                        break;
                    }
                }
            }
        }
    }

    // Close WebSocket gracefully
    let _ = ws.close(None).await;
    tracing::info!("wsh mcp stdio bridge exiting");
    Ok(())
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
/// Auto-spawned servers are UDS-only (no TCP listener).
fn spawn_server_daemon(
    socket_path: &std::path::Path,
    server_name: &str,
) -> Result<(), WshError> {
    let exe = std::env::current_exe().map_err(WshError::Io)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("server")
        .arg("--ephemeral")
        .arg("--socket")
        .arg(socket_path)
        .arg("--server-name")
        .arg(server_name);

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

/// Wait for the HTTP-over-UDS socket to become connectable and healthy.
async fn wait_for_http_socket(http_socket_path: &std::path::Path) -> Result<(), WshError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let client = UdsHttpClient::new(http_socket_path);
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(WshError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for HTTP socket at {}",
                    http_socket_path.display()
                ),
            )));
        }
        if client.health_check().await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Run the default mode (no subcommand): connect to (or spawn) a server, then attach.
async fn run_default(cli: Cli) -> Result<(), WshError> {
    tracing::info!("wsh starting");

    let server_name = &cli.server_name;
    let socket_path = resolve_socket_path(cli.socket.clone(), server_name);
    let http_socket_path = resolve_http_socket_path(cli.socket.clone(), server_name);

    // Try connecting to an existing server via HTTP health check; if none, spawn one.
    // Uses an advisory file lock to prevent two clients from racing to spawn
    // duplicate daemons (TOCTOU between connect-fail and spawn).
    let http_client = UdsHttpClient::new(&http_socket_path);
    if !http_client.health_check().await {
        tracing::debug!("no server running, acquiring spawn lock");
        let lock_path = server::spawn_lock_path_for_instance(server_name);
        let lp = lock_path.clone();
        let _lock = tokio::task::spawn_blocking(move || acquire_spawn_lock(&lp))
            .await
            .map_err(WshError::TaskJoin)??;

        // Re-check after acquiring the lock — another client may have
        // spawned the server while we waited.
        if !http_client.health_check().await {
            tracing::debug!("spawning daemon");
            spawn_server_daemon(&socket_path, server_name)?;
            wait_for_http_socket(&http_socket_path).await?;
        } else {
            tracing::debug!("connected to server (spawned by another client)");
        }
    } else {
        tracing::debug!("connected to existing server");
    }

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));
    tracing::debug!(rows, cols, "terminal size");

    // Determine what command to pass to the server
    let command = match &cli.cmd {
        Some(cmd) => Some(cmd.clone()),
        None => cli.shell.clone(),
    };

    // Build optional recording config from --record / --record-title flags.
    let recording_opts = if cli.record || cli.record_title.is_some() {
        Some(serde_json::json!({ "title": cli.record_title }))
    } else {
        None
    };

    // Create session via REST API
    let create_body = serde_json::json!({
        "name": cli.name,
        "command": command,
        "rows": rows,
        "cols": cols,
        "tags": cli.tags,
        "recording": recording_opts,
    });

    let resp = http_client.post_json("/sessions", &create_body).await.map_err(|e| {
        eprintln!("wsh: failed to create session: {}", e);
        WshError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
    })?;

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh: failed to create session: {}", body);
        return Err(WshError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("session creation failed: {}", body),
        )));
    }

    let session_info: serde_json::Value = resp.json().await.map_err(|e| {
        WshError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let session_name = session_info["name"]
        .as_str()
        .ok_or_else(|| WshError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing session name in response",
        )))?
        .to_string();

    tracing::info!(session = %session_name, "session created");

    // If auto-recording was requested, print the recording ID so the user can
    // reference it after the session ends.
    if let Some(rec_id) = session_info["recording_id"].as_str() {
        eprintln!("wsh: recording started ({})", rec_id);
    }

    // Connect WebSocket for streaming I/O
    let ws = ws_client::connect_ws_uds(&http_socket_path, &session_name).await.map_err(|e| {
        eprintln!("wsh: failed to connect WebSocket: {}", e);
        WshError::Io(e)
    })?;

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

    // Enter the WebSocket streaming I/O loop (no initial resize for new sessions)
    let result = ws_client::run_ws_streaming(ws, None).await;

    // Restore terminal
    drop(screen_guard);
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    eprintln!("[detached from session '{}']", session_name);
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
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let http_client = UdsHttpClient::new(&http_socket_path);

    let scrollback_limit: Option<usize> = match scrollback.as_str() {
        "none" => None,
        "all" => Some(usize::MAX),
        s => match s.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("wsh attach: invalid scrollback value: {}", s);
                std::process::exit(1);
            }
        },
    };

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));

    // Verify session exists via REST
    let resp = http_client.get(&format!("/sessions/{}", name)).await.map_err(|e| {
        eprintln!("wsh attach: failed to connect to server: {}", e);
        WshError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
    })?;

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh attach: session '{}' not found: {}", name, body);
        return Err(WshError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("session '{}' not found", name),
        )));
    }

    // Fetch scrollback and screen data for replay using the parser's
    // line_to_ansi to convert styled lines to raw ANSI sequences.
    use wsh::parser::ansi::line_to_ansi;
    use wsh::parser::state::FormattedLine;

    let mut scrollback_bytes: Vec<u8> = Vec::new();
    if let Some(limit) = scrollback_limit {
        let limit = limit.min(10_000);
        let path = format!(
            "/sessions/{}/scrollback?format=styled&offset=0&limit={}",
            name, limit
        );
        if let Ok(sb_resp) = http_client.get(&path).await {
            if sb_resp.status.is_success() {
                if let Ok(body) = sb_resp.text().await {
                    if let Ok(sb) = serde_json::from_str::<serde_json::Value>(&body) {
                        if let Some(lines) = sb.get("lines").and_then(|l| l.as_array()) {
                            let mut buf = String::new();
                            for line_val in lines {
                                if let Ok(line) = serde_json::from_value::<FormattedLine>(line_val.clone()) {
                                    buf.push_str(&line_to_ansi(&line));
                                    buf.push_str("\r\n");
                                }
                            }
                            scrollback_bytes = buf.into_bytes();
                        }
                    }
                }
            }
        }
    }

    let mut screen_bytes: Vec<u8> = Vec::new();
    {
        let path = format!("/sessions/{}/screen?format=styled", name);
        if let Ok(scr_resp) = http_client.get(&path).await {
            if scr_resp.status.is_success() {
                if let Ok(body) = scr_resp.text().await {
                    if let Ok(scr) = serde_json::from_str::<serde_json::Value>(&body) {
                        // The screen endpoint returns EnrichedScreen with flattened
                        // QueryResponse::Screen fields (lines, cursor, etc.)
                        // alongside last_activity_ms.
                        if let Some(lines) = scr.get("lines").and_then(|l| l.as_array()) {
                            let mut buf = String::new();
                            // Clear screen and home cursor before replaying
                            buf.push_str("\x1b[H\x1b[2J");
                            for (i, line_val) in lines.iter().enumerate() {
                                if let Ok(line) = serde_json::from_value::<FormattedLine>(line_val.clone()) {
                                    buf.push_str(&line_to_ansi(&line));
                                    if i + 1 < lines.len() {
                                        buf.push_str("\r\n");
                                    }
                                }
                            }
                            // Restore cursor position
                            if let Some(cursor) = scr.get("cursor") {
                                let row = cursor.get("row").and_then(|r| r.as_u64()).unwrap_or(0) + 1;
                                let col = cursor.get("col").and_then(|c| c.as_u64()).unwrap_or(0) + 1;
                                buf.push_str(&format!("\x1b[{};{}H", row, col));
                            }
                            screen_bytes = buf.into_bytes();
                        }
                    }
                }
            }
        }
    }

    // Connect WebSocket for streaming I/O
    let ws = ws_client::connect_ws_uds(&http_socket_path, &name).await.map_err(|e| {
        eprintln!("wsh attach: failed to connect WebSocket: {}", e);
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
        if !scrollback_bytes.is_empty() {
            let _ = stdout.write_all(&scrollback_bytes);
        }
        if !screen_bytes.is_empty() {
            let _ = stdout.write_all(&screen_bytes);
        }
        let _ = stdout.flush();
    }

    // Enter the WebSocket streaming I/O loop with initial resize
    let result = ws_client::run_ws_streaming(ws, Some((rows, cols))).await;

    // Restore terminal
    drop(screen_guard);
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh attach: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    eprintln!("[detached from session '{}']", name);
    Ok(())
}

async fn run_list(socket: Option<PathBuf>, server_name: String, server: Option<String>) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let path = match server.as_deref() {
        Some(s) => format!("/sessions?server={}", s),
        None => "/sessions".to_string(),
    };

    let resp = match client.get(&path).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "wsh list: could not connect to wsh server — is the server running? ({})",
                e
            );
            std::process::exit(1);
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh list: {}", body);
        std::process::exit(1);
    }

    let sessions: Vec<serde_json::Value> = match resp.json().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("wsh list: failed to parse response: {}", e);
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
            let name = s["name"].as_str().unwrap_or("-");
            let pid_str = match s["pid"].as_u64() {
                Some(pid) => pid.to_string(),
                None => "-".to_string(),
            };
            let command = s["command"].as_str().unwrap_or("-");
            let cols = s["cols"].as_u64().unwrap_or(0);
            let rows = s["rows"].as_u64().unwrap_or(0);
            let size = format!("{}x{}", cols, rows);
            let clients = s["clients"].as_u64().unwrap_or(0);
            let tags: Vec<&str> = s["tags"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let tags_str = tags.join(", ");
            println!(
                "{:<20} {:<8} {:<20} {:<12} {:<8} {}",
                name, pid_str, command, size, clients, tags_str
            );
        }
    }

    Ok(())
}

async fn run_kill(name: String, socket: Option<PathBuf>, server_name: String, server: Option<String>) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let path = match server.as_deref() {
        Some(s) => format!("/sessions/{}?server={}", name, s),
        None => format!("/sessions/{}", name),
    };

    let resp = match client.delete(&path).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "wsh kill: could not connect to wsh server — is the server running? ({})",
                e
            );
            std::process::exit(1);
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh kill: {}", body);
        std::process::exit(1);
    }

    println!("Session '{}' killed.", name);
    Ok(())
}

async fn run_detach(name: String, socket: Option<PathBuf>, server_name: String, server: Option<String>) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let path = match server.as_deref() {
        Some(s) => format!("/sessions/{}/detach?server={}", name, s),
        None => format!("/sessions/{}/detach", name),
    };

    let resp = match client.post_json(&path, &serde_json::json!({})).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "wsh detach: could not connect to wsh server — is the server running? ({})",
                e
            );
            std::process::exit(1);
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh detach: {}", body);
        std::process::exit(1);
    }

    println!("Session '{}' detached.", name);
    Ok(())
}

async fn run_tag(
    name: String,
    add: Vec<String>,
    remove: Vec<String>,
    server: Option<String>,
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let path = match server.as_deref() {
        Some(s) => format!("/sessions/{}?server={}", name, s),
        None => format!("/sessions/{}", name),
    };

    let body = serde_json::json!({
        "add_tags": add,
        "remove_tags": remove,
    });

    let resp = match client.patch_json(&path, &body).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "wsh tag: could not connect to wsh server — is the server running? ({})",
                e
            );
            std::process::exit(1);
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh tag: {}", body);
        std::process::exit(1);
    }

    let session_info: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("wsh tag: failed to parse response: {}", e);
            std::process::exit(1);
        }
    };

    let tags: Vec<&str> = session_info["tags"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if tags.is_empty() {
        println!("Session '{}': no tags", name);
    } else {
        println!("Session '{}': {}", name, tags.join(", "));
    }

    Ok(())
}

async fn run_hub(
    action: HubAction,
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    match action {
        HubAction::List => {
            let resp = match client.get("/servers").await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh hub list: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh hub list: {}", body);
                std::process::exit(1);
            }

            let servers: Vec<serde_json::Value> = match resp.json() .await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("wsh hub list: failed to parse response: {}", e);
                    std::process::exit(1);
                }
            };

            if servers.is_empty() {
                println!("No servers.");
            } else {
                println!(
                    "{:<20} {:<25} {:<12} {:<10} {}",
                    "HOSTNAME", "ADDRESS", "HEALTH", "ROLE", "SESSIONS"
                );
                for s in &servers {
                    let hostname = s["hostname"].as_str().unwrap_or("-");
                    let address = s["address"].as_str().unwrap_or("-");
                    let health = s["health"].as_str().unwrap_or("-");
                    let role = s["role"].as_str().unwrap_or("-");
                    let sessions_str = match s["sessions"].as_u64() {
                        Some(n) => n.to_string(),
                        None => "-".to_string(),
                    };
                    println!(
                        "{:<20} {:<25} {:<12} {:<10} {}",
                        hostname, address, health, role, sessions_str
                    );
                }
            }
        }
        HubAction::Add { address, token } => {
            let body = serde_json::json!({
                "address": address,
                "token": token,
            });

            let resp = match client.post_json("/servers", &body).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh hub add: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh hub add: {}", body);
                std::process::exit(1);
            }

            let result: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("wsh hub add: failed to parse response: {}", e);
                    std::process::exit(1);
                }
            };

            println!(
                "Server added: {} (health: {})",
                result["address"].as_str().unwrap_or(&address),
                result["health"].as_str().unwrap_or("unknown")
            );
        }
        HubAction::Remove { hostname } => {
            let resp = match client.delete(&format!("/servers/{}", hostname)).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh hub remove: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh hub remove: {}", body);
                std::process::exit(1);
            }

            println!("Server '{}' removed.", hostname);
        }
        HubAction::Reload => {
            let resp = match client.post_json("/server/reload-config", &serde_json::json!({})).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh hub reload: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh hub reload: {}", body);
                std::process::exit(1);
            }

            let result: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("wsh hub reload: failed to parse response: {}", e);
                    std::process::exit(1);
                }
            };

            println!(
                "Config reloaded: {} added, {} removed.",
                result["added"].as_u64().unwrap_or(0),
                result["removed"].as_u64().unwrap_or(0)
            );
        }
    }

    Ok(())
}

async fn run_record(
    action: RecordAction,
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    match action {
        RecordAction::Start { name, title } => {
            let body = serde_json::json!({ "title": title });
            let resp = match client
                .post_json(&format!("/sessions/{}/recording", name), &body)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record start: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record start: {}", body);
                std::process::exit(1);
            }
            let info: serde_json::Value = resp.json().await.unwrap_or_default();
            print_recording_info(&info, "started");
        }

        RecordAction::Stop { name } => {
            let resp = match client
                .delete(&format!("/sessions/{}/recording", name))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record stop: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record stop: {}", body);
                std::process::exit(1);
            }
            let info: serde_json::Value = resp.json().await.unwrap_or_default();
            print_recording_info(&info, "stopped");
        }

        RecordAction::Status { name } => {
            let resp = match client
                .get(&format!("/sessions/{}/recording", name))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record status: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if resp.status == hyper::StatusCode::NOT_FOUND {
                println!("No active recording for session '{}'.", name);
                return Ok(());
            }
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record status: {}", body);
                std::process::exit(1);
            }
            let info: serde_json::Value = resp.json().await.unwrap_or_default();
            print_recording_info(&info, "active");
        }

        RecordAction::List { session, status } => {
            let mut path = "/recordings".to_string();
            let mut params: Vec<String> = Vec::new();
            if let Some(ref s) = session {
                params.push(format!("session={}", s));
            }
            if let Some(ref st) = status {
                params.push(format!("status={}", st));
            }
            if !params.is_empty() {
                path.push('?');
                path.push_str(&params.join("&"));
            }
            let resp = match client.get(&path).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record list: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record list: {}", body);
                std::process::exit(1);
            }
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let recordings = body["recordings"].as_array().cloned().unwrap_or_default();
            if recordings.is_empty() {
                println!("No recordings.");
            } else {
                println!(
                    "{:<38} {:<20} {:<12} {:<10} {}",
                    "ID", "SESSION", "STATUS", "DURATION", "TITLE"
                );
                for r in &recordings {
                    let id = &r["id"].as_str().unwrap_or("-")[..36.min(r["id"].as_str().unwrap_or("-").len())];
                    let sess = r["session"].as_str().unwrap_or("-");
                    let status = r["status"].as_str().unwrap_or("-");
                    let duration = match r["duration_secs"].as_f64() {
                        Some(d) => format_duration(d),
                        None => "-".to_string(),
                    };
                    let title = r["title"].as_str().unwrap_or("");
                    println!(
                        "{:<38} {:<20} {:<12} {:<10} {}",
                        id, sess, status, duration, title
                    );
                }
            }
        }

        RecordAction::Get { id } => {
            let resp = match client.get(&format!("/recordings/{}", id)).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record get: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record get: {}", body);
                std::process::exit(1);
            }
            let info: serde_json::Value = resp.json().await.unwrap_or_default();
            print_recording_info(&info, "");
        }

        RecordAction::Delete { id } => {
            let resp = match client.delete(&format!("/recordings/{}", id)).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record delete: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record delete: {}", body);
                std::process::exit(1);
            }
            println!("Recording {} deleted.", id);
        }

        RecordAction::Download { id, output } => {
            let resp = match client.get(&format!("/recordings/{}/cast", id)).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh record download: could not connect to wsh server — is it running? ({})", e);
                    std::process::exit(1);
                }
            };
            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh record download: {}", body);
                std::process::exit(1);
            }
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("wsh record download: failed to read response: {}", e);
                    std::process::exit(1);
                }
            };
            if output == "-" {
                use std::io::Write;
                std::io::stdout().write_all(&bytes).map_err(WshError::Io)?;
            } else {
                tokio::fs::write(&output, &bytes).await.map_err(WshError::Io)?;
                eprintln!("wsh: saved {} bytes to {}", bytes.len(), output);
            }
        }
    }

    Ok(())
}

/// Format elapsed seconds as a human-readable duration string (e.g. "1h 23m 45s").
fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

/// Print a recording info block to stdout.
///
/// `verb` is an optional past-tense word printed in the first line
/// (e.g. "started", "stopped"). Pass "" to omit it.
fn print_recording_info(info: &serde_json::Value, verb: &str) {
    let id = info["id"].as_str().unwrap_or("-");
    let session = info["session"].as_str().unwrap_or("-");
    let status = info["status"].as_str().unwrap_or("-");
    let title = info["title"].as_str();
    let bytes = info["bytes_written"].as_u64().unwrap_or(0);
    let duration = info["duration_secs"]
        .as_f64()
        .map(format_duration)
        .unwrap_or_else(|| "-".to_string());
    let cast_url = info["urls"]["cast"].as_str().unwrap_or("");
    let player_url = info["urls"]["player"].as_str().unwrap_or("");
    let embed_url = info["urls"]["embed"].as_str().unwrap_or("");

    if verb.is_empty() {
        println!("Recording {}", id);
    } else {
        println!("Recording {} {}", id, verb);
    }
    println!("  Session:  {}", session);
    println!("  Status:   {}", status);
    if let Some(t) = title {
        println!("  Title:    {}", t);
    }
    println!("  Duration: {}", duration);
    println!("  Size:     {} bytes", bytes);
    if !cast_url.is_empty() {
        println!("  Cast:     {}", cast_url);
    }
    if !player_url.is_empty() {
        println!("  Player:   {}", player_url);
    }
    if !embed_url.is_empty() {
        println!("  Embed:    {}", embed_url);
    }
}

async fn run_info(socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let resp = match client.get("/server/info").await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "wsh info: could not connect to wsh server — is the server running? ({})",
                e
            );
            std::process::exit(1);
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh info: {}", body);
        std::process::exit(1);
    }

    let info: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("wsh info: failed to parse response: {}", e);
            std::process::exit(1);
        }
    };

    println!("Server:     {}", info["instance_name"].as_str().unwrap_or("unknown"));
    println!("Hostname:   {}", info["hostname"].as_str().unwrap_or("unknown"));
    println!("Version:    {}", info["version"].as_str().unwrap_or("unknown"));
    println!("Socket:     {}", info["socket_path"].as_str().unwrap_or("unknown"));
    if let Some(addr) = info["tcp_addr"].as_str() {
        println!("TCP:        {}", addr);
    }
    println!("Persistent: {}", if info["persistent"].as_bool().unwrap_or(false) { "yes" } else { "no" });
    println!("Sessions:   {}", info["session_count"].as_u64().unwrap_or(0));
    Ok(())
}

async fn run_stop(socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let resp = match client.post_json("/server/shutdown", &serde_json::json!({})).await {
        Ok(r) => r,
        Err(_) => {
            // If we can't connect, the server is likely not running
            println!("No server running.");
            return Ok(());
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh stop: {}", body);
        std::process::exit(1);
    }

    // Wait for the HTTP socket file to disappear (server cleanup)
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while http_socket_path.exists() {
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
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let resp = match client.get("/server/token").await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "wsh token: could not connect to wsh server — is the server running? ({})",
                e
            );
            std::process::exit(1);
        }
    };

    if !resp.status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("wsh token: {}", body);
        std::process::exit(1);
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("wsh token: failed to parse response: {}", e);
            std::process::exit(1);
        }
    };

    match body["token"].as_str() {
        Some(token) => {
            println!("{}", token);
        }
        None => {
            eprintln!("wsh token: no auth token configured (server is on localhost)");
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn run_persist(
    value: Option<String>,
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    let http_socket_path = socket.as_ref()
        .map(|p| p.with_extension("http.sock"))
        .unwrap_or_else(|| server::http_socket_path_for_instance(&server_name));

    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

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

    let (status, body) = match persistent_value {
        None => {
            // Query current state
            match client.get("/server/persist").await {
                Ok(resp) => {
                    let status = resp.status;
                    let body = resp.text().await.unwrap_or_default();
                    (status, body)
                }
                Err(e) => {
                    eprintln!("wsh persist: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            }
        }
        Some(val) => {
            // Set new state
            let body_json = serde_json::json!({"persistent": val});
            match client.put_json("/server/persist", &body_json).await {
                Ok(resp) => {
                    let status = resp.status;
                    let body = resp.text().await.unwrap_or_default();
                    (status, body)
                }
                Err(e) => {
                    eprintln!("wsh persist: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            }
        }
    };

    if !status.is_success() {
        eprintln!("wsh persist: server returned status {}", status);
        std::process::exit(1);
    }

    let body: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let is_persistent = body["persistent"].as_bool().unwrap_or(false);
    if is_persistent {
        println!("Server is in persistent mode (will stay alive when sessions end).");
    } else {
        println!("Server is in ephemeral mode (will exit when last session ends).");
    }
    Ok(())
}

