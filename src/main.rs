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

    /// Manage federated backend servers
    Servers {
        /// Action to perform
        #[command(subcommand)]
        action: ServersAction,
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
enum ServersAction {
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

    /// Show server info (hostname and version)
    Info,

    /// Reload federation config from file
    Reload,
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
        Some(Commands::Servers { action }) => {
            run_servers(action, socket, server_name).await
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
    };

    if !cors_origins.is_empty() {
        tracing::info!(origins = ?cors_origins, "CORS origins configured");
    }
    if let Some(rps) = rate_limit {
        tracing::info!(rps, "rate limiting configured");
    }

    let socket_token = token.clone();
    let socket_hostname = state.hostname.clone();
    let socket_fed_state = server::FederationState {
        federation: state.federation.clone(),
        backends: state.backends.clone(),
        config_path: state.federation_config_path.clone(),
        local_token: state.local_token.clone(),
        default_backend_token: state.default_backend_token.clone(),
        ip_access: state.ip_access.clone(),
        server_id: state.server_id.clone(),
    };
    if let Some(ref prefix) = base_prefix {
        tracing::info!(prefix = %prefix, "base path prefix configured");
    }
    let router_bind = bind.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap());
    let app = api::router(state, api::RouterConfig { token: token.clone(), bind: router_bind, cors_origins, rate_limit, base_prefix: base_prefix.clone() });

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

    let uds_app = app.clone()
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
    let shutdown_request_clone = shutdown_request.clone();
    let socket_handle = tokio::spawn(async move {
        if let Err(e) = server::serve(socket_sessions, &socket_path, socket_cancel_clone, socket_token, shutdown_request_clone, socket_hostname, socket_fed_state).await {
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

    // Remove socket files immediately. Once listeners are cancelled they
    // will never accept again, so the files are just stale markers.
    if socket_path_for_cleanup.exists() {
        let _ = std::fs::remove_file(&socket_path_for_cleanup);
        tracing::debug!(path = %socket_path_for_cleanup.display(), "removed binary socket file");
    }
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
            if let Err(e) = socket_handle.await {
                tracing::warn!(?e, "binary socket server task panicked");
            }
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
/// stdin/stdout JSON-RPC ↔ the server's `/mcp` Streamable HTTP endpoint via UDS.
async fn run_mcp(
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    tracing::info!("wsh mcp stdio bridge starting");

    let socket_path = resolve_socket_path(socket.clone(), &server_name);

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
                    spawn_server_daemon(&socket_path, &server_name)?;
                    wait_for_socket(&socket_path).await?;
                }
            }
        }
    }

    // Resolve the HTTP UDS socket path for the MCP endpoint
    let http_socket_path = socket.as_ref()
        .map(|p| p.with_extension("http.sock"))
        .unwrap_or_else(|| server::http_socket_path_for_instance(&server_name));

    // Wait for the HTTP UDS socket to appear
    {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if http_socket_path.exists() {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                return Err(WshError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("timed out waiting for HTTP socket at {}", http_socket_path.display()),
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    let uds_http = Arc::new(wsh::uds_client::UdsHttpClient::new(&http_socket_path));
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
        let client = uds_http.clone();
        let sid = session_id.clone();
        let out = stdout.clone();
        let sem = concurrency.clone();

        in_flight.spawn(async move {
            // Acquire permit before dispatching; dropped when the task completes.
            let _permit = sem.acquire().await;
            mcp_bridge_dispatch_uds(body_str, client, sid, out).await;
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
        let _ = uds_http.delete_with_headers(
            "/mcp",
            &[("Mcp-Session-Id", sid.as_str())],
        ).await;
        tracing::debug!("sent MCP session cleanup DELETE");
    }
    drop(sid_guard);

    tracing::info!("wsh mcp stdio bridge exiting");
    Ok(())
}

/// Dispatch a single MCP JSON-RPC request to the server over UDS and write
/// the response to stdout. Called from a spawned task for concurrency.
async fn mcp_bridge_dispatch_uds(
    body_str: String,
    client: Arc<wsh::uds_client::UdsHttpClient>,
    session_id: Arc<tokio::sync::Mutex<Option<String>>>,
    stdout: Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
) {
    // Extract the JSON-RPC request ID so we can echo it in error responses
    let request_id = serde_json::from_str::<serde_json::Value>(&body_str)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::Value::Null);

    // Build extra headers
    let sid_val;
    let mut headers = vec![
        ("Accept", "application/json, text/event-stream"),
    ];
    {
        let sid = session_id.lock().await;
        if let Some(ref s) = *sid {
            sid_val = s.clone();
        } else {
            sid_val = String::new();
        }
    }
    if !sid_val.is_empty() {
        headers.push(("Mcp-Session-Id", &sid_val));
    }

    let resp = match client.post_raw(
        "/mcp",
        "application/json",
        bytes::Bytes::from(body_str),
        &headers,
    ).await {
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
            *session_id.lock().await = None;
            return;
        }
    };

    // Capture headers before consuming the body
    let content_type = resp.headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Capture mcp-session-id from response headers
    if let Some(sid) = resp.headers.get("mcp-session-id") {
        if let Ok(s) = sid.to_str() {
            *session_id.lock().await = Some(s.to_string());
        }
    }

    let status = resp.status;

    // Stale session recovery
    if status == hyper::StatusCode::NOT_FOUND || status == hyper::StatusCode::BAD_REQUEST {
        let mut sid = session_id.lock().await;
        if sid.is_some() {
            tracing::warn!(status = %status, "server rejected session ID, clearing for re-init");
            *sid = None;
        }
    }

    let body = resp.text().await.unwrap_or_else(|_| String::new());

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
                    spawn_server_daemon(&socket_path, server_name)?;
                    wait_for_socket(&socket_path).await?;

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
        server: None,
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

async fn run_servers(
    action: ServersAction,
    socket: Option<PathBuf>,
    server_name: String,
) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket, &server_name);
    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    match action {
        ServersAction::List => {
            let resp = match client.get("/servers").await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh servers list: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh servers list: {}", body);
                std::process::exit(1);
            }

            let servers: Vec<serde_json::Value> = match resp.json() .await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("wsh servers list: failed to parse response: {}", e);
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
        ServersAction::Add { address, token } => {
            let body = serde_json::json!({
                "address": address,
                "token": token,
            });

            let resp = match client.post_json("/servers", &body).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh servers add: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh servers add: {}", body);
                std::process::exit(1);
            }

            let result: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("wsh servers add: failed to parse response: {}", e);
                    std::process::exit(1);
                }
            };

            println!(
                "Server added: {} (health: {})",
                result["address"].as_str().unwrap_or(&address),
                result["health"].as_str().unwrap_or("unknown")
            );
        }
        ServersAction::Remove { hostname } => {
            let resp = match client.delete(&format!("/servers/{}", hostname)).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh servers remove: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh servers remove: {}", body);
                std::process::exit(1);
            }

            println!("Server '{}' removed.", hostname);
        }
        ServersAction::Info => {
            let resp = match client.get("/server/info").await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh servers info: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh servers info: {}", body);
                std::process::exit(1);
            }

            let info: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("wsh servers info: failed to parse response: {}", e);
                    std::process::exit(1);
                }
            };

            println!("Hostname: {}", info["hostname"].as_str().unwrap_or("-"));
            println!("Version:  {}", info["version"].as_str().unwrap_or("-"));
        }
        ServersAction::Reload => {
            let resp = match client.post_json("/server/reload-config", &serde_json::json!({})).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("wsh servers reload: could not connect to wsh server — is the server running? ({})", e);
                    std::process::exit(1);
                }
            };

            if !resp.status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                eprintln!("wsh servers reload: {}", body);
                std::process::exit(1);
            }

            let result: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("wsh servers reload: failed to parse response: {}", e);
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

async fn run_stop(socket: Option<PathBuf>, server_name: String) -> Result<(), WshError> {
    let http_socket_path = resolve_http_socket_path(socket.clone(), &server_name);
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

    // Wait for the socket file to disappear (server cleanup)
    let socket_path = resolve_socket_path(socket, &server_name);
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

