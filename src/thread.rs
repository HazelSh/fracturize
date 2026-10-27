//! Threads: requests, proposals and responses between a person and an agent,
//! about one scene, kept as files (AGENT-COLLAB-PLAN.md §4, §7).
//!
//! Either side can send any kind of message. A **request** is text (and,
//! from the GUI, the scene as it was and a render of the view). A
//! **proposal** carries 1–9 options — each a scene, usually a render, and a
//! note — and the **base** the options were made from, which is what lets
//! Apply lay an option's changes over a scene that has moved on since
//! (`scene_diff::merge`) rather than writing the option over it. A
//! **response** answers a proposal: per option applied, picked, rejected,
//! sent back (with a note, and a tweaked scene if there is one) or commented.
//!
//! ```text
//! renders/collab/<scene-stem>/
//!   thread.toml                  # which scene
//!   0001-request/   message.toml  base.toml  view.png
//!   0002-proposal/  message.toml  base.toml  A.toml A.png  B.toml B.png
//!   0003-response/  message.toml  B.tweak.toml
//! ```
//!
//! **A message is written once and never edited.** Each is a directory,
//! built under a temporary name and renamed into place, so a reader never
//! sees half of one; two writers racing for a number both land, one a number
//! later. The only thing added to a message afterwards is `withdrawn.toml`,
//! written the same way. Threads live under the gitignored `renders/`, so
//! they never reach the public repo (§10.2); Keep, in the window, is the one
//! way an option reaches `scenes/`.
//!
//! **Who a message is for** is loose on purpose: `from` and `to` are free
//! text, and a participant treats anything not from itself as for it. Agents
//! name themselves with `--from`; the window signs with the author name in
//! prefs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::scene::Scene;

/// Where threads live, relative to where the program runs, like `scenes/`.
pub const ROOT: &str = "renders/collab";

/// At most this many options in a proposal.
pub const MAX_OPTIONS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Request,
    Proposal,
    Response,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Request => "request",
            Kind::Proposal => "proposal",
            Kind::Response => "response",
        }
    }
}

/// What a proposal asks of whoever answers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ask {
    /// Apply or pick one option.
    #[default]
    PickOne,
    /// Any number of them.
    PickAny,
    /// Comments only.
    Free,
}

impl Ask {
    pub fn name(self) -> &'static str {
        match self {
            Ask::PickOne => "pick-one",
            Ask::PickAny => "pick-any",
            Ask::Free => "free",
        }
    }
}

/// One option of a proposal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opt {
    pub id: String,
    /// File name of its scene, in the message's directory.
    pub scene: String,
    /// File name of its render, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// What became of an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Laid over the live scene (only a person's window does this).
    Applied,
    /// Chosen, without being applied: an agent's answer to a person's
    /// proposal, or a person's choice to look at later.
    Picked,
    Rejected,
    /// Wanted again, changed: the note says how, the tweak (if any) shows.
    SentBack,
    Comment,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Applied => "applied",
            Verdict::Picked => "picked",
            Verdict::Rejected => "rejected",
            Verdict::SentBack => "sent back",
            Verdict::Comment => "comment",
        }
    }
}

/// A response's word on one option.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub option: String,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// File name of a tweaked scene, in the response's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tweak: Option<String>,
    /// File name of the tweak's render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tweak_render: Option<String>,
}

/// `message.toml`, as written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub kind: Kind,
    #[serde(default)]
    pub from: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to: String,
    /// Seconds since the epoch.
    #[serde(default)]
    pub created: u64,
    /// The message this one answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replying_to: Option<String>,
    /// `<message>/<option>`: what this proposal is a revision of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revises: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    #[serde(default)]
    pub message: String,
    /// Of `base.toml`, when the message carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_sha256: Option<String>,
    /// A request's render of the view it was sent from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<String>,
    /// Transforms the request is about, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<String>,
    #[serde(default, rename = "option", skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Opt>,
    #[serde(default, rename = "answer", skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<Answer>,
}

impl Header {
    pub fn new(kind: Kind, from: &str, to: &str, message: &str) -> Self {
        Self {
            kind,
            from: from.to_string(),
            to: to.to_string(),
            created: now(),
            replying_to: None,
            revises: None,
            ask: (kind == Kind::Proposal).then_some(Ask::PickOne),
            message: message.to_string(),
            base_sha256: None,
            render: None,
            selection: Vec::new(),
            options: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// The first line of the text, for labels.
    pub fn headline(&self) -> &str {
        self.message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
    }
}

/// `withdrawn.toml`: the sender took the message back before it was answered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdrawn {
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub at: u64,
}

/// A message as read back.
#[derive(Clone, Debug)]
pub struct Message {
    /// Its directory's name: `0002-proposal`.
    pub id: String,
    pub dir: PathBuf,
    pub header: Header,
    pub withdrawn: Option<Withdrawn>,
}

impl Message {
    /// The number in its name.
    pub fn number(&self) -> u32 {
        number_of(&self.id).unwrap_or(0)
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The scene the options were made from, if it carries one.
    pub fn base(&self) -> Option<Result<Scene, String>> {
        let p = self.file("base.toml");
        p.exists().then(|| Scene::load(&p))
    }

    pub fn option(&self, id: &str) -> Option<&Opt> {
        self.header.options.iter().find(|o| o.id == id)
    }

    pub fn option_scene(&self, opt: &Opt) -> Result<Scene, String> {
        Scene::load(self.file(&opt.scene))
    }
}

/// `thread.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThreadFile {
    /// The scene the thread is about, as its path was given.
    #[serde(default)]
    pub scene: String,
    #[serde(default)]
    pub created: u64,
}

/// A thread, read from disk.
#[derive(Clone, Debug)]
pub struct Thread {
    pub dir: PathBuf,
    pub file: ThreadFile,
    /// Oldest first.
    pub messages: Vec<Message>,
    /// Message directories that didn't read, and why: shown, not skipped
    /// silently.
    pub broken: Vec<(String, String)>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn number_of(id: &str) -> Option<u32> {
    id.split('-').next()?.parse().ok()
}

/// The thread directory for a scene: named after its file's stem, or the
/// scene's name for one that has no file yet.
pub fn dir_for(scene_path: Option<&Path>, scene_name: &str) -> PathBuf {
    let stem = scene_path
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| slug(scene_name));
    Path::new(ROOT).join(stem)
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "untitled".to_string() } else { s }
}

impl Thread {
    /// Read a thread. A directory that isn't there yet is an empty thread.
    pub fn open(dir: &Path) -> Result<Thread, String> {
        let file = match std::fs::read_to_string(dir.join("thread.toml")) {
            Ok(t) => toml::from_str(&t).map_err(|e| format!("{}: {e}", dir.join("thread.toml").display()))?,
            Err(_) => ThreadFile::default(),
        };
        let mut messages = Vec::new();
        let mut broken = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut ids: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.') && number_of(n).is_some())
                .collect();
            ids.sort_by_key(|n| (number_of(n), n.clone()));
            for id in ids {
                let mdir = dir.join(&id);
                match read_message(&mdir) {
                    Ok((header, withdrawn)) => messages.push(Message { id, dir: mdir, header, withdrawn }),
                    Err(e) => broken.push((id, e)),
                }
            }
        }
        // `replying_to = "0001"` names `0001-request` as well as the full id
        // does; say it in full, so the links the thread is read by hold.
        let full: Vec<(u32, String)> = messages.iter().map(|m| (m.number(), m.id.clone())).collect();
        let canonical = |r: &str| match r.parse::<u32>() {
            Ok(n) => full.iter().find(|(k, _)| *k == n).map_or(r.to_string(), |(_, id)| id.clone()),
            Err(_) => r.to_string(),
        };
        for m in &mut messages {
            if let Some(r) = &m.header.replying_to {
                m.header.replying_to = Some(canonical(r));
            }
            if let Some(r) = &m.header.revises
                && let Some((msg, opt)) = r.split_once('/')
            {
                m.header.revises = Some(format!("{}/{opt}", canonical(msg)));
            }
        }
        Ok(Thread { dir: dir.to_path_buf(), file, messages, broken })
    }

    pub fn get(&self, id: &str) -> Option<&Message> {
        // `0002` and `2` name `0002-proposal` too.
        self.messages
            .iter()
            .find(|m| m.id == id)
            .or_else(|| id.parse::<u32>().ok().and_then(|n| self.messages.iter().find(|m| m.number() == n)))
    }

    /// The messages that answer `id`.
    pub fn replies_to<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Message> + 'a {
        self.messages.iter().filter(move |m| m.header.replying_to.as_deref() == Some(id))
    }

    /// Whether a message still wants an answer: a request or proposal that
    /// nothing replies to and that hasn't been withdrawn.
    pub fn waiting(&self, m: &Message) -> bool {
        m.header.kind != Kind::Response && m.withdrawn.is_none() && self.replies_to(&m.id).next().is_none()
    }

    /// Messages waiting for someone other than `me`.
    pub fn waiting_for<'a>(&'a self, me: &'a str) -> impl Iterator<Item = &'a Message> + 'a {
        self.messages.iter().filter(move |m| m.header.from != me && self.waiting(m))
    }

    /// Who a new message from `me` is to, by default: whoever last wrote
    /// that wasn't `me`, else `fallback`.
    pub fn other_party(&self, me: &str, fallback: &str) -> String {
        self.messages
            .iter()
            .rev()
            .map(|m| m.header.from.as_str())
            .find(|f| *f != me && !f.is_empty())
            .unwrap_or(fallback)
            .to_string()
    }

    /// What became of option `opt` of proposal `id`, by the latest answer.
    pub fn verdict<'a>(&'a self, id: &'a str, opt: &'a str) -> Option<(&'a Message, &'a Answer)> {
        self.replies_to(id)
            .filter(|m| m.header.kind == Kind::Response)
            .flat_map(|m| m.header.answers.iter().filter(|a| a.option == opt).map(move |a| (m, a)))
            .last()
    }

    /// Every thread under `root`, by directory name.
    pub fn all(root: &Path) -> Vec<Thread> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for d in dirs {
                if let Ok(t) = Thread::open(&d) {
                    out.push(t);
                }
            }
        }
        out
    }

    /// The thread's name: its directory's.
    pub fn name(&self) -> String {
        self.dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

fn read_message(dir: &Path) -> Result<(Header, Option<Withdrawn>), String> {
    let path = dir.join("message.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let header: Header = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let withdrawn = match std::fs::read_to_string(dir.join("withdrawn.toml")) {
        Ok(t) => Some(toml::from_str(&t).map_err(|e| format!("withdrawn.toml: {e}"))?),
        Err(_) => None,
    };
    Ok((header, withdrawn))
}

/// A file to put in a new message.
pub enum Content {
    Bytes(Vec<u8>),
    Copy(PathBuf),
}

/// A message to post: its header and the files that go beside it.
pub struct Draft {
    pub header: Header,
    pub files: BTreeMap<String, Content>,
}

impl Draft {
    pub fn new(header: Header) -> Self {
        Self { header, files: BTreeMap::new() }
    }

    /// Carry `scene` as `name`, and record its hash if it is the base.
    pub fn scene(&mut self, name: &str, scene: &Scene) -> Result<(), String> {
        let text = scene.to_toml_string()?;
        if name == "base.toml" {
            self.header.base_sha256 = Some(crate::record::sha256_hex(text.as_bytes()));
        }
        self.files.insert(name.to_string(), Content::Bytes(text.into_bytes()));
        Ok(())
    }
}

/// Post a draft to the thread in `dir`, creating the thread for `scene` if
/// it isn't there. Returns the new message's id.
pub fn post(dir: &Path, scene: &str, draft: Draft) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tf = dir.join("thread.toml");
    if !tf.exists() {
        let file = ThreadFile { scene: scene.to_string(), created: now() };
        let text = toml::to_string(&file).map_err(|e| e.to_string())?;
        write_atomic(&tf, text.as_bytes())?;
    }
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), nanos()));
    std::fs::create_dir(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let staged = (|| -> Result<(), String> {
        for (name, content) in &draft.files {
            let to = tmp.join(name);
            match content {
                Content::Bytes(b) => std::fs::write(&to, b),
                Content::Copy(from) => std::fs::copy(from, &to).map(|_| ()),
            }
            .map_err(|e| format!("{name}: {e}"))?;
        }
        let text = toml::to_string(&draft.header).map_err(|e| format!("message.toml: {e}"))?;
        std::fs::write(tmp.join("message.toml"), text).map_err(|e| format!("message.toml: {e}"))
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let kind = draft.header.kind.name();
    let mut n = Thread::open(dir).map(|t| t.messages.iter().map(Message::number).max().unwrap_or(0)).unwrap_or(0) + 1;
    for _ in 0..64 {
        let id = format!("{n:04}-{kind}");
        let target = dir.join(&id);
        // A plain rename would replace an empty directory someone else made;
        // check first, and let a lost race move on to the next number.
        if !target.exists() && std::fs::rename(&tmp, &target).is_ok() {
            return Ok(id);
        }
        n += 1;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Err(format!("couldn't find a free message number in {}", dir.display()))
}

/// Take a message back. Only an unanswered one: once answered, the answer
/// would be to nothing.
pub fn withdraw(thread: &Thread, id: &str, reason: &str) -> Result<String, String> {
    let m = thread.get(id).ok_or_else(|| format!("no message {id} in {}", thread.dir.display()))?;
    if m.withdrawn.is_some() {
        return Err(format!("{} is already withdrawn", m.id));
    }
    if !thread.waiting(m) {
        return Err(format!("{} has been answered, so it can't be withdrawn", m.id));
    }
    let w = Withdrawn { reason: reason.to_string(), at: now() };
    let text = toml::to_string(&w).map_err(|e| e.to_string())?;
    write_atomic(&m.file("withdrawn.toml"), text.as_bytes())?;
    Ok(m.id.clone())
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", nanos()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Check a proposal directory an agent made and turn it into a draft:
/// `message.toml` (kind may be left out), `base.toml`, and every option's
/// scene and render. Everything that's wrong is reported at once.
pub fn proposal_from_dir(src: &Path, from: &str, to: &str) -> Result<Draft, String> {
    let path = src.join("message.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut table: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    table.entry("kind").or_insert_with(|| "proposal".into());
    let mut header: Header = table.try_into().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut problems = Vec::new();
    if header.kind != Kind::Proposal {
        problems.push(format!("kind is {}, not proposal", header.kind.name()));
    }
    if header.options.is_empty() || header.options.len() > MAX_OPTIONS {
        problems.push(format!("a proposal has 1 to {MAX_OPTIONS} options, this has {}", header.options.len()));
    }
    header.ask.get_or_insert(Ask::PickOne);
    if header.from.is_empty() {
        header.from = from.to_string();
    }
    if header.to.is_empty() {
        header.to = to.to_string();
    }
    header.created = now();

    let mut draft_files = BTreeMap::new();
    let base = src.join("base.toml");
    match std::fs::read(&base) {
        Ok(bytes) => {
            if let Err(e) = Scene::load(&base) {
                problems.push(format!("base.toml doesn't load: {e}"));
            }
            header.base_sha256 = Some(crate::record::sha256_hex(&bytes));
            draft_files.insert("base.toml".to_string(), Content::Copy(base));
        }
        Err(_) => problems.push("no base.toml: the scene the options were made from".to_string()),
    }
    let mut ids = std::collections::BTreeSet::new();
    for o in &header.options {
        if o.id.is_empty() || o.id.contains(['/', '\\']) || !ids.insert(o.id.clone()) {
            problems.push(format!("option id '{}' is empty, has a slash, or is used twice", o.id));
        }
        for name in std::iter::once(&o.scene).chain(o.render.as_ref()) {
            if name.contains(['/', '\\']) || name == "message.toml" || name == "base.toml" {
                problems.push(format!("option {}: '{name}' must be a plain file name beside message.toml", o.id));
                continue;
            }
            let p = src.join(name);
            if !p.is_file() {
                problems.push(format!("option {}: {name} isn't there", o.id));
                continue;
            }
            draft_files.insert(name.clone(), Content::Copy(p));
        }
        if let Err(e) = Scene::load(src.join(&o.scene)) {
            if src.join(&o.scene).is_file() {
                problems.push(format!("option {}: {} doesn't load: {e}", o.id, o.scene));
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    Ok(Draft { header, files: draft_files })
}

// ------------------------------------------------------------------- text

/// A message's time, as local-ish text. UTC, to keep it free of a timezone
/// dependency; the order is what matters.
pub fn when(secs: u64) -> String {
    let days = secs / 86_400;
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}Z")
}

/// A one-line count of what changed between two scenes: `3 values, +1
/// transform`.
pub fn change_summary(from: &Scene, to: &Scene) -> String {
    match crate::scene_diff::diff(from, to) {
        Ok(c) if c.is_empty() => "no changes".to_string(),
        Ok(c) => crate::scene_diff::summary(&c),
        Err(e) => format!("can't compare: {e}"),
    }
}

/// The thread as text, for `--thread` and `--wait-reply`: every message, and
/// for each option its changes against the base, ready to read or paste.
pub fn render_text(thread: &Thread, since: Option<u32>) -> String {
    let mut out = String::new();
    if !thread.file.scene.is_empty() {
        out.push_str(&format!("thread {} · scene {}\n", thread.name(), thread.file.scene));
    } else {
        out.push_str(&format!("thread {}\n", thread.name()));
    }
    for m in thread.messages.iter().filter(|m| since.is_none_or(|s| m.number() > s)) {
        out.push('\n');
        out.push_str(&render_message(thread, m));
    }
    for (id, e) in &thread.broken {
        out.push_str(&format!("\n{id}: unreadable: {e}\n"));
    }
    out
}

pub fn render_message(thread: &Thread, m: &Message) -> String {
    let h = &m.header;
    let mut out = String::new();
    let to = if h.to.is_empty() { String::new() } else { format!(" to {}", h.to) };
    let state = if let Some(w) = &m.withdrawn {
        format!("withdrawn{}", if w.reason.is_empty() { String::new() } else { format!(": {}", w.reason) })
    } else if h.kind == Kind::Response {
        String::new()
    } else if thread.waiting(m) {
        "waiting".to_string()
    } else {
        "answered".to_string()
    };
    out.push_str(&format!("{}  from {}{to}  {}", m.id, h.from, when(h.created)));
    if let Some(ask) = h.ask.filter(|_| h.kind == Kind::Proposal) {
        out.push_str(&format!("  ask {}", ask.name()));
    }
    if !state.is_empty() {
        out.push_str(&format!("  [{state}]"));
    }
    out.push('\n');
    if let Some(r) = &h.replying_to {
        out.push_str(&format!("  replying to {r}\n"));
    }
    if let Some(r) = &h.revises {
        out.push_str(&format!("  revises {r}\n"));
    }
    for line in h.message.trim_end().lines() {
        out.push_str(&format!("  > {line}\n"));
    }
    if !h.selection.is_empty() {
        out.push_str(&format!("  about {}\n", h.selection.join(", ")));
    }
    if let Some(r) = &h.render {
        out.push_str(&format!("  render {}\n", m.file(r).display()));
    }
    if m.file("base.toml").exists() {
        out.push_str(&format!("  base {}\n", m.file("base.toml").display()));
    }
    let base = m.base().and_then(Result::ok);
    for o in &h.options {
        out.push_str(&format!("  option {}  {}", o.id, m.file(&o.scene).display()));
        if let Some(r) = &o.render {
            out.push_str(&format!("  render {r}"));
        }
        out.push('\n');
        if !o.note.is_empty() {
            out.push_str(&format!("    note: {}\n", o.note));
        }
        if let Some((_, a)) = thread.verdict(&m.id, &o.id) {
            out.push_str(&format!("    {}{}\n", a.verdict.name(), if a.note.is_empty() { String::new() } else { format!(": {}", a.note) }));
        }
        match (&base, m.option_scene(o)) {
            (Some(b), Ok(s)) => out.push_str(&indent(&diff_lines(b, &s), "    ")),
            (_, Err(e)) => out.push_str(&format!("    doesn't load: {e}\n")),
            (None, _) => {}
        }
    }
    let answered = h.replying_to.as_deref().and_then(|r| thread.get(r));
    for a in &h.answers {
        out.push_str(&format!("  {} {}", a.option, a.verdict.name()));
        if !a.note.is_empty() {
            out.push_str(&format!(": {}", a.note));
        }
        out.push('\n');
        if let Some(t) = &a.tweak {
            out.push_str(&format!("    tweak {}\n", m.file(t).display()));
            let original = answered.and_then(|p| p.option(&a.option).map(|o| p.option_scene(o)));
            match (original, Scene::load(m.file(t))) {
                (Some(Ok(o)), Ok(t)) => {
                    out.push_str("    the tweak, against the option:\n");
                    out.push_str(&indent(&diff_lines(&o, &t), "      "));
                }
                (_, Err(e)) => out.push_str(&format!("    tweak doesn't load: {e}\n")),
                _ => {}
            }
        }
    }
    out
}

fn diff_lines(from: &Scene, to: &Scene) -> String {
    match crate::scene_diff::diff(from, to) {
        Ok(c) if c.is_empty() => "no changes\n".to_string(),
        Ok(c) => format!("{}\n{}", crate::scene_diff::summary(&c), crate::scene_diff::render(&c)),
        Err(e) => format!("can't compare: {e}\n"),
    }
}

fn indent(text: &str, by: &str) -> String {
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| format!("{by}{l}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fracturize-thread-{name}-{}-{}", std::process::id(), nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_request_a_proposal_and_a_response_make_a_thread() {
        let root = tmp("flow");
        let dir = root.join("koru");
        let base = Scene::blank();

        let mut req = Draft::new(Header::new(Kind::Request, "hazel", "claude", "thinner seams, please"));
        req.scene("base.toml", &base).unwrap();
        let r = post(&dir, "scenes/koru.toml", req).unwrap();
        assert_eq!(r, "0001-request");

        // An agent's proposal, from a directory it made.
        let src = root.join("prop");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("base.toml"), base.to_toml_string().unwrap()).unwrap();
        let mut a = base.clone();
        a.transforms[0].weight = 0.25;
        std::fs::write(src.join("A.toml"), a.to_toml_string().unwrap()).unwrap();
        std::fs::write(
            src.join("message.toml"),
            "replying_to = \"0001-request\"\nmessage = \"lighter left half\"\n\n[[option]]\nid = \"A\"\nscene = \"A.toml\"\nnote = \"weight 0.25\"\n",
        )
        .unwrap();
        let draft = proposal_from_dir(&src, "claude", "hazel").unwrap();
        assert_eq!(post(&dir, "scenes/koru.toml", draft).unwrap(), "0002-proposal");

        let t = Thread::open(&dir).unwrap();
        assert_eq!(t.file.scene, "scenes/koru.toml");
        assert_eq!(t.messages.len(), 2);
        assert!(!t.waiting(&t.messages[0]), "the request is answered by the proposal");
        assert!(t.waiting(&t.messages[1]));
        assert_eq!(t.waiting_for("hazel").count(), 1);
        assert_eq!(t.other_party("hazel", "agent"), "claude");
        let text = render_text(&t, None);
        assert!(text.contains("thinner seams"), "{text}");
        assert!(text.contains("transform.") && text.contains("weight=0.25"), "{text}");

        let mut resp = Header::new(Kind::Response, "hazel", "claude", "close");
        resp.replying_to = Some("0002-proposal".into());
        resp.answers.push(Answer { option: "A".into(), verdict: Verdict::SentBack, note: "less".into(), tweak: None, tweak_render: None });
        post(&dir, "scenes/koru.toml", Draft::new(resp)).unwrap();
        let t = Thread::open(&dir).unwrap();
        assert!(!t.waiting(&t.messages[1]));
        assert_eq!(t.verdict("0002-proposal", "A").map(|(_, a)| a.verdict), Some(Verdict::SentBack));
        assert!(withdraw(&t, "0002", "nope").is_err(), "answered messages stay");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_proposal_says_everything_wrong_with_it() {
        let src = tmp("bad");
        std::fs::write(
            src.join("message.toml"),
            "message = \"x\"\n[[option]]\nid = \"A\"\nscene = \"missing.toml\"\nrender = \"../escape.png\"\n",
        )
        .unwrap();
        let e = proposal_from_dir(&src, "a", "b").err().expect("refused");
        assert!(e.contains("base.toml"), "{e}");
        assert!(e.contains("missing.toml isn't there"), "{e}");
        assert!(e.contains("plain file name"), "{e}");
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn withdrawing_marks_the_message_and_nothing_else() {
        let dir = tmp("withdraw");
        let id = post(&dir, "s.toml", Draft::new(Header::new(Kind::Request, "claude", "", "frame it"))).unwrap();
        let t = Thread::open(&dir).unwrap();
        withdraw(&t, &id, "changed my mind").unwrap();
        let t = Thread::open(&dir).unwrap();
        assert_eq!(t.messages[0].withdrawn.as_ref().map(|w| w.reason.as_str()), Some("changed my mind"));
        assert!(!t.waiting(&t.messages[0]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reply_by_number_answers_what_it_names() {
        // `replying_to = "1"` was accepted and then matched nothing, so the
        // request it answered stayed waiting for ever.
        let dir = tmp("short");
        post(&dir, "s.toml", Draft::new(Header::new(Kind::Request, "hazel", "", "frame it"))).unwrap();
        let mut h = Header::new(Kind::Proposal, "claude", "hazel", "framed");
        h.replying_to = Some("1".into());
        h.revises = Some("0001/A".into());
        post(&dir, "s.toml", Draft::new(h)).unwrap();
        let t = Thread::open(&dir).unwrap();
        assert_eq!(t.messages[1].header.replying_to.as_deref(), Some("0001-request"));
        assert_eq!(t.messages[1].header.revises.as_deref(), Some("0001-request/A"));
        assert!(!t.waiting(&t.messages[0]), "the request is answered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn racing_writers_both_land() {
        let dir = tmp("race");
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || post(&dir, "s.toml", Draft::new(Header::new(Kind::Request, &format!("w{i}"), "", "hi"))))
            })
            .collect();
        let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 6, "{ids:?}");
        assert_eq!(Thread::open(&dir).unwrap().messages.len(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dates_read_as_dates() {
        assert_eq!(when(0), "1970-01-01 00:00Z");
        assert_eq!(when(1_761_442_200), "2025-10-26 01:30Z");
    }
}
