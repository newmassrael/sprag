//! The client half of the transport: a blocking JSON-RPC connection to a host
//! socket.
//!
//! [`mount`](crate::mount) is the SERVER end (bind + accept + dispatch). This is
//! the CLIENT end a display client (`sprag-gui`'s `WireHost`) drives: connect to a
//! `sprag-term` host's always-on Unix socket and issue newline-delimited JSON-RPC
//! requests, reading one response line per request. The host serves each
//! connection on its own handler thread and funnels every frame into ONE dispatch
//! owner, so a client may hold SEVERAL [`HostConn`]s concurrently (e.g. one parked
//! on a long-poll `scene/waitFor` while another issues cell reads) without
//! head-of-line blocking — each connection is an independent request/response
//! stream.
//!
//! A [`HostConn`] is single-threaded (one outstanding request at a time): the
//! transport is strictly request→response (no server push — an async
//! `scene/waitFor` is a *deferred response* to the client's own request, still one
//! reply per request), so a connection never desyncs its read stream. A caller
//! that needs concurrency uses more connections, not shared mutable access.

use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A blocking JSON-RPC connection to a host socket — the client end of the wire.
///
/// One request/response at a time (see the module docs). Construct with
/// [`connect`](Self::connect) (which tolerates the spawn race by retrying until
/// the socket accepts), then [`call`](Self::call) per request.
pub struct HostConn {
    /// The write half (requests out). A `UnixStream` is bidirectional; this clone
    /// owns writes while `reader` owns the buffered read half.
    writer: UnixStream,
    /// The buffered read half (newline-delimited responses in).
    reader: BufReader<UnixStream>,
    /// The next JSON-RPC request id. Monotonic; the server echoes it back.
    next_id: u64,
}

impl HostConn {
    /// Connect to the host socket at `path`, retrying until it accepts or `timeout`
    /// elapses — so a client that spawned its host tolerates the bind race (the
    /// child has not yet bound the socket at the instant the parent connects).
    ///
    /// # Errors
    ///
    /// Returns the last connect error if `timeout` elapses before the socket
    /// accepts, or an I/O error if the accepted stream cannot be split for reading.
    pub fn connect(path: &Path, timeout: Duration) -> io::Result<Self> {
        let start = Instant::now();
        loop {
            match UnixStream::connect(path) {
                Ok(stream) => return Self::from_stream(stream),
                Err(error) => {
                    if start.elapsed() >= timeout {
                        return Err(error);
                    }
                    sleep(Duration::from_millis(20));
                }
            }
        }
    }

    /// Wrap an already-connected stream (splitting it into read + write halves).
    fn from_stream(stream: UnixStream) -> io::Result<Self> {
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            writer: stream,
            reader,
            next_id: 1,
        })
    }

    /// Issue one `method` request with `params` and block until its response line
    /// arrives, returning the JSON-RPC `result` value (`Null` when absent). A
    /// JSON-RPC `error` object in the reply is surfaced as an [`io::Error`]; a
    /// closed connection (host gone) is [`ErrorKind::UnexpectedEof`].
    ///
    /// Blocking is the point for `scene/waitFor {since}`: the host parks that
    /// reply until a pane produces output, so this read blocks (cheaply) until the
    /// change-notification fires — the long-poll a wire client repaints off.
    ///
    /// # Errors
    ///
    /// I/O failure writing the request or reading the reply, a malformed reply, or
    /// a JSON-RPC `error` object in the response.
    pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.writer, "{request}")?;
        self.writer.flush()?;

        // Read the next non-blank response line (the server terminates each reply
        // with a newline; blank lines, if any, are skipped).
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "host closed the connection",
                ));
            }
            if !line.trim().is_empty() {
                break;
            }
        }

        let response: Value = serde_json::from_str(line.trim())
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        if let Some(error) = response.get("error") {
            return Err(io::Error::other(format!("host rpc error: {error}")));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_rpc::{RpcFrame, RpcIngress};
    use pinion_rpc_transport::UnixSocketTransport;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;

    /// A trivial ingress: funnel frames to a channel a test-owned dispatch thread
    /// answers, so `HostConn` drives a REAL socket end-to-end (the same
    /// [`UnixSocketTransport`] the GUI/host mount) without standing up a full
    /// `HostState` or the mount policy layer (env + process-global `ENDPOINT`).
    struct ChannelIngress {
        tx: Sender<RpcFrame>,
    }
    impl RpcIngress for ChannelIngress {
        fn submit(&self, frame: RpcFrame) {
            let _ = self.tx.send(frame);
        }
    }

    /// Answer frames by echoing the request's `params` back as the `result` — enough
    /// to prove request framing + response parsing round-trip over the real socket.
    fn echo_dispatch(rx: Receiver<RpcFrame>) {
        for frame in rx {
            let request: Value = serde_json::from_str(&frame.request).unwrap();
            let response = json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": request["params"].clone(),
            });
            frame.reply.send(response.to_string());
        }
    }

    #[test]
    fn call_round_trips_a_request_over_the_socket() {
        // A unique socket under the temp dir (pid-scoped so parallel test binaries
        // do not collide). Bind the transport directly — no env, no global.
        let path =
            std::env::temp_dir().join(format!("sprag-rpc-client-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (tx, rx) = channel();
        thread::spawn(move || echo_dispatch(rx));
        let control = UnixSocketTransport::serve(&path, Arc::new(ChannelIngress { tx }))
            .expect("bind the test socket");
        control.set_enabled(true);

        let mut conn =
            HostConn::connect(&path, Duration::from_secs(2)).expect("connect to the socket");
        // Two calls prove the id increments and the read stream stays in sync.
        assert_eq!(
            conn.call("scene/echo", json!({"hello": "world"})).unwrap(),
            json!({"hello": "world"})
        );
        assert_eq!(conn.call("scene/echo", json!(42)).unwrap(), json!(42));

        drop(control);
        let _ = std::fs::remove_file(&path);
    }
}
