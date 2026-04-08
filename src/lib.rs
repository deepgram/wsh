pub fn build_version() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("WSH_GIT_DESCRIBE"),
        " ",
        env!("WSH_BUILD_PROFILE"),
        ")",
    )
}

pub mod activity;
pub mod recording;
pub mod config;
pub mod api;
pub mod federation;
pub mod broker;
pub mod client;
pub mod input;
pub mod mcp;
pub mod overlay;
pub mod panel;
pub mod parser;
pub mod pty;
pub mod server;
pub mod session;
pub mod shutdown;
pub mod terminal;
pub mod tls;
pub mod uds_client;
pub mod ws_client;
