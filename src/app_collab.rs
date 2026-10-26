//! The person's side of a thread (AGENT-COLLAB-PLAN.md §4): what the
//! Collaborate window asks of `App`. The window is `ui::collab`; the format
//! and the reading are `thread.rs`, shared with the CLI an agent uses.
//!
//! Three things here change what `App` means while they're going on:
//!
//! - **Preview puts the option on screen and leaves the live scene alone.**
//!   The live scene, its undo history and its save point are set aside, and
//!   the option comes up with a history of its own. So previewing touches
//!   neither undo nor the dirty bit, and an edit made while previewing is a
//!   **tweak**: a change to a copy of the option, undoable inside the preview,
//!   kept when you leave it and shown on its card. Ctrl+S is refused while
//!   previewing (it would write the option over the scene's file), and opening
//!   another scene leaves the preview first.
//! - **Apply is a three-way merge.** The changes from the proposal's
//!   `base.toml` to the option (or its tweak) are laid over the live scene by
//!   `scene_diff::merge`, so whatever was edited since the request was sent
//!   survives. Where both sides changed one value the window lists the rows,
//!   yours beside theirs, and nothing is applied until each is chosen. The
//!   result is one edit, so one Ctrl+Z.
//! - **Only a person's action changes the live scene.** Agents propose; the
//!   window applies.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::history::{EditSnapshot, History};
use crate::scene::Scene;
use crate::thread::{self, Answer, Draft, Header, Kind, Thread, Verdict};

use super::App;

/// How often the thread is read again while nothing else asks.
const RESCAN: Duration = Duration::from_millis(1500);

/// An option on screen in place of the live scene.
pub struct Preview {
    pub message: String,
    pub option: String,
    /// The live scene and everything history tracks with it.
    live: EditSnapshot,
    /// The live history and save point, set aside.
    history: History,
    saved_serial: Option<u64>,
    /// While "hold to compare" is held: the option as it was, the live scene
    /// being on screen instead.
    blink: Option<EditSnapshot>,
}

/// An Apply waiting on the person to settle its conflicts.
pub struct PendingApply {
    pub message: String,
    pub option: String,
    pub conflicts: Vec<crate::scene_diff::Conflict>,
    /// Per conflict: take theirs.
    pub take_theirs: Vec<bool>,
    base: Scene,
    theirs: Scene,
}

/// A message that waits for a render of the view before it's posted.
struct Staged {
    draft: Draft,
    /// The file name the render goes in as.
    name: String,
    /// The option it renders, for a proposal.
    option: Option<usize>,
    shot: PathBuf,
}

#[derive(Default)]
pub struct Collab {
    pub thread: Option<Thread>,
    pub dir: PathBuf,
    scanned: Option<Instant>,
    /// Messages for the person when last counted, for the notification.
    seen_waiting: Option<BTreeSet<String>>,
    pub preview: Option<Preview>,
    /// Tweaks made while previewing, by (message, option), for the thread on
    /// show; cleared when that changes.
    pub tweaks: HashMap<(String, String), Scene>,
    pub pending_apply: Option<PendingApply>,
    staged: Option<Staged>,
}

impl App {
    /// Who the person is in a thread: their author name, else `human`.
    pub fn collab_me(&self) -> String {
        self.prefs.author.clone().filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "human".to_string())
    }

    /// The thread for the scene on screen (the live one, while previewing).
    pub fn collab_dir_shown(&self) -> PathBuf {
        self.collab_dir()
    }

    fn collab_dir(&self) -> PathBuf {
        let name = match &self.collab.preview {
            Some(p) => p.live.scene.name.clone(),
            None => self.scene.name.clone(),
        };
        thread::dir_for(self.scene_path.as_deref().map(Path::new), &name)
    }

    /// Read the thread again if it's time, or `now`. Cheap: a directory
    /// listing and a few small files. Called every frame; it rations itself.
    pub fn poll_collab(&mut self, now: bool) {
        let dir = self.collab_dir();
        let due = now || dir != self.collab.dir || self.collab.scanned.is_none_or(|t| t.elapsed() >= RESCAN);
        if !due {
            return;
        }
        self.collab.scanned = Some(Instant::now());
        self.collab.thread = Thread::open(&dir).ok().filter(|t| !t.messages.is_empty() || !t.broken.is_empty());
        let fresh_dir = dir != self.collab.dir;
        if fresh_dir {
            // Tweaks are by message id, which every thread numbers from one.
            self.collab.tweaks.clear();
        }
        self.collab.dir = dir;
        let me = self.collab_me();
        let waiting: BTreeSet<String> = self
            .collab
            .thread
            .as_ref()
            .map(|t| t.waiting_for(&me).map(|m| m.id.clone()).collect())
            .unwrap_or_default();
        // Tell the desktop about a message that arrived while the window
        // wasn't looked at. Never a window over the work: a badge, and this.
        if let Some(before) = &self.collab.seen_waiting
            && !fresh_dir
            && !self.focused
        {
            for id in waiting.difference(before) {
                if let Some(m) = self.collab.thread.as_ref().and_then(|t| t.get(id)) {
                    let _ = std::process::Command::new("notify-send")
                        .args(["--app-name=Fracturize", &format!("{} sent a {}", m.header.from, m.header.kind.name()), m.header.headline()])
                        .spawn();
                }
            }
        }
        self.collab.seen_waiting = Some(waiting);
    }

    /// Messages in this scene's thread waiting on the person: the toolbar's
    /// badge.
    pub fn collab_waiting(&self) -> usize {
        let me = self.collab_me();
        self.collab.thread.as_ref().map_or(0, |t| t.waiting_for(&me).count())
    }

    /// The scene as the person sees it, to send: the live values that only
    /// reach the scene on a save, and the framing they're looking through.
    fn scene_to_send(&self) -> Scene {
        let snap = match &self.collab.preview {
            Some(p) => p.live.clone(),
            None => self.edit_snapshot(),
        };
        let mut scene = snap.scene.clone();
        scene.set_camera(&self.camera);
        scene.point_size = snap.point_size;
        scene.color_falloff = snap.color_falloff;
        scene.color_contrast = snap.color_contrast;
        scene.haze = snap.haze_amount;
        scene.exposure = snap.exposure;
        scene
    }

    fn thread_scene_name(&self) -> String {
        self.scene_path.clone().unwrap_or_else(|| format!("({})", self.scene.name))
    }

    /// Post a request: the text, the scene as it is (as `base.toml`), the
    /// selected transform, and a render of the view if `render`.
    pub fn collab_request(&mut self, text: &str, render: bool, selection: bool) {
        let me = self.collab_me();
        let to = self.collab.thread.as_ref().map_or("agent".to_string(), |t| t.other_party(&me, "agent"));
        let mut header = Header::new(Kind::Request, &me, &to, text);
        if selection && let Some(i) = self.selected_transform() {
            header.selection.push(self.scene.transform_names.get(i).cloned().flatten().unwrap_or_else(|| i.to_string()));
        }
        let mut draft = Draft::new(header);
        if let Err(e) = draft.scene("base.toml", &self.scene_to_send()) {
            self.set_notice(format!("Couldn't send: {e}"), true);
            return;
        }
        if render {
            self.stage(draft, "view.png", None);
        } else {
            self.post_draft(draft);
        }
    }

    /// Propose the scene as it is now: one option, against the scene as last
    /// saved (or, unsaved, the thread's first base). It answers the newest
    /// request waiting on the person, if there is one — "frame it the way
    /// you'd hang it" — and otherwise asks what the agent would do with it.
    pub fn collab_propose_mine(&mut self, text: &str, render: bool) {
        let me = self.collab_me();
        let thread = self.collab.thread.as_ref();
        let to = thread.map_or("agent".to_string(), |t| t.other_party(&me, "agent"));
        let mut header = Header::new(Kind::Proposal, &me, &to, text);
        header.ask = Some(thread::Ask::Free);
        header.replying_to = thread.and_then(|t| {
            t.messages.iter().rev().find(|m| m.header.kind == Kind::Request && m.header.from != me && t.waiting(m)).map(|m| m.id.clone())
        });
        let saved = self.scene_path.as_deref().and_then(|p| Scene::load(p).ok());
        let first_base = thread.and_then(|t| t.messages.iter().find_map(|m| m.base().and_then(Result::ok)));
        let mine = self.scene_to_send();
        let base = saved.or(first_base).unwrap_or_else(|| mine.clone());
        header.options.push(thread::Opt {
            id: "mine".into(),
            scene: "mine.toml".into(),
            render: None,
            note: String::new(),
        });
        let mut draft = Draft::new(header);
        if let Err(e) = draft.scene("base.toml", &base).and_then(|()| draft.scene("mine.toml", &mine)) {
            self.set_notice(format!("Couldn't send: {e}"), true);
            return;
        }
        if render {
            self.stage(draft, "mine.png", Some(0));
        } else {
            self.post_draft(draft);
        }
    }

    /// Hold a draft until the next frame's screenshot of the view lands.
    fn stage(&mut self, draft: Draft, name: &str, option: Option<usize>) {
        let shot = std::env::temp_dir().join(format!("fracturize-collab-{}-{}.png", std::process::id(), thread::now()));
        self.collab.staged = Some(Staged { draft, name: name.to_string(), option, shot: shot.clone() });
        self.screenshot_to = Some(shot);
        self.request_screenshot();
    }

    /// The screenshot a staged message was waiting for has been written (or
    /// failed): post it.
    pub(super) fn collab_screenshot_done(&mut self, written: Option<&Path>) {
        let Some(mut staged) = self.collab.staged.take() else { return };
        if let Some(path) = written.filter(|p| p.exists()) {
            staged.draft.files.insert(staged.name.clone(), thread::Content::Copy(path.to_path_buf()));
            match staged.option {
                Some(i) => {
                    if let Some(o) = staged.draft.header.options.get_mut(i) {
                        o.render = Some(staged.name.clone());
                    }
                }
                None => staged.draft.header.render = Some(staged.name.clone()),
            }
        }
        self.post_draft(staged.draft);
        let _ = std::fs::remove_file(&staged.shot);
    }

    fn post_draft(&mut self, draft: Draft) {
        let kind = draft.header.kind.name();
        let dir = self.collab_dir();
        match thread::post(&dir, &self.thread_scene_name(), draft) {
            Ok(id) => self.set_notice(format!("Sent {id} to {}", dir.display()), false),
            Err(e) => self.set_notice(format!("Couldn't send the {kind}: {e}"), true),
        }
        self.poll_collab(true);
    }

    /// The person's tweak of an option: the scene on screen while it's being
    /// previewed and has been edited, else what was kept when they left.
    fn tweak_of(&self, message: &str, option: &str) -> Option<Scene> {
        let p = self.collab.preview.as_ref();
        if p.is_some_and(|p| p.message == message && p.option == option) && self.preview_tweaked() {
            let shown = p.and_then(|p| p.blink.as_ref()).map_or(&self.scene, |b| &b.scene);
            return Some(shown.clone());
        }
        self.collab.tweaks.get(&(message.to_string(), option.to_string())).cloned()
    }

    /// The option, or the person's tweak of it.
    fn option_scene(&self, message: &str, option: &str) -> Result<(Scene, Scene), String> {
        let thread = self.collab.thread.as_ref().ok_or("no thread")?;
        let m = thread.get(message).ok_or_else(|| format!("no message {message}"))?;
        let opt = m.option(option).ok_or_else(|| format!("{message} has no option {option}"))?;
        let scene = match self.tweak_of(&m.id, option) {
            Some(t) => t,
            None => m.option_scene(opt)?,
        };
        let base = match m.base() {
            Some(b) => b?,
            // No base: the option is laid over what was there, whole.
            None => self.live_scene().clone(),
        };
        Ok((scene, base))
    }

    /// The live scene: the one previewing set aside, or the one on screen.
    pub fn live_scene(&self) -> &Scene {
        self.collab.preview.as_ref().map_or(&self.scene, |p| &p.live.scene)
    }

    // === Preview ===

    pub fn previewing(&self) -> Option<(&str, &str)> {
        self.collab.preview.as_ref().map(|p| (p.message.as_str(), p.option.as_str()))
    }

    /// What the toolbar says while an option is on screen.
    pub fn preview_label(&self) -> Option<String> {
        let p = self.collab.preview.as_ref()?;
        let tweaked = if self.history.top_serial().is_some() { ", tweaked" } else { "" };
        Some(format!("Previewing {} of {}{tweaked}", p.option, p.message))
    }

    /// Whether the option on screen has been edited.
    pub fn preview_tweaked(&self) -> bool {
        self.collab.preview.is_some() && self.history.top_serial().is_some()
    }

    pub fn start_preview(&mut self, message: &str, option: &str) {
        if self.collab.preview.is_some() {
            self.leave_preview();
        }
        let (scene, _) = match self.option_scene(message, option) {
            Ok(s) => s,
            Err(e) => return self.set_notice(format!("Can't preview: {e}"), true),
        };
        let live = self.edit_snapshot();
        let history = std::mem::replace(&mut self.history, History::new());
        let saved_serial = self.saved_serial.take();
        let id = self.collab.thread.as_ref().and_then(|t| t.get(message)).map_or(message.to_string(), |m| m.id.clone());
        self.collab.preview = Some(Preview { message: id, option: option.to_string(), live, history, saved_serial, blink: None });
        self.apply_snapshot(snapshot_of(scene));
    }

    /// Back to the live scene. A tweak is kept for the option's card.
    pub fn leave_preview(&mut self) {
        self.set_preview_blink(false);
        let Some(p) = self.collab.preview.take() else { return };
        if self.history.top_serial().is_some() {
            self.collab.tweaks.insert((p.message.clone(), p.option.clone()), self.scene.clone());
        }
        self.history = p.history;
        self.saved_serial = p.saved_serial;
        self.apply_snapshot(p.live);
    }

    /// Hold to compare: the live scene on screen while `on`, the option back
    /// when let go.
    pub fn set_preview_blink(&mut self, on: bool) {
        let Some(p) = &self.collab.preview else { return };
        match (on, p.blink.is_some()) {
            (true, false) => {
                let shown = self.edit_snapshot();
                let live = p.live.clone();
                if let Some(p) = &mut self.collab.preview {
                    p.blink = Some(shown);
                }
                self.apply_snapshot(live);
            }
            (false, true) => {
                let shown = self.collab.preview.as_mut().and_then(|p| p.blink.take());
                if let Some(s) = shown {
                    self.apply_snapshot(s);
                }
            }
            _ => {}
        }
    }

    /// Is the live scene dirty, whatever is on screen.
    pub(super) fn live_dirty(&self) -> Option<bool> {
        self.collab.preview.as_ref().map(|p| p.history.top_serial() != p.saved_serial)
    }

    /// Forget a tweak: the card goes back to the option as sent.
    pub fn drop_tweak(&mut self, message: &str, option: &str) {
        self.collab.tweaks.remove(&(message.to_string(), option.to_string()));
    }

    // === The verbs ===

    /// Lay an option's changes over the live scene. With conflicts, they wait
    /// for the person (see [`Self::finish_apply`]).
    pub fn collab_apply(&mut self, message: &str, option: &str) {
        let (theirs, base) = match self.option_scene(message, option) {
            Ok(s) => s,
            Err(e) => return self.set_notice(format!("Can't apply: {e}"), true),
        };
        if self.collab.preview.is_some() {
            self.leave_preview();
        }
        let dir = self.scene_dir();
        match crate::scene_diff::merge(&base, &self.scene, &theirs, &BTreeSet::new(), &dir) {
            Ok(m) if m.conflicts.is_empty() => self.commit_apply(message, option, &base, &theirs, m.scene),
            Ok(m) => {
                let n = m.conflicts.len();
                self.collab.pending_apply = Some(PendingApply {
                    message: message.to_string(),
                    option: option.to_string(),
                    take_theirs: vec![false; n],
                    conflicts: m.conflicts,
                    base,
                    theirs,
                });
            }
            Err(e) => self.set_notice(format!("Can't apply {option}: {e}"), true),
        }
    }

    /// Apply with the conflicts settled as the dialog says.
    pub fn finish_apply(&mut self) {
        let Some(p) = self.collab.pending_apply.take() else { return };
        let take: BTreeSet<String> =
            p.conflicts.iter().zip(&p.take_theirs).filter(|(_, t)| **t).map(|(c, _)| c.path.clone()).collect();
        let dir = self.scene_dir();
        match crate::scene_diff::merge(&p.base, &self.scene, &p.theirs, &take, &dir) {
            Ok(m) => self.commit_apply(&p.message, &p.option, &p.base, &p.theirs, m.scene),
            Err(e) => self.set_notice(format!("Can't apply {}: {e}", p.option), true),
        }
    }

    fn scene_dir(&self) -> PathBuf {
        self.scene_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .filter(|d| !d.as_os_str().is_empty())
            .map_or(PathBuf::from("."), Path::to_path_buf)
    }

    fn commit_apply(&mut self, message: &str, option: &str, base: &Scene, theirs: &Scene, merged: Scene) {
        let before = self.edit_snapshot();
        // The live values only reach the scene on a save, so the merge saw
        // the scene's copies; where the proposal didn't change one, the live
        // value stands.
        let pick = |theirs_v: f32, base_v: f32, merged_v: f32, live: f32| if theirs_v != base_v { merged_v } else { live };
        self.point_size = pick(theirs.point_size, base.point_size, merged.point_size, self.point_size);
        self.color_falloff = pick(theirs.color_falloff, base.color_falloff, merged.color_falloff, self.color_falloff);
        self.color_contrast = pick(theirs.color_contrast, base.color_contrast, merged.color_contrast, self.color_contrast);
        self.haze_amount = pick(theirs.haze, base.haze, merged.haze, self.haze_amount);
        self.exposure = pick(theirs.exposure, base.exposure, merged.exposure, self.exposure);
        self.scene = merged;
        self.transform_enabled.resize(self.scene.transforms.len(), true);
        self.bump_matrix_generation();
        self.after_scene_shape_change();
        let headline = self
            .collab
            .thread
            .as_ref()
            .and_then(|t| t.get(message))
            .map(|m| m.header.headline().to_string())
            .unwrap_or_default();
        let mut label = format!("Agent: {option}, {headline}");
        if label.chars().count() > 60 {
            label = label.chars().take(57).collect::<String>() + "...";
        }
        self.commit_edit(label, None, before);
        self.respond(message, option, Verdict::Applied, "");
    }

    /// Answer one option: applied, rejected or sent back (with the tweak, if
    /// there is one).
    pub fn respond(&mut self, message: &str, option: &str, verdict: Verdict, note: &str) {
        let me = self.collab_me();
        let Some(m) = self.collab.thread.as_ref().and_then(|t| t.get(message)) else { return };
        let (id, to) = (m.id.clone(), m.header.from.clone());
        let mut header = Header::new(Kind::Response, &me, &to, "");
        header.replying_to = Some(id.clone());
        let mut answer = Answer { option: option.to_string(), verdict, note: note.to_string(), tweak: None, tweak_render: None };
        let mut draft_scene = None;
        if verdict == Verdict::SentBack
            && let Some(t) = self.tweak_of(&id, option)
        {
            let name = format!("{option}.tweak.toml");
            answer.tweak = Some(name.clone());
            draft_scene = Some((name, t));
        }
        header.answers.push(answer);
        let mut draft = Draft::new(header);
        if let Some((name, scene)) = draft_scene
            && let Err(e) = draft.scene(&name, &scene)
        {
            return self.set_notice(format!("Couldn't send the tweak: {e}"), true);
        }
        self.post_draft(draft);
    }

    /// Keep an option as a scene of the person's own, in `scenes/`. The one
    /// way anything in a thread reaches the project.
    pub fn collab_keep(&mut self, message: &str, option: &str) {
        let (scene, _) = match self.option_scene(message, option) {
            Ok(s) => s,
            Err(e) => return self.set_notice(format!("Can't keep: {e}"), true),
        };
        let stem = self.collab_dir().file_name().map_or("thread".into(), |s| s.to_string_lossy().into_owned());
        let n = self.collab.thread.as_ref().and_then(|t| t.get(message)).map_or(0, |m| m.number());
        let mut path = PathBuf::from(format!("scenes/{stem}-{n}{option}.toml"));
        let mut k = 2;
        while path.exists() {
            path = PathBuf::from(format!("scenes/{stem}-{n}{option}-{k}.toml"));
            k += 1;
        }
        match scene.save(&path) {
            Ok(()) => self.set_notice(format!("Kept {option} as {}", path.display()), false),
            Err(e) => self.set_notice(format!("Couldn't keep {option}: {e}"), true),
        }
    }

    pub fn collab_withdraw(&mut self, message: &str) {
        let Some(t) = &self.collab.thread else { return };
        match thread::withdraw(t, message, "") {
            Ok(id) => self.set_notice(format!("Withdrew {id}"), false),
            Err(e) => self.set_notice(format!("Can't withdraw: {e}"), true),
        }
        self.poll_collab(true);
    }
}

/// What history tracks, for a scene that arrives whole.
fn snapshot_of(scene: Scene) -> EditSnapshot {
    EditSnapshot {
        transform_enabled: vec![true; scene.transforms.len()],
        point_size: scene.point_size,
        color_falloff: scene.color_falloff,
        color_contrast: scene.color_contrast,
        haze_amount: scene.haze,
        exposure: scene.exposure,
        scene,
    }
}
