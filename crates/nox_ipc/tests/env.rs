//! Environment-derived paths. Environment variables are process-global, so everything that
//! sets them lives in this one test (its own test binary, hence its own process).

use std::path::{Path, PathBuf};

use nox_ipc::{Client, ENV_NOX_HOME, ENV_SOCKET, ENV_TOKEN, default_socket_path, nox_home};

fn set(key: &str, value: &str) {
    // SAFETY: this binary has a single test, so no other thread reads the environment.
    unsafe { std::env::set_var(key, value) };
}

#[test]
fn paths_and_token_from_environment() {
    // NOX_HOME wins over HOME.
    set(ENV_NOX_HOME, "/scratch/nox-home");
    assert_eq!(nox_home(), Path::new("/scratch/nox-home"));
    assert_eq!(default_socket_path(), Path::new("/scratch/nox-home/run/nox.sock"));

    // Empty NOX_HOME falls back to ~/.nox; empty HOME to /.nox.
    set(ENV_NOX_HOME, "");
    set("HOME", "/Users/someone");
    assert_eq!(nox_home(), Path::new("/Users/someone/.nox"));
    set("HOME", "");
    assert_eq!(nox_home(), Path::new("/.nox"));

    // Socket paths too long for sun_path move to /tmp/nox-<uid>.
    let long = format!("/{}", "d".repeat(120));
    set(ENV_NOX_HOME, &long);
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    assert_eq!(default_socket_path(), PathBuf::from(format!("/tmp/nox-{uid}/nox.sock")));

    // NOX_SOCKET overrides the default path; empty means unset.
    set(ENV_NOX_HOME, "/scratch/h");
    set(ENV_SOCKET, "/scratch/s.sock");
    assert_eq!(Client::socket_path_from_env(), Path::new("/scratch/s.sock"));
    set(ENV_SOCKET, "");
    assert_eq!(Client::socket_path_from_env(), Path::new("/scratch/h/run/nox.sock"));

    set(ENV_TOKEN, "");
    assert_eq!(Client::token_from_env(), None);
    set(ENV_TOKEN, "abc");
    assert_eq!(Client::token_from_env().as_deref(), Some("abc"));

    // Nothing listens on the socket: connecting fails.
    let dir = tempfile::tempdir().unwrap();
    set(ENV_SOCKET, dir.path().join("none.sock").to_str().unwrap());
    assert!(Client::connect_from_env().is_err());
}
