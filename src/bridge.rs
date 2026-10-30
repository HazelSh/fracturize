//! The GUI's socket (AGENT-COLLAB-PLAN.md §2, §5.1): how `fracturize mcp`,
//! the stdio bridge Claude Code launches, reaches the running window.
//!
//! ```text
//!  Claude Code ──stdio MCP──▶ fracturize mcp ──this socket──▶ the window
//! ```
//!
//! JSON lines over `$XDG_RUNTIME_DIR/fracturize/gui.sock`: `{"id", "method",
//! "params"}` one way, `{"id", "result"}` or `{"id", "error"}` back. The
//! window owns the socket; a bridge connects to it.
//!
//! **The socket carries only what needs the live window.** Messages are
//! files (`thread.rs`), and the bridge writes and reads those itself, the
//! same way the CLI does, so a thread works with no window at all. What the
//! socket adds is the live state (the scene as it is on screen, a render of
//! the view) and three courtesies for the window's sake: `hello` (who's
//! connected: the light in the Collaborate window), `posted` (read the
//! thread now rather than at the next rescan) and `delivered`/`seen` (the
//! message states, which aren't files because a message is written once).
//!
//! **One bridge at a time.** A second `hello` is refused, naming the session
//! that already holds the window. Two agents answering one person's request
//! at once is a muddle nobody asked for.
//!
//! **Never a frame's worth of waiting.** A thread per connection reads lines
//! and hands each request to the window over a channel; the window answers
//! from its frame loop (`App::poll_bridge`), which runs continuously
//! (`ControlFlow::Poll`), so nothing needs waking.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Where the window listens.
pub fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("fracturize-{}", unsafe { libc::getuid() })));
    base.join("fracturize").join("gui.sock")
}

/// A request from a bridge, for the window to answer.
pub struct Call {
    /// Which connection it came on.
    pub conn: u64,
    pub method: String,
    pub params: Value,
    reply: Replier,
}

impl Call {
    pub fn ok(self, result: Value) {
        self.reply.send(json!({ "result": result }));
    }

    pub fn err(self, message: impl Into<String>) {
        self.reply.send(json!({ "error": message.into() }));
    }
}

/// The way back to one bridge for one request.
struct Replier {
    id: Value,
    stream: Arc<Mutex<UnixStream>>,
}

impl Replier {
    fn send(self, mut body: Value) {
        body["id"] = self.id;
        if let Ok(mut s) = self.stream.lock() {
            let _ = writeln!(s, "{body}");
        }
    }
}

/// What the listener tells the window besides calls.
pub enum Event {
    Call(Call),
    /// A connection closed.
    Gone(u64),
}

/// The window's end: a listener thread, and what it has heard.
pub struct Server {
    pub path: PathBuf,
    rx: Receiver<Event>,
}

impl Server {
    /// Listen, unless another window already does. A socket file nobody
    /// answers on is a window that died; it's taken over.
    pub fn start() -> Result<Server, String> {
        Self::start_at(socket_path())
    }

    pub fn start_at(path: PathBuf) -> Result<Server, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(format!("another Fracturize window already listens on {}", path.display()));
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("bridge-listen".into())
            .spawn(move || {
                let mut next = 1u64;
                for stream in listener.incoming().flatten() {
                    let conn = next;
                    next += 1;
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new().name(format!("bridge-{conn}")).spawn(move || serve(conn, stream, tx));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Server { path, rx })
    }

    /// Everything heard since last asked.
    pub fn drain(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(conn: u64, stream: UnixStream, tx: Sender<Event>) {
    let Ok(read) = stream.try_clone() else { return };
    let write = Arc::new(Mutex::new(stream));
    for line in BufReader::new(read).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                if let Ok(mut s) = write.lock() {
                    let _ = writeln!(s, "{}", json!({ "id": null, "error": format!("not JSON: {e}") }));
                }
                continue;
            }
        };
        let call = Call {
            conn,
            method: msg["method"].as_str().unwrap_or("").to_string(),
            params: msg.get("params").cloned().unwrap_or(Value::Null),
            reply: Replier { id: msg.get("id").cloned().unwrap_or(Value::Null), stream: write.clone() },
        };
        if tx.send(Event::Call(call)).is_err() {
            return;
        }
    }
    let _ = tx.send(Event::Gone(conn));
}

/// The bridge's end: one connection to the window.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Client {
    pub fn connect() -> Result<Client, String> {
        Self::connect_at(&socket_path())
    }

    pub fn connect_at(path: &std::path::Path) -> Result<Client, String> {
        let s = UnixStream::connect(path).map_err(|_| format!("no Fracturize window is listening on {}", path.display()))?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(60))).ok();
        let writer = s.try_clone().map_err(|e| e.to_string())?;
        Ok(Client { reader: BufReader::new(s), writer, next: 1 })
    }

    /// One request, one answer.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        writeln!(self.writer, "{}", json!({ "id": id, "method": method, "params": params }))
            .map_err(|e| format!("the window went away: {e}"))?;
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).map_err(|e| format!("no answer from the window: {e}"))?;
            if n == 0 {
                return Err("the window went away".to_string());
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| format!("the window said something odd: {e}"))?;
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                return Err(e.as_str().map_or_else(|| e.to_string(), str::to_string));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_and_its_answer_cross_the_socket() {
        // Its own socket: the real one may belong to a window someone is using.
        let dir = std::env::temp_dir().join(format!("fracturize-bridge-test-{}", std::process::id()));
        let path = dir.join("gui.sock");
        let server = Server::start_at(path.clone()).unwrap();
        assert!(Server::start_at(path.clone()).is_err(), "a second window must not take the socket");
        let p = path.clone();
        let answer = std::thread::spawn(move || {
            let mut c = Client::connect_at(&p).unwrap();
            c.call("echo", json!({ "x": 3 }))
        });
        // Look through every event: the refused second start probed the
        // socket by connecting, and that probe's `Gone` can arrive in the
        // same drain as the call. Taking only the first dropped the call.
        let call = loop {
            if let Some(c) = server.drain().into_iter().find_map(|e| match e {
                Event::Call(c) => Some(c),
                Event::Gone(_) => None,
            }) {
                break c;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(call.method, "echo");
        let x = call.params["x"].clone();
        call.ok(json!({ "x": x }));
        assert_eq!(answer.join().unwrap().unwrap(), json!({ "x": 3 }));
        drop(server);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
