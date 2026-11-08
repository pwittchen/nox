//! Blocking client of the control socket, used by the `nox` CLI and `nox mcp`.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::{ErrorCode, IpcError, Method, PROTOCOL_VERSION, Request, Response};
use crate::{ENV_SOCKET, ENV_TOKEN, default_socket_path};

/// A connection to a running nox.
#[derive(Debug)]
pub struct Client {
    path: PathBuf,
    token: Option<String>,
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    /// Bytes of a partially read line (kept across timeouts).
    pending: Vec<u8>,
    next_id: u64,
}

impl Client {
    /// The socket path from `NOX_SOCKET`, else [`default_socket_path`].
    #[must_use]
    pub fn socket_path_from_env() -> PathBuf {
        match std::env::var_os(ENV_SOCKET) {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => default_socket_path(),
        }
    }

    /// The token from `NOX_TOKEN`, if set and not empty.
    #[must_use]
    pub fn token_from_env() -> Option<String> {
        std::env::var(ENV_TOKEN).ok().filter(|t| !t.is_empty())
    }

    /// Connect using `NOX_SOCKET` (or the default path) and `NOX_TOKEN` (optional).
    pub fn connect_from_env() -> io::Result<Client> {
        Self::connect(&Self::socket_path_from_env(), Self::token_from_env())
    }

    /// Connect to `path`, authenticating requests with `token`.
    pub fn connect(path: &Path, token: Option<String>) -> io::Result<Client> {
        let stream = UnixStream::connect(path)?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Client {
            path: path.to_path_buf(),
            token,
            writer: stream,
            reader,
            pending: Vec::new(),
            next_id: 1,
        })
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Send one request and wait for its response (responses to other ids are skipped).
    /// With `timeout`, fails with [`ErrorCode::Timeout`] when no answer arrives in time.
    pub fn call(&mut self, method: Method, timeout: Option<Duration>) -> Result<Value, IpcError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request { v: PROTOCOL_VERSION, id, token: self.token.clone(), method };
        let mut line = serde_json::to_string(&request)
            .map_err(|e| IpcError::new(ErrorCode::Internal, format!("serialize request: {e}")))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|e| io_error("send request", &e))?;

        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let read_timeout = match deadline {
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(timeout_error());
                    }
                    Some(left)
                }
                None => None,
            };
            // On macOS `setsockopt` fails with `EINVAL` once the peer closed the socket. Reads
            // can't block then (they return buffered data or EOF), so that's not an error: the
            // answer may already be buffered behind a line for another id.
            if let Err(e) = self.reader.get_ref().set_read_timeout(read_timeout)
                && e.raw_os_error() != Some(libc::EINVAL)
            {
                return Err(io_error("set timeout", &e));
            }
            match self.reader.read_until(b'\n', &mut self.pending) {
                Ok(0) => {
                    return Err(IpcError::new(ErrorCode::Internal, "nox closed the connection"));
                }
                Ok(_) if self.pending.last() != Some(&b'\n') => {
                    return Err(IpcError::new(ErrorCode::Internal, "nox closed the connection"));
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    return Err(timeout_error());
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_error("read response", &e)),
            }
            let line = std::mem::take(&mut self.pending);
            let response: Response = match serde_json::from_slice(&line) {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("nox_ipc client: unparsable response line: {e}");
                    continue;
                }
            };
            if response.id == id {
                return response.into_result();
            }
            // id 0 = the server could not even parse our request.
            if response.id == 0
                && let Err(e) = response.into_result()
            {
                return Err(e);
            }
        }
    }

    /// [`Client::call`] and deserialize the result.
    pub fn call_typed<T: serde::de::DeserializeOwned>(
        &mut self,
        method: Method,
        timeout: Option<Duration>,
    ) -> Result<T, IpcError> {
        let v = self.call(method, timeout)?;
        serde_json::from_value(v)
            .map_err(|e| IpcError::new(ErrorCode::Internal, format!("unexpected result: {e}")))
    }
}

fn timeout_error() -> IpcError {
    IpcError::new(ErrorCode::Timeout, "timed out waiting for nox")
}

fn io_error(what: &str, e: &io::Error) -> IpcError {
    IpcError::new(ErrorCode::Internal, format!("{what}: {e}"))
}
