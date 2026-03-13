//! Server instance management: socket paths, lock files, and related helpers.
//!
//! This module provides path resolution and locking utilities for wsh server
//! instances. Each named instance gets a set of files in the instance directory:
//! - `<name>.http.sock` — HTTP/WS/MCP API over Unix domain socket
//! - `<name>.lock` — flock-based mutual exclusion for the server process
//! - `<name>.spawn.lock` — coordination lock for client auto-spawn races

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Acquire an exclusive flock on the server instance lock file.
///
/// Returns the `File` handle that holds the lock. The lock is released when
/// the file is dropped (or the process exits, even on crash). Returns
/// `AddrInUse` if another server already holds the lock.
pub fn acquire_instance_lock(lock_path: &Path) -> io::Result<File> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;

    use std::os::unix::io::AsRawFd;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        let errno = io::Error::last_os_error();
        if errno.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another server is already listening for this instance",
            ));
        }
        return Err(errno);
    }

    Ok(file)
}

/// Base directory for all wsh instance files (sockets, locks).
///
/// Returns `$XDG_RUNTIME_DIR/wsh/` or `/tmp/wsh-$USER/wsh/` as fallback.
pub fn instance_dir() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/wsh-{}", whoami()));
    PathBuf::from(runtime_dir).join("wsh")
}

/// Compute the Unix socket path for a named server instance (legacy binary protocol).
///
/// Returns `<instance_dir>/<name>.sock`.
///
/// Note: The binary protocol socket is no longer created by the server.
/// This helper is retained for client-side compatibility during the transition
/// period (e.g., `wsh stop` waiting for socket file cleanup).
pub fn socket_path_for_instance(name: &str) -> PathBuf {
    instance_dir().join(format!("{}.sock", name))
}

/// Compute the HTTP Unix socket path for a named server instance.
///
/// Returns `<instance_dir>/<name>.http.sock`. This socket serves the full
/// HTTP/WS/MCP API over UDS and is the primary transport for local access.
pub fn http_socket_path_for_instance(name: &str) -> PathBuf {
    instance_dir().join(format!("{}.http.sock", name))
}

/// Compute the server lock path for a named server instance.
///
/// The server holds an exclusive flock on this file for its entire lifetime,
/// providing reliable mutual exclusion without the races of connect-probing.
pub fn lock_path_for_instance(name: &str) -> PathBuf {
    instance_dir().join(format!("{}.lock", name))
}

/// Compute the client spawn-coordination lock path for a named instance.
///
/// Clients acquire this lock (blocking) when racing to auto-spawn a server,
/// preventing duplicate daemons. Separate from the server lock to avoid
/// deadlock (client takes spawn lock, then server takes server lock).
pub fn spawn_lock_path_for_instance(name: &str) -> PathBuf {
    instance_dir().join(format!("{}.spawn.lock", name))
}

/// Compute the default Unix socket path for this user.
///
/// Equivalent to `socket_path_for_instance("default")`.
pub fn default_socket_path() -> PathBuf {
    socket_path_for_instance("default")
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_socket_path() {
        let path = default_socket_path();
        let s = path.to_str().unwrap();
        // New layout: .../wsh/default.sock
        assert!(s.ends_with("wsh/default.sock"), "expected path ending with wsh/default.sock, got {}", s);
    }

    #[test]
    fn test_socket_path_for_instance() {
        let path = socket_path_for_instance("staging");
        let s = path.to_str().unwrap();
        assert!(s.ends_with("wsh/staging.sock"), "got {}", s);
    }

    #[test]
    fn test_http_socket_path_for_instance() {
        let path = http_socket_path_for_instance("staging");
        let s = path.to_str().unwrap();
        assert!(s.ends_with("wsh/staging.http.sock"), "got {}", s);
    }

    #[test]
    fn test_lock_path_for_instance() {
        let path = lock_path_for_instance("staging");
        let s = path.to_str().unwrap();
        assert!(s.ends_with("wsh/staging.lock"), "got {}", s);
    }

    #[test]
    fn test_spawn_lock_path_for_instance() {
        let path = spawn_lock_path_for_instance("staging");
        let s = path.to_str().unwrap();
        assert!(s.ends_with("wsh/staging.spawn.lock"), "got {}", s);
    }

    #[test]
    fn test_instance_paths_share_parent() {
        let sock = socket_path_for_instance("foo");
        let lock = lock_path_for_instance("foo");
        let spawn = spawn_lock_path_for_instance("foo");
        assert_eq!(sock.parent(), lock.parent());
        assert_eq!(lock.parent(), spawn.parent());
    }
}
