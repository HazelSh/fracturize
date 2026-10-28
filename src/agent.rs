//! Who answers the Collaborate window (AGENT-COLLAB-PLAN.md §4.6, §5.3):
//! the `[agent]` preferences, and the headless Claude backend.
//!
//! Four backends, one setting:
//!
//! - **`claude-channel`** (the default): an interactive Claude Code session
//!   started with the `fracturize` channel. Nothing is spawned here: the
//!   session's `fracturize mcp` connects to the window's socket, messages
//!   reach it as channel events, and it answers through its tools. With no
//!   session connected, messages wait on disk for one.
//! - **`claude-headless`**: per message, the window runs `claude -p` itself,
//!   resuming one saved session so the briefing carries over, with the same
//!   bridge reached through the same socket. A cold start per message, and
//!   no terminal needed. Also the way round it if channels change shape.
//! - **`local`**: a model on llama-server, driven from the window
//!   (`agent_local.rs`).
//! - **`none`**: the thread is files only, for an agent that reads it with
//!   the CLI.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    #[default]
    ClaudeChannel,
    ClaudeHeadless,
    Local,
    None,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::ClaudeChannel => "Claude session",
            Backend::ClaudeHeadless => "Claude, headless",
            Backend::Local => "Local model",
            Backend::None => "Files only",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClaudePrefs {
    /// The session's display name, and what the connect hint tells you to
    /// start.
    pub session_name: String,
    /// Extra arguments for `claude -p`.
    pub headless_args: Vec<String>,
    /// The headless session to resume. Set by the first headless request;
    /// cleared from Agent settings to start afresh.
    pub session_id: Option<String>,
}

impl Default for ClaudePrefs {
    fn default() -> Self {
        Self { session_name: "fracturize".into(), headless_args: Vec::new(), session_id: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Vision {
    /// What the server says (llama-server's `/props`).
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalMode {
    /// Prose and one fenced scene a reply: `tools/llm-author`'s protocol.
    #[default]
    WholeScene,
    /// The MCP tools as function calls (plan §6.1). Not built yet; the
    /// setting is kept so a file that names it still loads.
    Tools,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalPrefs {
    pub base_url: String,
    /// Empty: whatever `/v1/models` reports.
    pub model: String,
    /// The with-llm preset to start when nothing answers at `base_url`;
    /// empty, never start one.
    pub preset: String,
    pub vision: Vision,
    pub thinking: bool,
    /// Constrain replies with `tools/llm-author/scene.gbnf`.
    pub grammar: bool,
    pub mode: LocalMode,
    /// Replies a request gets before the model is asked to report back.
    pub max_turns: u32,
    /// Where with-llm is. Empty: on `PATH`, else `~/code/lll/with-llm`.
    pub with_llm: String,
}

impl Default for LocalPrefs {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080/v1".into(),
            model: String::new(),
            preset: "gemma4-e4b".into(),
            vision: Vision::Auto,
            thinking: true,
            grammar: false,
            mode: LocalMode::WholeScene,
            max_turns: 6,
            with_llm: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentPrefs {
    pub backend: Backend,
    pub claude: ClaudePrefs,
    pub local: LocalPrefs,
}

/// What a backend is told about a message: the message as `--thread`
/// prints it, and how to answer.
pub fn brief(thread_name: &str, scene: &str, message: &str) -> String {
    format!(
        "A message arrived for you in the Fracturize thread `{thread_name}` (scene {scene}). \
         Answer it with the fracturize MCP tools (propose, respond, reply), passing \
         thread = \"{thread_name}\"; use the fracturize CLI for anything else. Read \
         AGENTS.md and CRAFT.md first if you haven't this session.\n\n{message}"
    )
}

// ------------------------------------------------------------- headless

/// What a headless run says as it goes.
pub enum Update {
    Session(String),
    Text(String),
    Done { result: String, error: bool },
}

pub struct Headless {
    child: Child,
    rx: Receiver<Update>,
    pub started: Instant,
    /// The thread and message it was started for.
    pub thread: PathBuf,
    pub message: String,
    /// The text it has said so far.
    pub said: String,
}

impl Headless {
    /// Start `claude -p` on one message. The MCP config is written fresh
    /// each time: this binary's `mcp`, named as `agent_name` in the thread.
    pub fn start(prefs: &ClaudePrefs, agent_name: &str, prompt: &str, thread: PathBuf, message: String) -> Result<Headless, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let config = json!({ "mcpServers": { "fracturize": {
            "command": exe, "args": ["mcp"], "env": { "FRACTURIZE_AGENT": agent_name },
        } } });
        let config_path = std::env::temp_dir().join(format!("fracturize-headless-{}.json", std::process::id()));
        std::fs::write(&config_path, config.to_string()).map_err(|e| format!("{}: {e}", config_path.display()))?;
        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "auto", "--strict-mcp-config"])
            .arg("--mcp-config")
            .arg(&config_path);
        match &prefs.session_id {
            Some(id) => {
                cmd.args(["--resume", id]);
            }
            None => {
                cmd.args(["-n", &prefs.session_name]);
            }
        }
        cmd.args(&prefs.headless_args).arg(prompt);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("couldn't start claude: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("headless-claude".into())
            .spawn(move || {
                let mut done = false;
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if let Some(id) = v["session_id"].as_str().filter(|_| v["type"] == "system") {
                        let _ = tx.send(Update::Session(id.to_string()));
                    }
                    if v["type"] == "assistant" {
                        for c in v["message"]["content"].as_array().into_iter().flatten() {
                            if c["type"] == "text"
                                && let Some(t) = c["text"].as_str()
                            {
                                let _ = tx.send(Update::Text(t.to_string()));
                            }
                        }
                    }
                    if v["type"] == "result" {
                        done = true;
                        if let Some(id) = v["session_id"].as_str() {
                            let _ = tx.send(Update::Session(id.to_string()));
                        }
                        let result = v["result"].as_str().unwrap_or("").to_string();
                        let error = v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success";
                        let _ = tx.send(Update::Done { result, error });
                    }
                }
                if !done {
                    let mut err = String::new();
                    if let Some(s) = stderr {
                        for l in BufReader::new(s).lines().map_while(Result::ok) {
                            err = l;
                        }
                    }
                    let msg = if err.is_empty() { "claude exited without a result".to_string() } else { format!("claude: {err}") };
                    let _ = tx.send(Update::Done { result: msg, error: true });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Headless { child, rx, started: Instant::now(), thread, message, said: String::new() })
    }

    pub fn updates(&self) -> Vec<Update> {
        self.rx.try_iter().collect()
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.stop();
        }
    }
}

// ------------------------------------------------------------ with-llm

/// Where with-llm is.
pub fn with_llm_path(prefs: &LocalPrefs) -> PathBuf {
    if !prefs.with_llm.trim().is_empty() {
        return PathBuf::from(prefs.with_llm.trim());
    }
    if let Some(p) = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("with-llm"))
        .find(|p| p.is_file())
    {
        return p;
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("code/lll/with-llm")
}

/// Whether a preset fits in the GPU's free memory, by with-llm's own rule
/// (`with-llm --fits`): `Ok((need, free))` in MiB, `free` 0 when it can't
/// tell.
pub fn fits(prefs: &LocalPrefs) -> Result<(u64, u64), String> {
    let out = Command::new(with_llm_path(prefs))
        .args(["--fits", "-m", &prefs.preset])
        .output()
        .map_err(|e| format!("with-llm: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "need 4321 free 6000" (free is "unknown" without nvidia-smi)
    let mut words = text.split_whitespace();
    let mut need = None;
    let mut free = None;
    while let Some(w) = words.next() {
        match w {
            "need" => need = words.next().and_then(|n| n.parse().ok()),
            "free" => free = words.next().map(|n| n.parse().unwrap_or(0)),
            _ => {}
        }
    }
    match need {
        Some(n) => Ok((n, free.unwrap_or(0))),
        None => Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("with-llm: ").to_string()),
    }
}

/// A llama-server the window started, through with-llm, held up for as
/// long as the window wants it. with-llm runs `cat`, which exits when the
/// window closes the pipe (or dies), and then stops the server itself, but
/// only if nothing else is using it.
pub struct LlmServer {
    child: Child,
    pub log: PathBuf,
}

impl LlmServer {
    pub fn start(prefs: &LocalPrefs) -> Result<LlmServer, String> {
        let port = port_of(&prefs.base_url);
        let log = std::env::temp_dir().join(format!("fracturize-with-llm-{}.log", std::process::id()));
        let logf = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let child = Command::new(with_llm_path(prefs))
            .args(["-m", &prefs.preset, "-p", &port.to_string(), "--", "cat"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(logf)
            .spawn()
            .map_err(|e| format!("with-llm: {e}"))?;
        Ok(LlmServer { child, log })
    }

    /// `Some(why)` once with-llm has given up (or the server ended).
    pub fn exited(&mut self) -> Option<String> {
        self.child.try_wait().ok().flatten().map(|_| {
            let log = std::fs::read_to_string(&self.log).unwrap_or_default();
            log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("with-llm exited").trim_start_matches("with-llm: ").to_string()
        })
    }
}

impl Drop for LlmServer {
    fn drop(&mut self) {
        // Closing the pipe ends `cat`, and with-llm's trap stops the server.
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

fn port_of(url: &str) -> u16 {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|host| host.rsplit_once(':'))
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(8080)
}

/// The prompt files local models get, shared with `tools/llm-author/` so
/// the script and the window can't drift apart.
pub fn llm_author_dir() -> PathBuf {
    Path::new("tools").join("llm-author")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_and_take_defaults() {
        let p: AgentPrefs = toml::from_str("backend = \"local\"\n[local]\nmax_turns = 4\n").unwrap();
        assert_eq!(p.backend, Backend::Local);
        assert_eq!(p.local.max_turns, 4);
        assert_eq!(p.local.preset, "gemma4-e4b");
        let back: AgentPrefs = toml::from_str(&toml::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn the_port_comes_from_the_url() {
        assert_eq!(port_of("http://127.0.0.1:8080/v1"), 8080);
        assert_eq!(port_of("http://localhost:9001"), 9001);
        assert_eq!(port_of("garbage"), 8080);
    }
}
