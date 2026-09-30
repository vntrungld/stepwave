//! Unix-socket control channel: the daemon serves, CLI subcommands connect.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::{Request, Response};

/// Hard cap on a single request/response line, so a client that never sends a
/// newline (or sends an oversized line) cannot grow our read buffer without bound.
const MAX_LINE: u64 = 64 * 1024;

/// `$XDG_RUNTIME_DIR/stepwave.sock`.
pub fn default_socket_path() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("stepwave.sock"))
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set; pass --socket".to_string())
}

/// Bind the socket. A live daemon on it is an error; a stale file is removed.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another stepwave daemon is running ({})", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    UnixListener::bind(path)
}

/// Serve requests on a background thread: each accepted connection is handled on its
/// own spawned thread (one request line, one response line per connection), so a
/// client that connects and stalls cannot block any other connection. `handler` is
/// called on those per-connection threads.
pub fn serve<F>(listener: UnixListener, handler: F) -> std::thread::JoinHandle<()>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || {
                if let Err(e) = handle_conn(conn, &*handler) {
                    eprintln!("stepwave: control connection: {e}");
                }
            });
        }
    })
}

fn handle_conn<F: Fn(Request) -> Response>(conn: UnixStream, handler: &F) -> io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(conn.try_clone()?.take(MAX_LINE));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response = if line.ends_with('\n') {
        match Request::parse(&line) {
            Ok(req) => handler(req),
            Err(e) => Response::err(e),
        }
    } else {
        Response::err(format!(
            "request line exceeds {MAX_LINE} bytes or is missing a newline"
        ))
    };
    let mut conn = conn;
    conn.write_all(response.to_line().as_bytes())
}

#[derive(Debug)]
pub enum ClientError {
    /// Nothing is listening on the socket.
    NotRunning,
    Io(io::Error),
    BadResponse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => {
                write!(
                    f,
                    "daemon not running; start with: systemctl --user start stepwave"
                )
            }
            ClientError::Io(e) => write!(f, "control socket: {e}"),
            ClientError::BadResponse(e) => write!(f, "bad response from daemon: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Send one request and wait for the response.
pub fn request(path: &Path, req: &Request) -> Result<Response, ClientError> {
    let mut conn = UnixStream::connect(path).map_err(|_| ClientError::NotRunning)?;
    conn.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(ClientError::Io)?;
    let mut line = serde_json::to_string(req).expect("Request always serialises");
    line.push('\n');
    conn.write_all(line.as_bytes()).map_err(ClientError::Io)?;
    let mut reply = String::new();
    BufReader::new(conn.take(MAX_LINE))
        .read_line(&mut reply)
        .map_err(ClientError::Io)?;
    if !reply.ends_with('\n') {
        return Err(ClientError::BadResponse(format!(
            "response exceeds {MAX_LINE} bytes or is missing a newline"
        )));
    }
    serde_json::from_str(reply.trim()).map_err(|e| ClientError::BadResponse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ModeArg, Status};

    fn status() -> Status {
        Status {
            enabled: true,
            mode: ModeArg::Auto,
            strength_db: Some(7.0),
            profile: Some("cs2".into()),
            profile_pinned: false,
            processing: "model".into(),
            fallback_reason: None,
            graph_rate: 48_000,
            routed_streams: vec![],
            io: None,
        }
    }

    #[test]
    fn round_trip_through_a_real_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = bind(&path).unwrap();
        serve(listener, |req| match req {
            Request::Status => Response::ok(status()),
            _ => Response::err("only status"),
        });
        let r = request(&path, &Request::Status).unwrap();
        assert_eq!(r.status.unwrap().processing, "model");
        let r = request(&path, &Request::On).unwrap();
        assert_eq!(r.error.as_deref(), Some("only status"));
    }

    #[test]
    fn invalid_line_gets_an_error_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        serve(bind(&path).unwrap(), |_| Response::ok(status()));
        let mut conn = UnixStream::connect(&path).unwrap();
        conn.write_all(b"{\"cmd\":\"strength\",\"value\":99}\n")
            .unwrap();
        let mut reply = String::new();
        BufReader::new(conn).read_line(&mut reply).unwrap();
        assert!(reply.contains("\"ok\":false"), "{reply}");
    }

    #[test]
    fn not_running_and_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        assert!(matches!(
            request(&path, &Request::Status),
            Err(ClientError::NotRunning)
        ));
        // A stale socket file (listener dropped) is replaced.
        drop(bind(&path).unwrap());
        assert!(path.exists());
        let _l = bind(&path).unwrap();
        // A live one is refused.
        assert_eq!(bind(&path).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn silent_client_does_not_block_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        serve(bind(&path).unwrap(), |_| Response::ok(status()));
        // Connect but never write anything.
        let _silent = UnixStream::connect(&path).unwrap();
        let start = std::time::Instant::now();
        let r = request(&path, &Request::Status).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "took {:?}",
            start.elapsed()
        );
        assert!(r.ok);
    }

    #[test]
    fn oversized_request_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        serve(bind(&path).unwrap(), |_| Response::ok(status()));

        let mut conn = UnixStream::connect(&path).unwrap();
        let mut writer = conn.try_clone().unwrap();
        let writer_thread = std::thread::spawn(move || {
            // No newline: an oversized, unterminated "line".
            let _ = writer.write_all(&vec![b'a'; 64 * 1024 + 4096]);
        });
        let mut reply = String::new();
        let _ = BufReader::new(&mut conn).read_line(&mut reply);
        assert!(reply.contains("\"ok\":false"), "{reply}");
        writer_thread.join().unwrap();

        // The server keeps serving other connections afterwards.
        let r = request(&path, &Request::Status).unwrap();
        assert!(r.ok);
    }
}
