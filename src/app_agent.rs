//! The window's side of the agent connection (AGENT-COLLAB-PLAN.md §5, §6):
//! the socket a Claude Code session's bridge reaches it on, and the two
//! backends the window drives itself, headless Claude and a local model.
//!
//! Three things, each polled once a frame (the loop runs continuously, so
//! nothing waits on a wake-up):
//!
//! - **The socket** (`bridge.rs`). A bridge says `hello` and is the one
//!   connected agent, the Collaborate window's green light; a second is
//!   refused, naming the first. It asks for the live state and for a render
//!   of the view, and says when it has posted (read the thread now) and when
//!   a message was delivered or seen. Those two states aren't files: a
//!   message is written once.
//! - **Dispatch.** With `claude-headless` or `local` chosen and no session
//!   connected, each new message the person sends (a request, their own
//!   proposal, a send-back) starts the backend on it, one at a time. Only
//!   messages sent since the window opened or the backend was chosen, so
//!   switching to a backend never answers last week's thread.
//! - **The runs**: their progress for the working line, a note in the thread
//!   when a headless run ends without having posted, and Stop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agent::{Backend, Headless, LlmServer, Update};
use crate::agent_local::{self, Conversation, LocalRun, Status, Trigger};
use crate::bridge::{self, Call, Event};
use crate::thread::{self, Draft, Header, Kind, Message, Thread, Verdict};

use super::{App, Tone};

/// How long a render a bridge asked for may take before it's given up on.
const RENDER_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a message may wait on a connected session before the window
/// says "check the terminal".
const QUIET: Duration = Duration::from_secs(10 * 60);

/// The connected bridge.
pub struct Connected {
    conn: u64,
    pub name: String,
    pub pid: u64,
    pub since: Instant,
}

#[derive(Default)]
pub struct AgentState {
    server: Option<bridge::Server>,
    tried_server: bool,
    pub server_error: Option<String>,
    pub bridge: Option<Connected>,
    /// `thread/message` keys.
    delivered: HashSet<String>,
    seen: HashSet<String>,
    render_call: Option<(Call, PathBuf, Instant)>,
    render_queue: VecDeque<(Call, PathBuf)>,
    pub headless: Option<Headless>,
    pub local: Option<LocalRun>,
    llm: Option<LlmServer>,
    conversations: HashMap<PathBuf, Conversation>,
    /// Per thread, the newest message dispatched to a backend.
    marks: HashMap<PathBuf, u32>,
    /// Messages older than this are never dispatched: when the window opened
    /// or the backend was last chosen.
    epoch: u64,
    /// When the person's oldest unanswered message to a connected session
    /// was first seen waiting.
    quiet_since: Option<(String, Instant)>,
}

/// The Collaborate window's header line.
pub struct Light {
    pub state: LightState,
    pub text: String,
    pub busy: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LightState {
    Connected,
    Working,
    Waiting,
    Off,
}

fn key(thread: &Path, message: &str) -> String {
    format!("{}/{message}", thread.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
}

impl App {
    /// Once a frame: the socket, the runs, and dispatch.
    pub fn poll_agent(&mut self) {
        if !self.agent.tried_server {
            self.agent.tried_server = true;
            self.agent.epoch = thread::now();
            if std::env::var_os("FRACTURIZE_NO_SOCKET").is_none() {
                match bridge::Server::start() {
                    Ok(s) => {
                        log::info!("agent socket: {}", s.path.display());
                        self.agent.server = Some(s);
                    }
                    Err(e) => {
                        log::warn!("agent socket: {e}");
                        self.agent.server_error = Some(e);
                    }
                }
            }
        }
        let events = self.agent.server.as_ref().map(|s| s.drain()).unwrap_or_default();
        for e in events {
            match e {
                Event::Call(c) => self.bridge_call(c),
                Event::Gone(conn) => {
                    if self.agent.bridge.as_ref().is_some_and(|b| b.conn == conn) {
                        self.agent.bridge = None;
                    }
                }
            }
        }
        self.poll_renders();
        self.poll_headless();
        self.poll_local();
        self.dispatch();
    }

    // === The socket ===

    fn bridge_call(&mut self, c: Call) {
        match c.method.as_str() {
            "hello" => {
                if let Some(b) = self.agent.bridge.as_ref().filter(|b| b.conn != c.conn) {
                    let msg = format!(
                        "Fracturize already has {} connected (pid {}). One agent at a time: close that session first.",
                        b.name, b.pid
                    );
                    return c.err(msg);
                }
                let name = c.params["name"].as_str().unwrap_or("agent").to_string();
                let pid = c.params["pid"].as_u64().unwrap_or(0);
                self.agent.bridge = Some(Connected { conn: c.conn, name, pid, since: Instant::now() });
                let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
                let thread = self.thread_name();
                c.ok(json!({ "cwd": cwd, "human": self.collab_me(), "thread": thread }));
            }
            "get_state" => {
                let brief = c.params["brief"].as_bool().unwrap_or(false);
                let state = self.agent_state(brief);
                match state {
                    Ok(v) => c.ok(v),
                    Err(e) => c.err(e),
                }
            }
            "get_render" => {
                let path = c.params["path"].as_str().map(PathBuf::from);
                match path {
                    Some(p) => self.agent.render_queue.push_back((c, p)),
                    None => c.err("get_render: say where to write it"),
                }
            }
            "posted" => {
                self.poll_collab(true);
                c.ok(json!({}));
            }
            "delivered" | "seen" => {
                let thread = c.params["thread"].as_str().unwrap_or("").to_string();
                let ids: Vec<String> = match &c.params["messages"] {
                    Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                    _ => c.params["message"].as_str().map(|s| vec![s.to_string()]).unwrap_or_default(),
                };
                let set = if c.method == "seen" { &mut self.agent.seen } else { &mut self.agent.delivered };
                for id in ids {
                    set.insert(format!("{thread}/{id}"));
                }
                c.ok(json!({}));
            }
            m => {
                let m = m.to_string();
                c.err(format!("the window has no method {m}"));
            }
        }
    }

    fn thread_name(&self) -> String {
        self.collab_dir_shown().file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn agent_state(&self, brief: bool) -> Result<Value, String> {
        let selection = self
            .selected_transform()
            .map(|i| self.scene.transform_names.get(i).cloned().flatten().unwrap_or_else(|| i.to_string()));
        let renderer = match self.render_mode {
            super::RenderMode::Points => "points",
            super::RenderMode::Splat => "splat",
            super::RenderMode::Volume => "volume",
        };
        let mut v = json!({
            "scene_path": self.scene_path,
            "thread": self.thread_name(),
            "dirty": self.is_dirty(),
            "selection": selection,
            "renderer": renderer,
            "previewing": self.preview_label(),
        });
        if !brief {
            v["scene_toml"] = json!(self.scene_to_send(false).to_toml_string()?);
            v["live_view"] = json!(crate::camera::CameraOverride::describe(&self.camera));
        }
        Ok(v)
    }

    /// One render at a time, through the screenshot path, and only when no
    /// other screenshot is under way.
    fn poll_renders(&mut self) {
        if let Some((_, _, at)) = &self.agent.render_call
            && at.elapsed() > RENDER_TIMEOUT
            && let Some((c, _, _)) = self.agent.render_call.take()
        {
            c.err("the render didn't come back in time");
        }
        if self.agent.render_call.is_none()
            && self.screenshot_to.is_none()
            && !self.pending_screenshot
            && let Some((c, path)) = self.agent.render_queue.pop_front()
        {
            self.screenshot_to = Some(path.clone());
            self.request_screenshot();
            self.agent.render_call = Some((c, path, Instant::now()));
        }
    }

    /// A screenshot finished. `true` if it was a bridge's.
    pub(super) fn agent_screenshot_done(&mut self, written: Option<&Path>) -> bool {
        let ours = match (&self.agent.render_call, written) {
            (Some((_, p, _)), Some(w)) => p == w,
            (Some(_), None) => true,
            _ => false,
        };
        if !ours {
            return false;
        }
        if let Some((c, p, _)) = self.agent.render_call.take() {
            if written.is_some_and(Path::exists) {
                c.ok(json!({ "path": p }));
            } else {
                c.err("the view couldn't be rendered (are there points yet?)");
            }
        }
        true
    }

    // === What the window shows ===

    /// Who the person's messages go to.
    pub fn collab_to(&self) -> String {
        if let Some(b) = &self.agent.bridge {
            return b.name.clone();
        }
        "agent".to_string()
    }

    pub fn agent_light(&self) -> Light {
        if let Some(r) = &self.agent.local {
            let model = self.prefs.agent.local.model.clone();
            let who = if model.is_empty() { "Local model".to_string() } else { model };
            let text = if r.stopping() { format!("{who}: stopping after this turn") } else { format!("{who}: {}", r.working) };
            return Light { state: LightState::Working, text, busy: true };
        }
        if let Some(h) = &self.agent.headless {
            let s = h.started.elapsed().as_secs();
            return Light { state: LightState::Working, text: format!("Claude (headless) is working, {}:{:02}", s / 60, s % 60), busy: true };
        }
        if let Some(b) = &self.agent.bridge {
            if let Some((_, since)) = &self.agent.quiet_since
                && since.elapsed() >= QUIET
            {
                let m = since.elapsed().as_secs() / 60;
                return Light {
                    state: LightState::Waiting,
                    text: format!("{} is connected, but there's been no reply in {m} min; check its terminal", b.name),
                    busy: false,
                };
            }
            return Light { state: LightState::Connected, text: format!("{} is connected", b.name), busy: false };
        }
        let text = match self.prefs.agent.backend {
            Backend::ClaudeChannel => match &self.agent.server_error {
                Some(e) => format!("No session can connect: {e}"),
                None => "No Claude session connected; messages wait here until one is".to_string(),
            },
            Backend::ClaudeHeadless => "Claude (headless) starts on your next message".to_string(),
            Backend::Local => "The local model starts on your next message".to_string(),
            Backend::None => "Files only: an agent reads the thread with the CLI".to_string(),
        };
        Light { state: LightState::Off, text, busy: false }
    }

    /// A message's state beyond the files: delivered, seen, working.
    pub fn message_state(&self, m: &Message) -> Option<&'static str> {
        let k = key(&self.collab_dir_shown(), &m.id);
        let working = self.agent.local.as_ref().map(|r| (&r.thread, &r.message)).or(self.agent.headless.as_ref().map(|h| (&h.thread, &h.message)));
        if working.is_some_and(|(t, id)| key(t, id) == k) {
            return Some("working on it");
        }
        if self.agent.seen.contains(&k) {
            Some("seen")
        } else if self.agent.delivered.contains(&k) {
            Some("delivered")
        } else {
            None
        }
    }

    pub fn set_agent_backend(&mut self, b: Backend) {
        if self.prefs.agent.backend != b {
            self.prefs.agent.backend = b;
            self.agent.epoch = thread::now();
            self.agent.marks.clear();
            self.save_agent_prefs();
        }
    }

    /// The `[agent]` preferences, for Agent settings. Call
    /// [`Self::save_agent_prefs`] after changing them.
    pub fn agent_prefs_mut(&mut self) -> &mut crate::agent::AgentPrefs {
        &mut self.prefs.agent
    }

    pub fn save_agent_prefs(&mut self) {
        self.prefs_dirty_since = Some(Instant::now());
    }

    /// Stop whatever is running, between turns for a local model.
    pub fn stop_agent(&mut self) {
        if let Some(r) = &self.agent.local {
            r.stop();
        }
        if let Some(mut h) = self.agent.headless.take() {
            h.stop();
            let me = self.collab_me();
            let mut header = Header::new(Kind::Note, "claude", &me, &format!("{me} stopped the headless session."));
            header.replying_to = Some(h.message.clone());
            let scene = self.thread_scene_for(&h.thread);
            let _ = thread::post(&h.thread, &scene, Draft::new(header));
            self.poll_collab(true);
        }
    }

    /// Forget the headless session, so the next message starts a new one.
    pub fn forget_headless_session(&mut self) {
        self.prefs.agent.claude.session_id = None;
        self.save_agent_prefs();
    }

    /// Forget the local model's conversation in this thread.
    pub fn forget_local_conversation(&mut self) {
        let dir = self.collab_dir_shown();
        self.agent.conversations.remove(&dir);
    }

    pub fn has_local_conversation(&self) -> bool {
        self.agent.conversations.contains_key(&self.collab_dir_shown())
    }

    fn thread_scene_for(&self, dir: &Path) -> String {
        Thread::open(dir)
            .ok()
            .map(|t| t.file.scene)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.scene_path.clone().unwrap_or_else(|| format!("({})", self.scene.name)))
    }

    // === Dispatch ===

    fn dispatch(&mut self) {
        // The "check the terminal" clock, for a connected session.
        let me = self.collab_me();
        let oldest = self
            .agent
            .bridge
            .as_ref()
            .and_then(|_| self.collab.thread.as_ref())
            .and_then(|t| t.messages.iter().find(|m| m.header.from == me && t.waiting(m) && !replied_after(t, m, &me)))
            .map(|m| m.id.clone());
        match (oldest, &self.agent.quiet_since) {
            (None, _) => self.agent.quiet_since = None,
            (Some(id), Some((q, _))) if *q == id => {}
            (Some(id), _) => self.agent.quiet_since = Some((id, Instant::now())),
        }

        let backend = self.prefs.agent.backend;
        if !matches!(backend, Backend::ClaudeHeadless | Backend::Local)
            || self.agent.bridge.is_some()
            || self.agent.headless.is_some()
            || self.agent.local.is_some()
            || self.collab.preview.is_some()
        {
            return;
        }
        let Some(t) = self.collab.thread.as_ref() else { return };
        let mark = self.agent.marks.get(&t.dir).copied().unwrap_or(0);
        let epoch = self.agent.epoch;
        let next = t.messages.iter().find(|m| {
            m.number() > mark && m.header.created >= epoch && m.header.from == me && m.withdrawn.is_none() && wants_an_agent(m)
        });
        let Some(m) = next.cloned() else { return };
        let dir = t.dir.clone();
        self.agent.marks.insert(dir.clone(), m.number());
        match backend {
            Backend::ClaudeHeadless => self.start_headless(&dir, &m),
            Backend::Local => self.start_local(&dir, &m),
            _ => {}
        }
    }

    fn start_headless(&mut self, dir: &Path, m: &Message) {
        let Some(t) = self.collab.thread.as_ref() else { return };
        let name = t.name();
        let scene = self.thread_scene_for(dir);
        let prompt = crate::agent::brief(&name, &scene, &thread::render_message(t, m));
        match Headless::start(&self.prefs.agent.claude, "claude", &prompt, dir.to_path_buf(), m.id.clone()) {
            Ok(h) => self.agent.headless = Some(h),
            Err(e) => self.set_notice(format!("Couldn't start Claude: {e}"), Tone::Error),
        }
    }

    fn poll_headless(&mut self) {
        let Some(h) = &mut self.agent.headless else { return };
        let mut finished = None;
        for u in h.updates() {
            match u {
                Update::Session(id) => {
                    if self.prefs.agent.claude.session_id.as_deref() != Some(&id) {
                        self.prefs.agent.claude.session_id = Some(id);
                        self.prefs_dirty_since = Some(Instant::now());
                    }
                }
                Update::Text(t) => {
                    h.said.push_str(&t);
                    h.said.push('\n');
                }
                Update::Done { result, error } => finished = Some((result, error)),
            }
        }
        let Some((result, error)) = finished else { return };
        let Some(h) = self.agent.headless.take() else { return };
        // If it answered through the tools, the thread already says so;
        // otherwise its last words go in as a note, so nothing is silent.
        let trigger = thread::Thread::open(&h.thread).ok().and_then(|t| t.get(&h.message).map(|m| m.number())).unwrap_or(0);
        let me = self.collab_me();
        let answered = Thread::open(&h.thread).is_ok_and(|t| t.messages.iter().any(|m| m.number() > trigger && m.header.from != me));
        if !answered || error {
            let text = if error {
                format!("The headless Claude session ended with an error: {}", if result.is_empty() { "no detail" } else { &result })
            } else if result.trim().is_empty() {
                "Claude finished without saying anything.".to_string()
            } else {
                result
            };
            let mut header = Header::new(Kind::Note, "claude", &me, &text);
            header.replying_to = Some(h.message.clone());
            let scene = self.thread_scene_for(&h.thread);
            if let Err(e) = thread::post(&h.thread, &scene, Draft::new(header)) {
                self.set_notice(format!("Couldn't post Claude's answer: {e}"), Tone::Error);
            }
        }
        self.poll_collab(true);
    }

    fn start_local(&mut self, dir: &Path, m: &Message) {
        let Some(t) = self.collab.thread.as_ref() else { return };
        let trigger = match local_trigger(t, m, self.scene_path.as_deref()) {
            Ok(tr) => tr,
            Err(e) => return self.set_notice(format!("The local model can't take this message: {e}"), Tone::Error),
        };
        let prefs = self.prefs.agent.local.clone();
        if !prefs.preset.is_empty() && self.agent.llm.is_none() {
            match LlmServer::start(&prefs) {
                Ok(s) => self.agent.llm = Some(s),
                Err(e) => return self.set_notice(format!("Couldn't start the local model: {e}"), Tone::Error),
            }
        }
        let renderer = match self.render_mode {
            super::RenderMode::Points => "points",
            super::RenderMode::Splat => "splat",
            super::RenderMode::Volume => "volume",
        };
        let job = agent_local::Job {
            prefs,
            human: self.collab_me(),
            thread: dir.to_path_buf(),
            scene: self.thread_scene_for(dir),
            trigger,
            conversation: self.agent.conversations.remove(dir).unwrap_or_default(),
            renderer: Some(renderer.to_string()),
        };
        self.agent.local = Some(agent_local::start(job));
    }

    fn poll_local(&mut self) {
        // with-llm gave up (most often: not enough free VRAM): say why, once,
        // and stop the run waiting on it.
        if let Some(why) = self.agent.llm.as_mut().and_then(LlmServer::exited) {
            self.agent.llm = None;
            if let Some(r) = &self.agent.local {
                r.stop();
            }
            self.set_notice(format!("The local model's server stopped: {why}"), Tone::Error);
        }
        let Some(r) = &mut self.agent.local else { return };
        let mut done = None;
        let mut rescan = false;
        for s in r.updates() {
            match s {
                Status::Working(w) => r.working = w,
                Status::Posted => rescan = true,
                Status::Done { conversation } => done = Some(conversation),
            }
        }
        if let Some(c) = done
            && let Some(r) = self.agent.local.take()
        {
            if !c.messages.is_empty() {
                self.agent.conversations.insert(r.thread, c);
            }
            rescan = true;
        }
        if rescan {
            self.poll_collab(true);
        }
    }
}

/// Whether a message from the person is one an agent should take up.
fn wants_an_agent(m: &Message) -> bool {
    match m.header.kind {
        Kind::Request | Kind::Proposal => true,
        Kind::Response => m.header.answers.iter().any(|a| a.verdict == Verdict::SentBack),
        Kind::Note => false,
    }
}

/// Whether anyone but `me` has written since `m`.
fn replied_after(t: &Thread, m: &Message, me: &str) -> bool {
    t.messages.iter().any(|x| x.number() > m.number() && x.header.from != me)
}

/// What the local model is given for a message.
fn local_trigger(t: &Thread, m: &Message, scene_path: Option<&str>) -> Result<Trigger, String> {
    let base = Some(m.file("base.toml")).filter(|p| p.is_file());
    match m.header.kind {
        Kind::Request => {
            let scene = base.clone().or_else(|| scene_path.map(PathBuf::from)).ok_or("it carries no scene")?;
            Ok(Trigger::Scene {
                message: m.id.clone(),
                text: m.header.message.clone(),
                scene,
                render: m.header.render.as_ref().map(|r| m.file(r)),
                base,
            })
        }
        Kind::Proposal => {
            let o = m.header.options.first().ok_or("it has no options")?;
            Ok(Trigger::Scene { message: m.id.clone(), text: m.header.message.clone(), scene: m.file(&o.scene), render: o.render.as_ref().map(|r| m.file(r)), base })
        }
        Kind::Response => {
            let a = m.header.answers.iter().find(|a| a.verdict == Verdict::SentBack).ok_or("nothing was sent back")?;
            let p = m.header.replying_to.as_deref().and_then(|r| t.get(r)).ok_or("it doesn't say what it answers")?;
            let o = p.option(&a.option).ok_or_else(|| format!("{} has no option {}", p.id, a.option))?;
            let scene = match &a.tweak {
                Some(tw) => m.file(tw),
                None => p.file(&o.scene),
            };
            Ok(Trigger::SendBack {
                message: m.id.clone(),
                option: a.option.clone(),
                note: a.note.clone(),
                scene,
                tweaked: a.tweak.is_some(),
                base: Some(p.file("base.toml")).filter(|p| p.is_file()),
            })
        }
        Kind::Note => Err("a note wants no answer".into()),
    }
}
