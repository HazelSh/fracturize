//! `fracturize mcp`: the stdio MCP server Claude Code launches, and the
//! bridge from it to the running window (AGENT-COLLAB-PLAN.md §2, §5).
//!
//! ```sh
//! claude mcp add --scope project fracturize -- fracturize mcp     # once
//! claude -n fracturize --dangerously-load-development-channels server:fracturize
//! ```
//!
//! The one subcommand: neither do-one-thing-and-exit nor the GUI, but a
//! long-running server Claude Code talks to over stdin and stdout.
//!
//! **Two directions.** Tools (`tools/call`) are the agent's way to act: read
//! the live scene and a render of the view (through the window's socket,
//! `bridge.rs`), read a thread, and post requests, proposals, responses and
//! notes to it (as files, through `thread.rs`, exactly as the CLI does). And
//! **channel events** (`notifications/claude/channel`) are how the person
//! reaches the agent: a watcher reads every thread once a second and pushes
//! each new message for the agent into the session, which is the whole
//! difference between this and an agent looping on `--wait-reply`.
//!
//! **It works without a window.** The thread operations are files. Only
//! `get_state` and `get_render` need the window, and say so; the connection
//! is retried on each tool call, so a window opened later is found.
//!
//! **The JSON-RPC is hand-rolled** (§2): `initialize`, `tools/list`,
//! `tools/call`, `ping`, and the one notification. `rmcp` would bring tokio
//! into a binary that otherwise doesn't want it.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::bridge::Client;
use crate::scene::Scene;
use crate::thread::{self, Answer, Ask, Draft, Header, Kind, Thread, Verdict};

/// The protocol version answered when the client names none we know.
const PROTOCOL: &str = "2025-06-18";

/// How often the watcher reads the threads.
const WATCH: Duration = Duration::from_millis(1000);

const INSTRUCTIONS: &str = "\
You are connected to Fracturize, a 3D fractal flame renderer, and through it to the \
person using its window. Read AGENTS.md and CRAFT.md in the project before you change a \
scene.

Messages from the person arrive as <channel source=\"fracturize\" kind=... thread=... \
message=...> events. kind is request (they want something changed), proposal (their own \
edit, asking what you'd do with it, or answering a request of yours) or response (their \
verdict on your proposal: applied, rejected, or sent back with a note and maybe their \
tweak). The event body is the message as `fracturize --thread` prints it, with file paths.

Answer through these tools, passing the thread from the event:
- propose: your options, 1 to 9 complete scenes, each rendered for the person to compare. \
Base them on the request's base.toml (the scene as they saw it) and set replying_to. For a \
send-back, set revises to \"<message>/<option>\".
- respond: answer a proposal of theirs.
- reply: a note that wants no answer (progress, a question in passing).
- request: ask them for something.
get_state and get_render show the live window. Use the fracturize CLI (-S, --info, \
--render, sheets) for everything else, as usual. You never change the person's scene \
yourself: they preview, tweak and apply what you propose.";

/// Who this bridge is in a thread.
fn me() -> String {
    std::env::var("FRACTURIZE_AGENT").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "claude".to_string())
}

/// Everything the threads share: stdout (one writer at a time) and the
/// window, if there is one.
struct Shared {
    me: String,
    out: Mutex<std::io::Stdout>,
    window: Mutex<Option<Client>>,
    /// Why the window isn't connected, last time it was tried.
    window_error: Mutex<String>,
    /// Never look for a window (the tests: the real socket may belong to
    /// a window someone is using).
    offline: bool,
}

impl Shared {
    fn send(&self, v: &Value) {
        if let Ok(mut o) = self.out.lock() {
            let _ = writeln!(o, "{v}");
            let _ = o.flush();
        }
    }

    /// Call the window, connecting first if need be. `None`: no window.
    fn window(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut w = self.window.lock().map_err(|_| "poisoned")?;
        if w.is_none() {
            *w = self.connect();
        }
        let Some(c) = w.as_mut() else {
            return Err(self.window_error.lock().map(|e| e.clone()).unwrap_or_default());
        };
        match c.call(method, params) {
            Ok(v) => Ok(v),
            Err(e) => {
                *w = None;
                Err(e)
            }
        }
    }

    /// Like `window`, for courtesies: a missing window is nothing to report.
    fn tell(&self, method: &str, params: Value) {
        let _ = self.window(method, params);
    }

    fn connect(&self) -> Option<Client> {
        if self.offline {
            return None;
        }
        let set_err = |e: String| {
            if let Ok(mut we) = self.window_error.lock() {
                *we = e;
            }
        };
        let mut c = match Client::connect() {
            Ok(c) => c,
            Err(e) => {
                set_err(format!("{e}: start the Fracturize window to use this tool"));
                return None;
            }
        };
        let hello = json!({
            "name": self.me,
            "pid": std::process::id(),
            "session": std::env::var("CLAUDE_SESSION_NAME").unwrap_or_default(),
        });
        match c.call("hello", hello) {
            Ok(v) => {
                // Threads and scene paths are relative to where the window
                // runs; work from there too.
                if let Some(cwd) = v["cwd"].as_str() {
                    let _ = std::env::set_current_dir(cwd);
                }
                Some(c)
            }
            Err(e) => {
                set_err(e);
                None
            }
        }
    }
}

pub fn run(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "fracturize mcp: the MCP server Claude Code starts, bridging it to a running window\n\n\
             Set it up once in the project, then start a session that listens on it:\n\n  \
             claude mcp add --scope project fracturize -- fracturize mcp\n  \
             claude -n fracturize --dangerously-load-development-channels server:fracturize\n\n\
             It names itself $FRACTURIZE_AGENT in threads, else \"claude\". It speaks MCP on\n\
             stdin and stdout and is not meant to be run by hand."
        );
        return 0;
    }
    if let Some(a) = args.first() {
        eprintln!("fracturize mcp takes no arguments (got {a}); see fracturize mcp --help");
        return 1;
    }
    let shared = Arc::new(Shared {
        me: me(),
        out: Mutex::new(std::io::stdout()),
        window: Mutex::new(None),
        window_error: Mutex::new(String::new()),
        offline: false,
    });
    if let Ok(mut w) = shared.window.lock() {
        *w = shared.connect();
    }
    let watcher = {
        let shared = shared.clone();
        move || watch(&shared)
    };
    let mut watching = Some(watcher);

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                shared.send(&json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }));
                continue;
            }
        };
        let method = msg["method"].as_str().unwrap_or("");
        let id = msg.get("id").cloned();
        // The channel only exists once the client is ready for it.
        if method == "notifications/initialized"
            && let Some(w) = watching.take()
        {
            std::thread::Builder::new().name("mcp-watch".into()).spawn(w).ok();
        }
        let Some(id) = id else { continue }; // a notification: nothing to answer
        let result = match method {
            "initialize" => Ok(initialize(&msg["params"])),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(call(&shared, &msg["params"])),
            _ => Err(json!({ "code": -32601, "message": format!("no method {method}") })),
        };
        let reply = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
        };
        shared.send(&reply);
    }
    0
}

fn initialize(params: &Value) -> Value {
    let version = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
    json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": {},
            "experimental": { "claude/channel": {} },
        },
        "serverInfo": { "name": "fracturize", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

// ------------------------------------------------------------- the watcher

/// Read every thread once a second and push each new message for this agent
/// into the session. What was there at the start isn't replayed, but what's
/// waiting is said once, so a session started late knows.
fn watch(shared: &Shared) {
    let root = Path::new(thread::ROOT);
    let mut seen: BTreeMap<PathBuf, u32> = BTreeMap::new();
    let mut waiting = Vec::new();
    for t in Thread::all(root) {
        seen.insert(t.dir.clone(), t.messages.iter().map(|m| m.number()).max().unwrap_or(0));
        for m in t.messages.iter().filter(|m| for_me(&shared.me, &m.header) && t.waiting(m)) {
            waiting.push(format!("{}/{} from {}: {} ({})", t.name(), m.id, m.header.from, m.header.headline(), m.header.kind.name()));
        }
    }
    if !waiting.is_empty() {
        let content = format!(
            "{} message{} were already waiting for you when this session connected:\n{}\n\nRead one with the thread tool.",
            waiting.len(),
            if waiting.len() == 1 { "" } else { "s" },
            waiting.join("\n")
        );
        push(shared, content, json!({ "kind": "waiting" }));
    }
    loop {
        std::thread::sleep(WATCH);
        for t in Thread::all(root) {
            let last = seen.get(&t.dir).copied().unwrap_or(0);
            let mut top = last;
            for m in t.messages.iter().filter(|m| m.number() > last) {
                top = top.max(m.number());
                if !for_me(&shared.me, &m.header) {
                    continue;
                }
                let meta = json!({
                    "kind": m.header.kind.name(),
                    "thread": t.name(),
                    "message": m.id,
                    "from": m.header.from,
                    "scene": t.file.scene,
                });
                push(shared, thread::render_message(&t, m), meta);
                shared.tell("delivered", json!({ "thread": t.name(), "message": m.id }));
            }
            seen.insert(t.dir.clone(), top);
        }
    }
}

/// Whether a message is for this agent: not from it, and addressed to it,
/// to agents in general, or to nobody in particular.
fn for_me(me: &str, h: &Header) -> bool {
    h.from != me && (h.to.is_empty() || h.to == me || h.to == "agent")
}

fn push(shared: &Shared, content: String, meta: Value) {
    shared.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": { "content": content, "meta": meta },
    }));
}

// ------------------------------------------------------------------ tools

fn tools() -> Value {
    let thread_arg = json!({
        "type": "string",
        "description": "The thread: its name (the `thread` attribute of an event, e.g. \"koru\") or a scene path. \
                        Leave it out for the scene on screen in the window."
    });
    let opt_schema = json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "Short and plain: A, B, B2" },
            "scene_toml": { "type": "string", "description": "The whole scene, as TOML" },
            "scene_file": { "type": "string", "description": "Or: a path to the scene file" },
            "render_file": { "type": "string", "description": "A render of it, if you made one; otherwise one is made" },
            "note": { "type": "string", "description": "One line: what this option does" }
        },
        "required": ["id"]
    });
    json!([
        {
            "name": "get_state",
            "description": "The window as it is: the scene on screen (as TOML, with the live framing and look), \
                            its path, whether it has unsaved edits, the selected transform, the renderer, the thread, \
                            and whether an option is being previewed.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_render",
            "description": "A picture of the window's view as it is now, as the person sees it (without the gizmos).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_threads",
            "description": "Every thread, and the messages in each still waiting for an answer.",
            "inputSchema": { "type": "object", "properties": { "waiting_only": { "type": "boolean" } } }
        },
        {
            "name": "thread",
            "description": "Read a thread: every message, and each option's changes from its base as -S lines.",
            "inputSchema": { "type": "object", "properties": {
                "thread": thread_arg,
                "since": { "type": "string", "description": "Only messages after this one, e.g. \"0003\"" }
            } }
        },
        {
            "name": "propose",
            "description": "Post a proposal: 1 to 9 options, each a complete scene. Options without a render get one \
                            (a small splat still), so the person can compare them at a glance. The base is the scene \
                            the options were made from: give base_toml, or it is taken from the message you reply \
                            to (or revise).",
            "inputSchema": { "type": "object", "properties": {
                "thread": thread_arg,
                "message": { "type": "string", "description": "What you did and why, briefly" },
                "options": { "type": "array", "items": opt_schema, "minItems": 1, "maxItems": 9 },
                "ask": { "type": "string", "enum": ["pick-one", "pick-any", "free"], "description": "[pick-one]" },
                "replying_to": { "type": "string", "description": "The message this answers, e.g. \"0001\"" },
                "revises": { "type": "string", "description": "\"<message>/<option>\" when this is a new round of one" },
                "base_toml": { "type": "string" }
            }, "required": ["message", "options"] }
        },
        {
            "name": "respond",
            "description": "Answer a proposal of the person's: pick options, reject them, comment, or send one back \
                            with a note (and your own version of it).",
            "inputSchema": { "type": "object", "properties": {
                "thread": thread_arg,
                "message": { "type": "string", "description": "The proposal, e.g. \"0004\"" },
                "text": { "type": "string", "description": "Your verdict, in words" },
                "picks": { "type": "array", "items": { "type": "string" } },
                "rejects": { "type": "array", "items": { "type": "object", "properties": {
                    "option": { "type": "string" }, "reason": { "type": "string" } }, "required": ["option"] } },
                "comments": { "type": "array", "items": { "type": "object", "properties": {
                    "option": { "type": "string" }, "note": { "type": "string" } }, "required": ["option", "note"] } },
                "send_back": { "type": "array", "items": { "type": "object", "properties": {
                    "option": { "type": "string" }, "note": { "type": "string" },
                    "scene_toml": { "type": "string", "description": "Your tweak of it" } }, "required": ["option"] } }
            }, "required": ["message"] }
        },
        {
            "name": "reply",
            "description": "A note that wants no answer: progress, a question in passing, where things stand.",
            "inputSchema": { "type": "object", "properties": {
                "thread": thread_arg,
                "text": { "type": "string" },
                "replying_to": { "type": "string" }
            }, "required": ["text"] }
        },
        {
            "name": "request",
            "description": "Ask the person for something, e.g. \"frame it the way you'd hang it and send it back\". \
                            The thread's scene goes with it as base.toml.",
            "inputSchema": { "type": "object", "properties": {
                "thread": thread_arg,
                "text": { "type": "string" },
                "transforms": { "type": "array", "items": { "type": "string" }, "description": "Maps it is about, by name" }
            }, "required": ["text"] }
        },
        {
            "name": "withdraw",
            "description": "Take back a message of yours that hasn't been answered. It stays in the thread, greyed.",
            "inputSchema": { "type": "object", "properties": {
                "thread": thread_arg,
                "message": { "type": "string" },
                "reason": { "type": "string" }
            }, "required": ["message"] }
        }
    ])
}

fn call(shared: &Shared, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];
    let result = match name {
        "get_state" => get_state(shared),
        "get_render" => return get_render(shared),
        "list_threads" => Ok(list_threads(shared, args["waiting_only"].as_bool().unwrap_or(false))),
        "thread" => read_thread(shared, args),
        "propose" => propose(shared, args),
        "respond" => respond(shared, args),
        "reply" => reply(shared, args),
        "request" => request(shared, args),
        "withdraw" => withdraw(shared, args),
        _ => Err(format!("no tool {name}")),
    };
    match result {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn get_state(shared: &Shared) -> Result<String, String> {
    let v = shared.window("get_state", json!({}))?;
    let mut out = String::new();
    for key in ["scene_path", "thread", "dirty", "selection", "renderer", "previewing"] {
        if let Some(x) = v.get(key).filter(|x| !x.is_null()) {
            out.push_str(&format!("{key}: {}\n", x.as_str().map_or_else(|| x.to_string(), str::to_string)));
        }
    }
    out.push_str("\n```toml\n");
    out.push_str(v["scene_toml"].as_str().unwrap_or(""));
    out.push_str("```\n");
    Ok(out)
}

fn get_render(shared: &Shared) -> Value {
    let path = std::env::temp_dir().join(format!("fracturize-mcp-view-{}-{}.png", std::process::id(), thread::now()));
    let r = shared.window("get_render", json!({ "path": path }));
    match r.and_then(|_| std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))) {
        Ok(bytes) => json!({ "content": [
            { "type": "image", "data": base64(&bytes), "mimeType": "image/png" },
            { "type": "text", "text": format!("the view, 1280x720: {}", path.display()) },
        ] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

fn list_threads(shared: &Shared, waiting_only: bool) -> String {
    let mut out = String::new();
    for t in Thread::all(Path::new(thread::ROOT)) {
        let waiting: Vec<_> = t.messages.iter().filter(|m| t.waiting(m)).collect();
        if waiting_only && !waiting.iter().any(|m| m.header.from != shared.me) {
            continue;
        }
        out.push_str(&format!("{}  scene {}  {} messages\n", t.name(), t.file.scene, t.messages.len()));
        for m in waiting {
            let on = if m.header.from == shared.me { "waiting on them" } else { "waiting on you" };
            out.push_str(&format!("    {} from {}: {} [{on}]\n", m.id, m.header.from, m.header.headline()));
        }
    }
    if out.is_empty() {
        out = if waiting_only { "nothing waiting for you".into() } else { "no threads yet".into() };
    }
    out
}

/// Where a thread is, and the scene path to create it under.
struct Where {
    dir: PathBuf,
    scene: String,
}

fn locate(shared: &Shared, args: &Value) -> Result<Where, String> {
    let name = match str_arg(args, "thread") {
        Some(n) => n.to_string(),
        None => {
            let v = shared
                .window("get_state", json!({ "brief": true }))
                .map_err(|e| format!("say which thread ({e})"))?;
            v["thread"].as_str().ok_or("the window has no thread to name")?.to_string()
        }
    };
    if name.ends_with(".toml") || name.ends_with(".png") {
        let path = Path::new(&name);
        let scene = Scene::load(path).map_err(|e| format!("{name}: {e}"))?;
        return Ok(Where { dir: thread::dir_for(Some(path), &scene.name), scene: name });
    }
    let name = name.trim_start_matches(&format!("{}/", thread::ROOT)).to_string();
    if name.contains(['/', '\\']) || name == ".." {
        return Err(format!("{name}: a thread is named by its folder under {}", thread::ROOT));
    }
    let dir = Path::new(thread::ROOT).join(&name);
    let t = Thread::open(&dir)?;
    let scene = if !t.file.scene.is_empty() {
        t.file.scene.clone()
    } else {
        // A thread not started yet: the window's scene, if it's this one.
        let v = shared.window("get_state", json!({ "brief": true })).unwrap_or(Value::Null);
        if v["thread"].as_str() == Some(name.as_str()) {
            v["scene_path"].as_str().unwrap_or("").to_string()
        } else {
            return Err(format!("there's no thread {name} yet; name a scene path to start one"));
        }
    };
    Ok(Where { dir, scene })
}

fn read_thread(shared: &Shared, args: &Value) -> Result<String, String> {
    let w = locate(shared, args)?;
    let t = Thread::open(&w.dir)?;
    if t.messages.is_empty() {
        return Ok(format!("no messages in {} yet", w.dir.display()));
    }
    let since = match str_arg(args, "since") {
        Some(id) => Some(t.get(id).ok_or_else(|| format!("since: no message {id}"))?.number()),
        None => None,
    };
    let seen: Vec<&str> = t
        .messages
        .iter()
        .filter(|m| for_me(&shared.me, &m.header) && since.is_none_or(|s| m.number() > s))
        .map(|m| m.id.as_str())
        .collect();
    if !seen.is_empty() {
        shared.tell("seen", json!({ "thread": t.name(), "messages": seen }));
    }
    Ok(thread::render_text(&t, since))
}

fn posted(shared: &Shared, w: &Where, id: &str) {
    let name = w.dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    shared.tell("posted", json!({ "thread": name, "message": id }));
}

fn request(shared: &Shared, args: &Value) -> Result<String, String> {
    let w = locate(shared, args)?;
    let text = str_arg(args, "text").ok_or("request: say what you'd like")?;
    let t = Thread::open(&w.dir)?;
    let mut h = Header::new(Kind::Request, &shared.me, &t.other_party(&shared.me, "human"), text);
    if let Some(a) = args["transforms"].as_array() {
        h.selection = a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    }
    let mut draft = Draft::new(h);
    if let Ok(scene) = Scene::load(&w.scene) {
        draft.scene("base.toml", &scene)?;
    }
    let id = thread::post(&w.dir, &w.scene, draft)?;
    posted(shared, &w, &id);
    Ok(format!("posted {id} to {}", w.dir.display()))
}

fn reply(shared: &Shared, args: &Value) -> Result<String, String> {
    let w = locate(shared, args)?;
    let text = str_arg(args, "text").ok_or("reply: nothing to say")?;
    let t = Thread::open(&w.dir)?;
    let mut h = Header::new(Kind::Note, &shared.me, &t.other_party(&shared.me, "human"), text);
    if let Some(r) = str_arg(args, "replying_to") {
        h.replying_to = Some(t.get(r).ok_or_else(|| format!("replying_to: no message {r}"))?.id.clone());
    }
    let id = thread::post(&w.dir, &w.scene, Draft::new(h))?;
    posted(shared, &w, &id);
    Ok(format!("posted {id}"))
}

fn withdraw(shared: &Shared, args: &Value) -> Result<String, String> {
    let w = locate(shared, args)?;
    let id = str_arg(args, "message").ok_or("withdraw: which message?")?;
    let t = Thread::open(&w.dir)?;
    let m = t.get(id).ok_or_else(|| format!("no message {id}"))?;
    if m.header.from != shared.me {
        return Err(format!("{} is from {}, not {}; only its sender can take it back", m.id, m.header.from, shared.me));
    }
    let id = thread::withdraw(&t, id, str_arg(args, "reason").unwrap_or(""))?;
    posted(shared, &w, &id);
    Ok(format!("withdrew {id}"))
}

/// A scene from `scene_toml` or `scene_file`, checked, as text.
fn scene_text(args: &Value, what: &str) -> Result<String, String> {
    let text = match (str_arg(args, "scene_toml"), str_arg(args, "scene_file")) {
        (Some(t), _) => t.to_string() + "\n",
        (None, Some(f)) => std::fs::read_to_string(f).map_err(|e| format!("{what}: {f}: {e}"))?,
        (None, None) => return Err(format!("{what}: give scene_toml or scene_file")),
    };
    Ok(text)
}

fn propose(shared: &Shared, args: &Value) -> Result<String, String> {
    let w = locate(shared, args)?;
    let t = Thread::open(&w.dir)?;
    let text = str_arg(args, "message").unwrap_or("");
    let mut h = Header::new(Kind::Proposal, &shared.me, &t.other_party(&shared.me, "human"), text);
    h.ask = Some(match str_arg(args, "ask") {
        None | Some("pick-one") => Ask::PickOne,
        Some("pick-any") => Ask::PickAny,
        Some("free") => Ask::Free,
        Some(a) => return Err(format!("ask {a}: say pick-one, pick-any or free")),
    });
    let mut answering = None;
    if let Some(r) = str_arg(args, "replying_to") {
        let m = t.get(r).ok_or_else(|| format!("replying_to: no message {r} in {}", w.dir.display()))?;
        h.replying_to = Some(m.id.clone());
        answering = Some(m);
    }
    if let Some(r) = str_arg(args, "revises") {
        let (msg, opt) = r.split_once('/').ok_or("revises: say \"<message>/<option>\", e.g. \"0002/B\"")?;
        let m = t.get(msg).ok_or_else(|| format!("revises: no message {msg}"))?;
        m.option(opt).ok_or_else(|| format!("revises: {} has no option {opt}", m.id))?;
        h.revises = Some(format!("{}/{opt}", m.id));
        answering = answering.or(Some(m));
    }

    // The base: given, else what the answered message was made from, else
    // the newest base in the thread, else the scene file.
    let base_text = match str_arg(args, "base_toml") {
        Some(b) => b.to_string() + "\n",
        None => {
            let from_msg = answering
                .into_iter()
                .chain(t.messages.iter().rev())
                .map(|m| m.file("base.toml"))
                .find(|p| p.is_file());
            match from_msg {
                Some(p) => std::fs::read_to_string(&p).map_err(|e| e.to_string())?,
                None => std::fs::read_to_string(&w.scene).map_err(|e| format!("no base to compare with ({}: {e}); give base_toml", w.scene))?,
            }
        }
    };

    let opts = args["options"].as_array().ok_or("propose: options is a list")?;
    if opts.is_empty() || opts.len() > thread::MAX_OPTIONS {
        return Err(format!("a proposal has 1 to {} options, not {}", thread::MAX_OPTIONS, opts.len()));
    }
    // Build it as a proposal directory and let the CLI's checks have it:
    // every option parses, ids are plain and distinct, and so on.
    let stage = std::env::temp_dir().join(format!("fracturize-mcp-{}-{}", std::process::id(), thread::now()));
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let built = (|| -> Result<Draft, String> {
        std::fs::write(stage.join("base.toml"), &base_text).map_err(|e| e.to_string())?;
        let renderer = shared.window("get_state", json!({ "brief": true })).ok().and_then(|v| v["renderer"].as_str().map(str::to_string));
        for o in opts {
            let id = str_arg(o, "id").ok_or("every option needs an id")?;
            let what = format!("option {id}");
            if id.contains(['/', '\\', ' ']) {
                return Err(format!("{what}: an id is plain, like A or B2"));
            }
            let scene = format!("{id}.toml");
            std::fs::write(stage.join(&scene), scene_text(o, &what)?).map_err(|e| e.to_string())?;
            Scene::load(stage.join(&scene)).map_err(|e| format!("{what} doesn't load: {e}"))?;
            let render = match str_arg(o, "render_file") {
                Some(f) => {
                    let name = format!("{id}.png");
                    std::fs::copy(f, stage.join(&name)).map_err(|e| format!("{what}: {f}: {e}"))?;
                    Some(name)
                }
                None => {
                    let name = format!("{id}.png");
                    match render_option(&stage.join(&scene), &stage.join(&name), renderer.as_deref()) {
                        Ok(()) => Some(name),
                        Err(e) => {
                            eprintln!("fracturize mcp: couldn't render {what}: {e}");
                            None
                        }
                    }
                }
            };
            h.options.push(thread::Opt { id: id.to_string(), scene, render, note: str_arg(o, "note").unwrap_or("").to_string() });
        }
        let msg = toml::to_string(&h).map_err(|e| e.to_string())?;
        std::fs::write(stage.join("message.toml"), msg).map_err(|e| e.to_string())?;
        thread::proposal_from_dir(&stage, &shared.me, &h.to)
    })();
    let result = built.and_then(|draft| thread::post(&w.dir, &w.scene, draft));
    let _ = std::fs::remove_dir_all(&stage);
    let id = result?;
    posted(shared, &w, &id);
    let t = Thread::open(&w.dir)?;
    Ok(match t.get(&id) {
        Some(m) => format!("posted {id}\n\n{}", thread::render_message(&t, m)),
        None => format!("posted {id}"),
    })
}

/// A small still of an option, for its card: this binary, rendering the way
/// the window is drawing (splat unless it says points).
pub fn render_option(scene: &Path, png: &Path, renderer: Option<&str>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("-s").arg(scene).arg("-r").arg(png);
    cmd.args(["--effort", "small", "--width", "640", "--height", "360", "--no-labels"]);
    match renderer {
        Some("points") => {}
        Some("volume") => {
            cmd.arg("--volume");
        }
        _ => {
            cmd.arg("--splat");
        }
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() || !png.exists() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().last().unwrap_or("the render failed").to_string());
    }
    // The sidecar would land in the thread beside the picture; it isn't
    // part of the message.
    let _ = std::fs::remove_file(png.with_extension("render.toml"));
    Ok(())
}

fn respond(shared: &Shared, args: &Value) -> Result<String, String> {
    let w = locate(shared, args)?;
    let target = str_arg(args, "message").ok_or("respond: which proposal?")?;
    let t = Thread::open(&w.dir)?;
    let m = t.get(target).ok_or_else(|| format!("no message {target} in {}", w.dir.display()))?;
    if m.header.kind != Kind::Proposal {
        return Err(format!("respond answers a proposal; {} is a {}. Answer a request with propose.", m.id, m.header.kind.name()));
    }
    if m.withdrawn.is_some() {
        return Err(format!("{} was withdrawn", m.id));
    }
    let mut h = Header::new(Kind::Response, &shared.me, &m.header.from, str_arg(args, "text").unwrap_or(""));
    h.replying_to = Some(m.id.clone());
    let check = |id: &str| -> Result<(), String> {
        m.option(id).map(|_| ()).ok_or_else(|| {
            let ids: Vec<_> = m.header.options.iter().map(|o| o.id.as_str()).collect();
            format!("{} has no option {id} (it has {})", m.id, ids.join(", "))
        })
    };
    let answer = |id: &str, verdict, note: &str| Answer {
        option: id.to_string(),
        verdict,
        note: note.to_string(),
        tweak: None,
        tweak_render: None,
    };
    let mut files = Vec::new();
    for p in args["picks"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        check(p)?;
        h.answers.push(answer(p, Verdict::Picked, ""));
    }
    for r in args["rejects"].as_array().into_iter().flatten() {
        let id = str_arg(r, "option").ok_or("rejects: each names an option")?;
        check(id)?;
        h.answers.push(answer(id, Verdict::Rejected, str_arg(r, "reason").unwrap_or("")));
    }
    for c in args["comments"].as_array().into_iter().flatten() {
        let id = str_arg(c, "option").ok_or("comments: each names an option")?;
        check(id)?;
        h.answers.push(answer(id, Verdict::Comment, str_arg(c, "note").unwrap_or("")));
    }
    for s in args["send_back"].as_array().into_iter().flatten() {
        let id = str_arg(s, "option").ok_or("send_back: each names an option")?;
        check(id)?;
        let mut a = answer(id, Verdict::SentBack, str_arg(s, "note").unwrap_or(""));
        if let Some(toml) = str_arg(s, "scene_toml") {
            let scene: Scene = Scene::from_toml_str(toml, Path::new(".")).map_err(|e| format!("send_back {id}: the scene doesn't load: {e}"))?;
            let name = format!("{id}.tweak.toml");
            files.push((name.clone(), scene));
            a.tweak = Some(name);
        }
        h.answers.push(a);
    }
    let mut said = std::collections::BTreeSet::new();
    for a in &h.answers {
        if !said.insert(a.option.clone()) {
            return Err(format!("option {} is given two verdicts", a.option));
        }
    }
    if h.answers.is_empty() && h.message.trim().is_empty() {
        return Err("respond: say something, or pick, reject, comment on or send back an option".into());
    }
    if m.header.ask == Some(Ask::PickOne) && h.answers.iter().filter(|a| a.verdict == Verdict::Picked).count() > 1 {
        return Err(format!("{} asks you to pick one", m.id));
    }
    let mut draft = Draft::new(h);
    for (name, scene) in &files {
        draft.scene(name, scene)?;
    }
    let id = thread::post(&w.dir, &w.scene, draft)?;
    posted(shared, &w, &id);
    Ok(format!("posted {id}, answering {}", m.id))
}

/// Standard base64, padded. Twenty lines rather than a dependency.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn a_message_is_for_an_agent_unless_it_sent_it_or_it_names_someone_else() {
        let h = |from: &str, to: &str| Header::new(Kind::Request, from, to, "x");
        assert!(for_me("claude", &h("hazel", "agent")));
        assert!(for_me("claude", &h("hazel", "claude")));
        assert!(for_me("claude", &h("hazel", "")));
        assert!(!for_me("claude", &h("claude", "hazel")));
        assert!(!for_me("claude", &h("hazel", "gemma4-e4b")));
    }

    #[test]
    fn every_tool_has_a_schema_and_a_handler() {
        let shared = Shared {
            me: "t".into(),
            out: Mutex::new(std::io::stdout()),
            window: Mutex::new(None),
            window_error: Mutex::new("no window".into()),
            offline: true,
        };
        for t in tools().as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            assert!(t["inputSchema"]["type"] == "object", "{name}");
            let r = call(&shared, &json!({ "name": name, "arguments": {} }));
            let text = r["content"][0]["text"].as_str().unwrap_or("");
            assert!(!text.starts_with("no tool"), "{name} has no handler");
        }
    }
}
