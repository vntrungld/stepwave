//! Control channel over a local socket: a named pipe (`\\.\pipe\<name>`) on Windows, an
//! abstract-namespace Unix socket elsewhere (which is what the Linux tests exercise). The
//! protocol is M5's: one JSON request line and one response line per connection.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, Listener, ListenerOptions, Name, Stream,
};
use stepwave_host::protocol::{Request, Response};

/// Pipe name used by `stepwave.exe`.
pub const PIPE_NAME: &str = "stepwave";
/// Longest accepted request or response line, in bytes.
const MAX_LINE: u64 = 64 * 1024;

fn name(pipe: &str) -> io::Result<Name<'_>> {
    pipe.to_ns_name::<GenericNamespaced>()
}

/// Create the listener. If a live instance already answers on `pipe`, fail with `AddrInUse`.
pub fn bind(pipe: &str) -> io::Result<Listener> {
    if Stream::connect(name(pipe)?).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another stepwave instance is running (pipe '{pipe}')"),
        ));
    }
    ListenerOptions::new().name(name(pipe)?).create_sync()
}

/// Serve on a background thread, one thread per connection so a silent client cannot block
/// others. `handler` runs on the connection threads.
pub fn serve<F>(listener: Listener, handler: F) -> std::thread::JoinHandle<()>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else {
                // E.g. all pipe instances busy: back off instead of spinning a core.
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            let handler = handler.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle_conn(conn, &*handler) {
                    eprintln!("stepwave: control connection: {e}");
                }
            });
        }
    })
}

fn handle_conn<F: Fn(Request) -> Response>(conn: Stream, handler: &F) -> io::Result<()> {
    // Best effort: Windows named pipes do not support I/O timeouts. A silent client can then
    // only hold its own connection thread, never other requests.
    let _ = conn.set_recv_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(conn.take(MAX_LINE));
    let mut line = String::new();
    let bytes_read = reader.read_line(&mut line)?;
    if is_silent_probe(bytes_read) {
        // The peer (e.g. bind()'s live-instance check) connected and disconnected without
        // writing anything. Nothing to reply to, and no error: this is expected traffic.
        return Ok(());
    }
    let response = if !line.ends_with('\n') {
        Response::err(format!(
            "request longer than {MAX_LINE} bytes or not newline-terminated"
        ))
    } else {
        match Request::parse(&line) {
            Ok(req) => handler(req),
            Err(e) => Response::err(e),
        }
    };
    let mut conn = reader.into_inner().into_inner();
    conn.write_all(response.to_line().as_bytes())
}

/// A connection that reached EOF on its very first read (0 bytes) connected and disconnected
/// without writing anything, e.g. `bind()`'s live-instance probe. It gets a quiet close rather
/// than the "not newline-terminated" error reply.
fn is_silent_probe(bytes_read: usize) -> bool {
    bytes_read == 0
}

#[derive(Debug)]
pub enum ClientError {
    /// Nothing is listening on the pipe.
    NotRunning,
    Io(io::Error),
    BadResponse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => write!(
                f,
                "stepwave is not running; start it with `stepwave run` (or log in again after `stepwave install`)"
            ),
            ClientError::Io(e) => write!(f, "control pipe: {e}"),
            ClientError::BadResponse(e) => write!(f, "bad response from stepwave: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Send one request and wait for the response.
pub fn request(pipe: &str, req: &Request) -> Result<Response, ClientError> {
    let name = name(pipe).map_err(ClientError::Io)?;
    let mut conn = Stream::connect(name).map_err(|_| ClientError::NotRunning)?;
    // Best effort, as on the server side (unsupported on Windows named pipes).
    let _ = conn.set_recv_timeout(Some(Duration::from_secs(10)));
    let mut line = serde_json::to_string(req).expect("Request always serialises");
    line.push('\n');
    conn.write_all(line.as_bytes()).map_err(ClientError::Io)?;
    let mut reply = String::new();
    BufReader::new(conn.take(MAX_LINE))
        .read_line(&mut reply)
        .map_err(ClientError::Io)?;
    if !reply.ends_with('\n') {
        return Err(ClientError::BadResponse("truncated reply".into()));
    }
    serde_json::from_str(reply.trim()).map_err(|e| ClientError::BadResponse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use stepwave_host::protocol::{ModeArg, Status};

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

    /// A pipe name unique to this test process and test.
    fn unique(test: &str) -> String {
        format!("stepwave-test-{}-{test}", std::process::id())
    }

    #[test]
    fn round_trip() {
        let pipe = unique("rt");
        serve(bind(&pipe).unwrap(), |req| match req {
            Request::Status => Response::ok(status()),
            _ => Response::err("only status"),
        });
        let r = request(&pipe, &Request::Status).unwrap();
        assert_eq!(r.status.unwrap().processing, "model");
        let r = request(&pipe, &Request::On).unwrap();
        assert_eq!(r.error.as_deref(), Some("only status"));
    }

    #[test]
    fn is_silent_probe_is_only_true_on_empty_read() {
        assert!(is_silent_probe(0));
        assert!(!is_silent_probe(1));
    }

    #[test]
    fn silent_probe_connection_is_ignored_and_does_not_block_others() {
        let pipe = unique("probe");
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        // Connect and drop without writing, like bind()'s live-instance probe.
        drop(Stream::connect(name(&pipe).unwrap()).unwrap());
        // A normal request still succeeds afterward.
        assert!(request(&pipe, &Request::Status).unwrap().ok);
    }

    #[test]
    fn not_running_and_second_instance() {
        let pipe = unique("nr");
        assert!(matches!(
            request(&pipe, &Request::Status),
            Err(ClientError::NotRunning)
        ));
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        assert_eq!(bind(&pipe).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn silent_client_does_not_block_others() {
        let pipe = unique("sc");
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        let _silent = Stream::connect(name(&pipe).unwrap()).unwrap();
        let start = Instant::now();
        assert!(request(&pipe, &Request::Status).unwrap().ok);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn invalid_and_oversized_requests_get_error_replies() {
        let pipe = unique("bad");
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        for payload in [
            b"{\"cmd\":\"strength\",\"value\":99}\n".to_vec(),
            vec![b'x'; (MAX_LINE + 4096) as usize],
        ] {
            // Full duplex: on Windows a pipe write blocks until the peer reads, so write on
            // one half while reading the reply on the other.
            let conn = Stream::connect(name(&pipe).unwrap()).unwrap();
            let (recv, mut send) = conn.split();
            let writer = std::thread::spawn(move || {
                let _ = send.write_all(&payload);
            });
            let mut reply = String::new();
            BufReader::new(recv).read_line(&mut reply).unwrap();
            let _ = writer.join();
            assert!(reply.contains("\"ok\":false"), "{reply}");
        }
        assert!(request(&pipe, &Request::Status).unwrap().ok);
    }
}
