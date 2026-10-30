//! The local-model backend (AGENT-COLLAB-PLAN.md §6): a model on
//! llama-server answering the person's messages, driven from the window.
//!
//! **This is `tools/llm-author/author.py`'s protocol, moved into the
//! window.** The model gets the system prompt and `primer.md`, then the
//! person's message, the scene, the program's `--info` report and (if it can
//! see) a render, and replies with prose and one fenced scene, or `DONE`, or
//! `STOP`. The prompts are read from `tools/llm-author/`, never copied, so
//! the script and the window can't drift apart; a missing one is an error
//! said in the thread, not a quiet fallback.
//!
//! **Each scene it sends is a card at once** (§6.2): a proposal answering
//! the message, so the person can preview or apply turn 2 while turns 3 to 6
//! are still being written.
//!
//! **A turn limit, then a report.** At most `max_turns` replies a message.
//! On reaching it the model is asked one last, optional question (what it
//! tried, where it stands, what it would do next), and the answer goes in the
//! thread as a note. Carrying on is the person's choice, by replying, and a
//! reply starts a fresh budget, in the same conversation.
//!
//! **The llm-author welfare rules, unchanged** (§6.3):
//! - `DONE` or `STOP` on a line of its own ends the request at once. `STOP`
//!   needs no reason, is never called a failure, and isn't followed by any
//!   question.
//! - A load error is "common and easy to fix", with `STOP` offered again;
//!   after two failed fixes in one turn the request ends.
//! - A reply that fills its whole budget without a scene ends the request
//!   (it's most often a loop, and asking again feeds it more of the same).
//! - The same scene again is asked about once; a second time is taken as the
//!   `DONE` it means.
//! - One request at a time; the window's Stop ends it between turns.
//! - The whole transcript, reasoning included, is written in llm-author's
//!   format under the thread's `transcripts/`, so `review.sh` reads it as is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agent::{LocalPrefs, Vision};
use crate::thread::{self, Draft, Header, Kind};

/// What started a request.
#[derive(Clone, Debug)]
pub enum Trigger {
    /// A request, or the person's own proposal: their words and a scene.
    Scene { message: String, text: String, scene: PathBuf, render: Option<PathBuf>, base: Option<PathBuf> },
    /// One of the model's versions sent back with a note, and the person's
    /// tweak if they made one.
    SendBack { message: String, option: String, note: String, scene: PathBuf, tweaked: bool, base: Option<PathBuf> },
}

impl Trigger {
    fn message(&self) -> &str {
        match self {
            Trigger::Scene { message, .. } | Trigger::SendBack { message, .. } => message,
        }
    }

    fn base(&self) -> Option<&Path> {
        match self {
            Trigger::Scene { base, .. } | Trigger::SendBack { base, .. } => base.as_deref(),
        }
    }
}

/// The conversation so far with one thread's model, kept between requests
/// so a reply carries on rather than starting over.
#[derive(Clone, Debug, Default)]
pub struct Conversation {
    pub messages: Vec<Value>,
}

/// What a run tells the window.
pub enum Status {
    /// What it's doing now, for the window's working line.
    Working(String),
    /// It posted a message.
    Posted,
    Done { conversation: Conversation },
}

pub struct LocalRun {
    stop: Arc<AtomicBool>,
    rx: Receiver<Status>,
    pub thread: PathBuf,
    pub message: String,
    pub working: String,
}

impl LocalRun {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn updates(&self) -> Vec<Status> {
        self.rx.try_iter().collect()
    }
}

pub struct Job {
    pub prefs: LocalPrefs,
    /// Who the model is answering.
    pub human: String,
    pub thread: PathBuf,
    /// The scene path the thread belongs to (for a thread not started yet).
    pub scene: String,
    pub trigger: Trigger,
    pub conversation: Conversation,
    /// The window's renderer, for the cards.
    pub renderer: Option<String>,
}

pub fn start(job: Job) -> LocalRun {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let run = LocalRun {
        stop: stop.clone(),
        rx,
        thread: job.thread.clone(),
        message: job.trigger.message().to_string(),
        working: "starting".into(),
    };
    std::thread::Builder::new()
        .name("local-llm".into())
        .spawn(move || {
            let mut r = Runner::new(job, stop, tx.clone());
            let conversation = r.run();
            let _ = tx.send(Status::Done { conversation });
        })
        .expect("spawn the local-model thread");
    run
}

// ------------------------------------------------------------ the runner

struct Facts {
    model: String,
    vision: bool,
    n_ctx: Option<u64>,
}

struct Runner {
    job: Job,
    stop: Arc<AtomicBool>,
    tx: Sender<Status>,
    http: ureq::Agent,
    /// `http://host:port`, without `/v1`.
    root: String,
    exe: PathBuf,
    out: PathBuf,
    notes: Vec<String>,
    model: String,
}

impl Runner {
    fn new(job: Job, stop: Arc<AtomicBool>, tx: Sender<Status>) -> Runner {
        let base = job.prefs.base_url.trim_end_matches('/');
        let root = base.strip_suffix("/v1").unwrap_or(base).to_string();
        let http = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(900)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fracturize"));
        Runner { job, stop, tx, http, root, exe, out: PathBuf::new(), notes: Vec::new(), model: String::new() }
    }

    fn working(&self, s: impl Into<String>) {
        let _ = self.tx.send(Status::Working(s.into()));
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// The whole request. Returns the conversation, to carry on from.
    fn run(&mut self) -> Conversation {
        let mut convo = std::mem::take(&mut self.job.conversation);
        let facts = match self.wait_for_server() {
            Ok(f) => f,
            Err(e) => {
                self.note(&format!("The local model couldn't start on this: {e}"), None);
                return convo;
            }
        };
        self.model = facts.model.clone();
        let vision = match self.job.prefs.vision {
            Vision::Auto => facts.vision,
            Vision::On => true,
            Vision::Off => false,
        };
        let stamp = stamp();
        self.out = self.job.thread.join("transcripts").join(format!("{stamp}-{}", sanitize(&facts.model)));
        if let Err(e) = std::fs::create_dir_all(&self.out) {
            self.note(&format!("Couldn't write the transcript ({e}), so the request didn't start."), None);
            return convo;
        }
        let mut t = Transcript::new(&self.out);
        match self.converse(&mut convo.messages, &mut t, &facts, vision) {
            Ok(ending) => t.summary(&self.model, &ending, &self.notes, &self.job.trigger),
            Err(e) => {
                t.summary(&self.model, &format!("error: {e}"), &self.notes, &self.job.trigger);
                self.note(&format!("The request ended: {e}"), None);
            }
        }
        convo
    }

    fn wait_for_server(&self) -> Result<Facts, String> {
        let start = Instant::now();
        let mut said = false;
        loop {
            if self.stopped() {
                return Err("stopped".into());
            }
            if self.get(&format!("{}/health", self.root)).is_ok_and(|v| v["status"] == "ok") {
                break;
            }
            if self.job.prefs.preset.is_empty() || start.elapsed() > Duration::from_secs(300) {
                return Err(format!("nothing answers at {}", self.job.prefs.base_url));
            }
            if !said {
                self.working(format!("starting {}", self.job.prefs.preset));
                said = true;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let mut f = Facts { model: self.job.prefs.model.clone(), vision: false, n_ctx: None };
        if f.model.is_empty() {
            f.model = self
                .get(&format!("{}/v1/models", self.root))
                .ok()
                .and_then(|v| v["data"][0]["id"].as_str().map(str::to_string))
                .unwrap_or_else(|| "local-model".into());
        }
        if let Ok(p) = self.get(&format!("{}/props", self.root)) {
            f.vision = p["modalities"]["vision"].as_bool().unwrap_or(false);
            f.n_ctx = p["default_generation_settings"]["n_ctx"].as_u64();
        }
        Ok(f)
    }

    fn get(&self, url: &str) -> Result<Value, String> {
        let mut r = self.http.get(url).call().map_err(|e| e.to_string())?;
        r.body_mut().read_json::<Value>().map_err(|e| e.to_string())
    }

    fn ask(&self, convo: &[Value], t: &mut Transcript, free_text: bool) -> Result<Reply, String> {
        let p = &self.job.prefs;
        let mut body = json!({
            "model": self.model,
            "messages": convo,
            "max_tokens": 8192,
            "chat_template_kwargs": { "enable_thinking": p.thinking },
        });
        if p.grammar && !free_text {
            body["grammar"] = Value::String(load_grammar()?);
        }
        let mut resp = self
            .http
            .post(&format!("{}/v1/chat/completions", self.root))
            .header("Authorization", "Bearer sk-local")
            .send_json(&body)
            .map_err(|e| format!("the model's server: {e}"))?;
        let status = resp.status();
        let v: Value = resp.body_mut().read_json().map_err(|e| format!("the model's server sent something odd: {e}"))?;
        if !status.is_success() {
            return Err(format!("the model's server said {status}: {}", v["error"]["message"].as_str().unwrap_or("")));
        }
        let choice = &v["choices"][0];
        let reply = Reply {
            content: choice["message"]["content"].as_str().unwrap_or("").to_string(),
            reasoning: choice["message"]["reasoning_content"].as_str().unwrap_or("").to_string(),
            finish: choice["finish_reason"].as_str().unwrap_or("").to_string(),
            tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0) + v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        };
        t.record("assistant", &json!(reply.content), &reply.reasoning, json!({ "finish_reason": reply.finish, "usage": v["usage"] }));
        Ok(reply)
    }

    fn converse(&mut self, convo: &mut Vec<Value>, t: &mut Transcript, facts: &Facts, vision: bool) -> Result<String, String> {
        let p = self.job.prefs.clone();
        let turns = p.max_turns.max(1);
        if convo.is_empty() {
            let system = fill(
                "system.md",
                &[
                    ("model", &self.model),
                    ("author", &self.model),
                    ("rounds", &turns.to_string()),
                    ("image_clause", if vision { " and a picture of four views around it" } else { "" }),
                    ("primer", &read_prompt_file("primer.md")?),
                ],
            )?;
            t.record("system", &json!(system), "", Value::Null);
            convo.push(json!({ "role": "system", "content": system }));
        }
        let opening = self.opening(vision)?;
        self.say(convo, t, opening);

        let mut last_good: Option<PathBuf> = None;
        let mut last_proposal: Option<(String, String)> = None;
        let (mut turn, mut repairs, mut repeats) = (1u32, 0u32, 0u32);
        let ending = loop {
            if self.stopped() {
                self.note(&format!("{} stopped this request.", self.job.human), None);
                break "stopped by the person".to_string();
            }
            if turn > turns {
                break "ran out of turns".to_string();
            }
            self.working(format!("turn {turn} of {turns}: writing"));
            let reply = self.ask(convo, t, false)?;
            convo.push(json!({ "role": "assistant", "content": reply.content }));
            for line in reply.content.lines() {
                if let Some(n) = line.trim().strip_prefix("NOTE:").or_else(|| line.trim().strip_prefix("Note:")) {
                    self.notes.push(n.trim().to_string());
                }
            }
            let blocks = toml_blocks(&reply.content);
            if blocks.is_empty() && has_line(&reply.content, "STOP") {
                // No reason asked for, and no question after.
                self.note(&format!("{} chose to stop here.", self.model), None);
                return Ok("the model chose STOP".into());
            }
            if blocks.is_empty() && has_line(&reply.content, "DONE") {
                self.done_note(&reply.content);
                return Ok("the model said DONE".into());
            }
            if blocks.is_empty() && reply.finish == "length" {
                self.note(&format!("{}'s reply ran out of room before it reached a scene, so the request ends here.", self.model), None);
                return Ok("a reply ran out of room before any scene".into());
            }
            let mut problem = if blocks.is_empty() { Some("I couldn't find a ```toml block in your reply.".to_string()) } else { None };
            let path = self.out.join(format!("round-{turn}.toml"));
            let mut report = String::new();
            if let Some(b) = blocks.last() {
                std::fs::write(&path, format!("{}\n", b.trim_end())).map_err(|e| e.to_string())?;
                match info_report(&self.exe, &path) {
                    Ok(r) => report = r,
                    Err(e) => {
                        problem = Some(format!("The scene didn't load. The program said:\n\n```\n{}\n```", tail(&e, 1200)))
                    }
                }
            }
            if let Some(problem) = problem {
                repairs += 1;
                if repairs > 2 {
                    self.note("Too many format problems in one turn, so the request ends here.", None);
                    return Ok("too many format problems in one round".into());
                }
                let text = fill("repair.md", &[("problem", &problem)])?;
                self.say(convo, t, json!(text));
                continue;
            }
            repairs = 0;
            let done = has_line(&reply.content, "DONE");
            if !done && last_good.as_ref().is_some_and(|l| same_scene(l, &path)) {
                let _ = std::fs::remove_file(&path);
                if repeats > 0 {
                    self.note(&format!("{} sent the same scene again; taken as done.", self.model), None);
                    return Ok("the same scene again after the reminder; taken as DONE".into());
                }
                repeats += 1;
                if turn == turns {
                    break "ran out of turns".to_string();
                }
                let prev = last_good.as_ref().and_then(|l| l.file_stem()).map_or(String::new(), |s| s.to_string_lossy().replace("round-", ""));
                let text = fill("same.md", &[("prev", &prev), ("left", &(turns - turn).to_string())])?;
                self.say(convo, t, json!(text));
                turn += 1;
                continue;
            }
            repeats = 0;
            last_good = Some(path.clone());

            // The card first, so the person has it while the model looks.
            self.working(format!("turn {turn} of {turns}: rendering"));
            let id = format!("v{turn}");
            match self.post_option(&path, &id, &reply.content, last_proposal.as_ref()) {
                Ok(m) => last_proposal = Some((m, id.clone())),
                Err(e) => self.note(&format!("Couldn't post version {id}: {e}"), None),
            }
            if done {
                self.done_note(&reply.content);
                return Ok("the model said DONE with this version".into());
            }
            if has_line(&reply.content, "STOP") {
                self.note(&format!("{} chose to stop here.", self.model), None);
                return Ok("the model chose STOP".into());
            }
            if turn == turns {
                break "ran out of turns".to_string();
            }
            if facts.n_ctx.is_some_and(|n| reply.tokens as f64 > 0.8 * n as f64) {
                self.note(&format!("{}'s context is nearly full, so the request ends here.", self.model), None);
                return Ok("context nearly full".into());
            }
            let png = self.out.join(format!("round-{turn}.png"));
            match render_views(&self.exe, &path, &png) {
                Ok(stats) => report.push_str(&format!("\n\n{stats}")),
                Err(e) => report.push_str(&format!("\n\nThe render failed:\n{e}")),
            }
            let image_note = if vision && png.exists() { "\nThe picture shows four views from around the scene.\n" } else { "" };
            let text = fill(
                "round.md",
                &[("round", &turn.to_string()), ("report", &report), ("left", &(turns - turn).to_string()), ("image_note", image_note)],
            )?;
            self.say(convo, t, with_image(&text, Some(&png), vision));
            forget_old_images(convo);
            turn += 1;
        };
        if ending == "ran out of turns" && !self.stopped() {
            self.working("reporting back");
            let text = fill("report.md", &[("who", &self.job.human)])?;
            self.say(convo, t, json!(text));
            match self.ask(convo, t, true) {
                Ok(r) => {
                    convo.push(json!({ "role": "assistant", "content": r.content }));
                    let answer = strip_blocks(&r.content);
                    let answer = answer.trim();
                    if answer.is_empty() || has_line(answer, "STOP") {
                        self.note(&format!("{} used its {turns} turns.", self.model), None);
                    } else {
                        self.note(&format!("{} used its {turns} turns. Its report:\n\n{answer}", self.model), None);
                    }
                }
                Err(e) => self.note(&format!("{} used its {turns} turns (the report didn't come: {e}).", self.model), None),
            }
        }
        Ok(ending)
    }

    /// The first message of this request.
    fn opening(&self, vision: bool) -> Result<Value, String> {
        match &self.job.trigger {
            Trigger::Scene { text, scene, render, .. } => {
                let src = std::fs::read_to_string(scene).map_err(|e| format!("{}: {e}", scene.display()))?;
                let info = info_report(&self.exe, scene).unwrap_or_else(|e| e);
                let mut msg = fill("tweak.md", &[("scene", src.trim()), ("info", &info), ("scene_name", &scene_name(&src))])?;
                if !text.trim().is_empty() {
                    msg.push_str(&fill("request.md", &[("request", text.trim())])?);
                }
                let has_picture = vision && render.as_ref().is_some_and(|p| p.exists());
                if has_picture {
                    msg.push_str("\n\nThe picture is the view they were looking at.\n");
                }
                Ok(with_image(&msg, render.as_deref(), vision))
            }
            Trigger::SendBack { option, note, scene, tweaked, .. } => {
                let src = std::fs::read_to_string(scene).map_err(|e| format!("{}: {e}", scene.display()))?;
                let info = info_report(&self.exe, scene).unwrap_or_else(|e| e);
                let tweak_clause = if *tweaked {
                    "They changed it themselves first; this is their version:\n\n".to_string()
                } else {
                    "This is the version they sent back:\n\n".to_string()
                };
                let note = if note.trim().is_empty() { "(no note)" } else { note.trim() };
                Ok(json!(fill(
                    "sendback.md",
                    &[("who", &self.job.human), ("option", option), ("note", note), ("tweak_clause", &tweak_clause), ("scene", src.trim()), ("info", &info)],
                )?))
            }
        }
    }

    fn say(&self, convo: &mut Vec<Value>, t: &mut Transcript, content: Value) {
        t.record("user", &content, "", Value::Null);
        convo.push(json!({ "role": "user", "content": content }));
    }

    /// Post a version as a proposal of one option, answering the message.
    fn post_option(&self, scene: &Path, id: &str, reply: &str, previous: Option<&(String, String)>) -> Result<String, String> {
        let stage = self.out.join(format!(".card-{id}"));
        std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        let name = format!("{id}.toml");
        std::fs::copy(scene, stage.join(&name)).map_err(|e| e.to_string())?;
        let base = match self.job.trigger.base() {
            Some(b) => b.to_path_buf(),
            None => scene.to_path_buf(),
        };
        std::fs::copy(&base, stage.join("base.toml")).map_err(|e| format!("{}: {e}", base.display()))?;
        let png = format!("{id}.png");
        let render = crate::mcp::render_option(&stage.join(&name), &stage.join(&png), self.job.renderer.as_deref()).ok().map(|()| png);
        let prose = strip_blocks(reply);
        let note = self.notes.last().cloned().unwrap_or_default();
        let mut h = Header::new(Kind::Proposal, &self.model, &self.job.human, prose.trim());
        h.replying_to = Some(self.job.trigger.message().to_string());
        if let Some((msg, opt)) = previous {
            h.revises = Some(format!("{msg}/{opt}"));
        }
        h.options.push(thread::Opt { id: id.to_string(), scene: name, render, note: truncate(&note, 80) });
        std::fs::write(stage.join("message.toml"), toml::to_string(&h).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let draft = thread::proposal_from_dir(&stage, &self.model, &self.job.human);
        let result = draft.and_then(|d| thread::post(&self.job.thread, &self.job.scene, d));
        let _ = std::fs::remove_dir_all(&stage);
        let posted = result?;
        let _ = self.tx.send(Status::Posted);
        Ok(posted)
    }

    fn done_note(&self, reply: &str) {
        let prose = strip_blocks(reply);
        let prose = prose.lines().filter(|l| !is_word_line(l, "DONE")).collect::<Vec<_>>().join("\n");
        let prose = prose.trim();
        if prose.is_empty() {
            self.note(&format!("{} is done.", self.model), None);
        } else {
            self.note(&format!("{} is done:\n\n{prose}", self.model), None);
        }
    }

    fn note(&self, text: &str, replying_to: Option<&str>) {
        let from = if self.model.is_empty() { "local model" } else { &self.model };
        let mut h = Header::new(Kind::Note, from, &self.job.human, text);
        h.replying_to = Some(replying_to.unwrap_or(self.job.trigger.message()).to_string());
        match thread::post(&self.job.thread, &self.job.scene, Draft::new(h)) {
            Ok(_) => {
                let _ = self.tx.send(Status::Posted);
            }
            Err(e) => log::warn!("local model: couldn't post a note: {e}"),
        }
    }
}

struct Reply {
    content: String,
    reasoning: String,
    finish: String,
    tokens: u64,
}

// ------------------------------------------------------------ transcripts

/// llm-author's run files: `transcript.jsonl`, `transcript.md`,
/// `summary.md`. `review.sh` reads `transcript.md`.
struct Transcript {
    dir: PathBuf,
    jsonl: Option<std::fs::File>,
    md: Option<std::fs::File>,
}

impl Transcript {
    fn new(dir: &Path) -> Transcript {
        let open = |n: &str| std::fs::OpenOptions::new().create(true).append(true).open(dir.join(n)).ok();
        Transcript { dir: dir.to_path_buf(), jsonl: open("transcript.jsonl"), md: open("transcript.md") }
    }

    fn record(&mut self, role: &str, content: &Value, reasoning: &str, extra: Value) {
        use std::io::Write;
        let mut entry = json!({ "role": role, "content": content, "time": iso_now() });
        if !reasoning.is_empty() {
            entry["reasoning"] = json!(reasoning);
        }
        if let Value::Object(m) = extra {
            for (k, v) in m {
                entry[k] = v;
            }
        }
        if let Some(f) = &mut self.jsonl {
            let _ = writeln!(f, "{entry}");
        }
        let text = match content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts.iter().map(|p| p["text"].as_str().unwrap_or("[image]").to_string()).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        if let Some(f) = &mut self.md {
            let _ = write!(f, "\n## {role}\n\n");
            if !reasoning.is_empty() {
                let _ = write!(f, "<details><summary>reasoning</summary>\n\n{reasoning}\n\n</details>\n\n");
            }
            let _ = writeln!(f, "{text}");
        }
    }

    fn summary(&self, model: &str, ending: &str, notes: &[String], trigger: &Trigger) {
        let what = match trigger {
            Trigger::Scene { text, .. } => format!("- request: {}", text.trim()),
            Trigger::SendBack { option, note, .. } => format!("- sent back: {option}: {}", note.trim()),
        };
        let mut s = vec![format!("# collaborate by {model}"), String::new(), format!("- ended: {ending}"), what];
        if !notes.is_empty() {
            s.extend(["".into(), "## Notes from the model".into(), "".into()]);
            s.extend(notes.iter().map(|n| format!("> {n}")));
        }
        if ending == "the model chose STOP" {
            s.extend(["".into(), "(No report asked for: the model chose STOP, and that is respected.)".into()]);
        }
        let _ = std::fs::write(self.dir.join("summary.md"), s.join("\n") + "\n");
    }
}

// ------------------------------------------------------------- helpers

fn read_prompt_file(name: &str) -> Result<String, String> {
    let dir = crate::agent::llm_author_dir();
    let path = if name == "primer.md" || name == "scene.gbnf" { dir.join(name) } else { dir.join("prompts").join(name) };
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e} (the local model's prompts live in tools/llm-author/)", path.display()))
}

/// A prompt file with `{key}`s filled, as author.py's `fill` does.
fn fill(name: &str, kw: &[(&str, &str)]) -> Result<String, String> {
    let mut text = read_prompt_file(name)?;
    for (k, v) in kw {
        text = text.replace(&format!("{{{k}}}"), v);
    }
    Ok(text)
}

/// author.py's `load_grammar`: DONE/STOP, or prose then one fenced scene.
fn load_grammar() -> Result<String, String> {
    let scene = read_prompt_file("scene.gbnf")?;
    let mut lines = scene.lines();
    let mut out = String::new();
    let mut dropped = false;
    for l in lines.by_ref() {
        if !dropped && l.starts_with("root ::=") {
            dropped = true;
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    let reply = "root ::= finish | prose \"```toml\\n\" scene \"```\" tail\n\
                 finish ::= ( \"DONE\" | \"STOP\" ) tail\n\
                 prose ::= [^`]*\n\
                 tail ::= [^`]*\n";
    Ok(format!("{reply}{out}"))
}

/// The bodies of every ```toml block.
fn toml_blocks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("```toml") {
        let after = &rest[i + 7..];
        let Some(nl) = after.find('\n') else { break };
        if !after[..nl].trim().is_empty() {
            rest = &after[nl..];
            continue;
        }
        let body = &after[nl + 1..];
        let Some(end) = body.find("```") else { break };
        out.push(body[..end].to_string());
        rest = &body[end + 3..];
    }
    out
}

/// The text with fenced blocks taken out (and any `<think>` left in it).
fn strip_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for l in text.lines() {
        if l.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

fn is_word_line(line: &str, word: &str) -> bool {
    let t = line.trim().trim_matches('*').trim_end_matches(['.', '!']).trim_matches('*').trim();
    t == word
}

/// author.py's `has_line`: the word alone on a line, bold or not.
fn has_line(text: &str, word: &str) -> bool {
    text.lines().any(|l| is_word_line(l, word))
}

fn same_scene(a: &Path, b: &Path) -> bool {
    let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let (ta, tb) = (read(a), read(b));
    match (ta.parse::<toml::Table>(), tb.parse::<toml::Table>()) {
        (Ok(x), Ok(y)) => x == y,
        _ => ta.split_whitespace().eq(tb.split_whitespace()),
    }
}

fn scene_name(src: &str) -> String {
    src.parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("meta").and_then(|m| m.get("name")).and_then(|n| n.as_str().map(str::to_string)))
        .unwrap_or_else(|| "the scene".into())
}

/// The parts of `--info` a model can act on: notes and shape, with the
/// CLI's `-S a.b=c` said as the TOML the model is editing. Err: the load
/// error.
fn info_report(exe: &Path, scene: &Path) -> Result<String, String> {
    let out = std::process::Command::new(exe).arg("-s").arg(scene).arg("--info").output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        let name = scene.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = if err.trim().is_empty() { stdout.to_string() } else { err.to_string() };
        return Err(text.trim().replace(&scene.display().to_string(), &name));
    }
    let blocks: Vec<&str> = stdout
        .trim()
        .split("\n\n")
        .filter(|b| matches!(b.split_whitespace().next(), Some("notes" | "shape")))
        .collect();
    Ok(set_flags_as_toml(&blocks.join("\n\n")))
}

/// `-S meta.haze=0.3` → `[meta] haze = 0.3`, as author.py rewrites it.
fn set_flags_as_toml(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("-S ") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 3..];
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let tok = &after[..end];
        let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
        match tok.split_once('=').and_then(|(path, v)| path.split_once('.').map(|(a, b)| (a, b, v))) {
            Some((a, b, v)) if word(a) && word(b) && !v.is_empty() => out.push_str(&format!("[{a}] {b} = {v}")),
            _ => out.push_str(&rest[i..i + 3 + end]),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Four views from around the scene in one small image, and `--stats`.
fn render_views(exe: &Path, scene: &Path, png: &Path) -> Result<String, String> {
    let out = std::process::Command::new(exe)
        .arg("-s")
        .arg(scene)
        .arg("-r")
        .arg(png)
        .args(["--splat", "--effort", "small", "--orbit-grid", "2x2", "--width", "320", "--height", "320", "--no-labels"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() || !png.exists() {
        let mut e = String::from_utf8_lossy(&out.stderr).to_string();
        e.push_str(&String::from_utf8_lossy(&out.stdout));
        return Err(tail(e.trim(), 1500).to_string());
    }
    let stats = std::process::Command::new(exe).arg("--stats").arg(png).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&stats.stdout);
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| {
            !l.trim().is_empty()
                && !l.starts_with("stats")
                && !l.contains("histogram")
                && !l.contains("pixels)")
                && !(l.starts_with(char::is_whitespace) && l.trim().chars().all(|c| "RYGCBM ".contains(c)))
        })
        .collect();
    Ok(lines.join("\n"))
}

/// A user message: text, plus the picture if the model can see.
fn with_image(text: &str, png: Option<&Path>, vision: bool) -> Value {
    match png.filter(|p| vision && p.exists()).and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => json!([
            { "type": "text", "text": text },
            { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{}", crate::mcp::base64(&bytes)) } },
        ]),
        None => json!(text),
    }
}

/// Keep only the newest picture in the history; each costs context.
fn forget_old_images(convo: &mut [Value]) {
    let mut seen = false;
    for m in convo.iter_mut().rev() {
        if let Some(parts) = m["content"].as_array_mut() {
            if seen {
                for p in parts.iter_mut() {
                    if p["type"] != "text" {
                        *p = json!({ "type": "text", "text": "[earlier render omitted]" });
                    }
                }
            }
            seen = true;
        }
    }
}

fn tail(s: &str, n: usize) -> &str {
    let mut i = s.len().saturating_sub(n);
    while !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn stamp() -> String {
    let secs = thread::now();
    // 2026-10-27 23:14Z → 20261027-2314; to the second, as author.py's are.
    let w = thread::when(secs);
    format!("{}{:02}", w.replace(['-', ':', 'Z'], "").replace(' ', "-"), secs % 60)
}

fn iso_now() -> String {
    let secs = thread::now();
    thread::when(secs).replace(' ', "T").replace('Z', &format!(":{:02}Z", secs % 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_read_as_author_py_reads_them() {
        let r = "Here it is.\nNOTE: the primer is vague on haze\n```toml\n[meta]\nname = \"x\"\n```\nDONE\n";
        assert_eq!(toml_blocks(r), vec!["[meta]\nname = \"x\"\n".to_string()]);
        assert!(has_line(r, "DONE"));
        assert!(has_line("**STOP**", "STOP"));
        assert!(has_line("  DONE.", "DONE"));
        assert!(!has_line("I'm not DONE yet", "DONE"));
        assert!(toml_blocks("```toml extra\nx\n```").is_empty());
        assert_eq!(strip_blocks(r).trim(), "Here it is.\nNOTE: the primer is vague on haze\nDONE");
    }

    #[test]
    fn set_flags_read_as_toml() {
        assert_eq!(set_flags_as_toml("try -S camera.distance=1.42 to fill"), "try [camera] distance = 1.42 to fill");
        assert_eq!(set_flags_as_toml("-S transform.a.weight=2"), "-S transform.a.weight=2");
    }

    #[test]
    fn the_prompts_the_window_needs_are_in_llm_author() {
        // Run from the repo root, as the window is.
        if !crate::agent::llm_author_dir().exists() {
            return;
        }
        for p in ["system.md", "tweak.md", "request.md", "round.md", "repair.md", "same.md", "sendback.md", "report.md", "primer.md"] {
            read_prompt_file(p).unwrap();
        }
        assert!(load_grammar().unwrap().starts_with("root ::= finish"));
    }

    #[test]
    fn only_the_newest_picture_is_kept() {
        let img = |n| json!({ "role": "user", "content": [{ "type": "text", "text": n }, { "type": "image_url", "image_url": { "url": "x" } }] });
        let mut c = vec![img("a"), json!({ "role": "assistant", "content": "ok" }), img("b")];
        forget_old_images(&mut c);
        assert_eq!(c[0]["content"][1]["text"], "[earlier render omitted]");
        assert_eq!(c[2]["content"][1]["type"], "image_url");
    }
}
