//! Collaborate: the scene's thread with an agent (AGENT-COLLAB-PLAN.md §4.3).
//!
//! The thread, oldest first — requests as text, proposals as a row of option
//! cards, responses as a line under what they answer — over a composer. Every
//! option has the same verbs whoever sent it: **Preview** (the option on
//! screen, the live scene set aside; hold *Compare* to see the live scene
//! again), **Apply** (its changes laid over the live scene, one Ctrl+Z),
//! **Send back** (a note, and the tweak if you edited it while previewing),
//! **Reject**, and **Keep** (copy it into `scenes/`).
//!
//! Nothing here needs an agent to be running: messages are files under
//! `renders/collab/`, and an agent reads and answers them with
//! `fracturize --thread`, `--propose`, `--respond` and `--wait-reply`. The
//! model is `app_collab`; the format is `thread`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::thread::{self, Kind, Message, Thread, Verdict};

use super::hints::hinted;
use super::icons;

/// A card's picture, in points.
const CARD_W: f32 = 176.0;
const CARD_H: f32 = 99.0;

/// The window's own state: the composer, the note being written for a
/// verb, and pictures and summaries already worked out.
#[derive(Default)]
pub struct CollabUi {
    pub composer: String,
    /// Attach a render of the view to what's sent.
    pub render: bool,
    /// Say which transform is selected.
    pub selection: bool,
    /// The note being written for Send back or Reject: (message, option, verb).
    note_for: Option<(String, String, Verdict)>,
    note_text: String,
    /// Pictures by file, `None` for one that wouldn't load.
    textures: HashMap<PathBuf, Option<egui::TextureHandle>>,
    /// "3 values, +1 transform" per (message folder, option, tweaked), and the matrix
    /// generation a tweak's was counted at.
    summaries: HashMap<(String, String, bool), (u64, String)>,
}

impl CollabUi {
    pub fn new() -> Self {
        Self { render: true, ..Default::default() }
    }
}

/// What the window asked for this frame, done after it's drawn.
enum Do {
    Preview(String, String),
    Leave,
    Apply(String, String),
    Respond(String, String, Verdict, String),
    Keep(String, String),
    DropTweak(String, String),
    Withdraw(String),
    Request(String),
    Propose(String),
}

pub fn draw(ctx: &egui::Context, app: &mut App) {
    app.poll_collab(false);
    draw_conflicts(ctx, app);
    if !app.ui_state.panels.collab_open {
        // Let go of "compare" even with the window shut.
        app.set_preview_blink(false);
        return;
    }
    let mut open = true;
    let mut todo = Vec::new();
    let mut blink = false;

    super::window(ctx, app, super::WindowKey::Collab, "Collaborate")
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Panel::top("fracturize_collab_head").show(ui, |ui| {
                header(ui, app, &mut todo, &mut blink);
                ui.add_space(2.0);
            });
            egui::Panel::bottom("fracturize_collab_composer").show(ui, |ui| {
                ui.add_space(4.0);
                composer(ui, app, &mut todo);
                ui.add_space(4.0);
            });
            messages(ui, app, &mut todo);
        });
    super::remember(ctx, app, super::WindowKey::Collab);
    app.ui_state.panels.collab_open = open;

    app.set_preview_blink(blink);
    for d in todo {
        match d {
            Do::Preview(m, o) => app.start_preview(&m, &o),
            Do::Leave => app.leave_preview(),
            Do::Apply(m, o) => app.collab_apply(&m, &o),
            Do::Respond(m, o, v, note) => app.respond(&m, &o, v, &note),
            Do::Keep(m, o) => app.collab_keep(&m, &o),
            Do::DropTweak(m, o) => {
                app.drop_tweak(&m, &o);
                let dir = app.collab_dir_shown().join(&m).display().to_string();
                app.ui_state.collab.summaries.remove(&(dir, o, true));
            }
            Do::Withdraw(m) => app.collab_withdraw(&m),
            Do::Request(text) => {
                let (render, sel) = (app.ui_state.collab.render, app.ui_state.collab.selection);
                app.collab_request(&text, render, sel);
                app.ui_state.collab.composer.clear();
            }
            Do::Propose(text) => {
                let render = app.ui_state.collab.render;
                app.collab_propose_mine(&text, render);
                app.ui_state.collab.composer.clear();
            }
        }
    }
}

fn header(ui: &mut egui::Ui, app: &mut App, todo: &mut Vec<Do>, blink: &mut bool) {
    let dir = app.collab_dir_shown();
    let resp = ui.add(
        egui::Label::new(egui::RichText::new(format!("Thread: {}", dir.display())).small().weak())
            .sense(egui::Sense::hover()),
    );
    hinted(
        resp,
        &mut app.ui_state,
        "Where this scene's messages are kept: one folder each, under the gitignored \
         renders/. An agent reads them with  fracturize -s <scene> --thread  and answers \
         with --propose or --respond.",
        "this scene's thread, on disk",
    );
    let Some(label) = app.preview_label() else { return };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).strong().color(ui.visuals().warn_fg_color));
        let resp = ui.add(egui::Button::new("Hold to compare").sense(egui::Sense::click_and_drag()));
        *blink = resp.is_pointer_button_down_on();
        hinted(
            resp,
            &mut app.ui_state,
            "Press and hold to see your scene instead of the option; let go to come back. \
             The cloud re-forms each way, so give it a moment.",
            "hold: show your own scene while held",
        );
        let resp = ui.button((icons::X, "Leave preview"));
        if hinted(
            resp,
            &mut app.ui_state,
            "Back to your scene (Esc). Edits made while previewing are kept as a tweak of \
             the option, on its card.",
            "click: back to your scene",
        )
        .clicked()
        {
            todo.push(Do::Leave);
        }
    });
}

fn composer(ui: &mut egui::Ui, app: &mut App, todo: &mut Vec<Do>) {
    let resp = ui.add(
        egui::TextEdit::multiline(&mut app.ui_state.collab.composer)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("Ask for a change, or say what you'd like done with yours"),
    );
    hinted(resp, &mut app.ui_state, "What to send", "type a message");
    ui.horizontal(|ui| {
        let resp = ui.checkbox(&mut app.ui_state.collab.render, "with a render");
        hinted(
            resp,
            &mut app.ui_state,
            "Send a picture of the view as it is now with the message",
            "toggle: attach a render of the view",
        );
        let selected = app.selected_transform().is_some();
        let resp = ui.add_enabled(selected, egui::Checkbox::new(&mut app.ui_state.collab.selection, "about the selection"));
        hinted(
            resp,
            &mut app.ui_state,
            if selected {
                "Say which transform is selected, so the request can be about it"
            } else {
                "Select a transform to say the request is about it"
            },
            "toggle: name the selected transform in the request",
        );
    });
    let empty = app.ui_state.collab.composer.trim().is_empty();
    let previewing = app.previewing().is_some();
    ui.horizontal(|ui| {
        let resp = ui.add_enabled(!empty, egui::Button::new((icons::ARROW, "Send request")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if empty {
                "Write what you'd like changed first"
            } else {
                "Ask for a change. The scene goes with it as it is now, so whatever comes \
                 back can be laid over your later edits."
            },
            "click: send a request",
        );
        if resp.clicked() {
            todo.push(Do::Request(app.ui_state.collab.composer.trim().to_string()));
        }
        let resp = ui.add_enabled(!previewing, egui::Button::new((icons::SHAPES, "Propose my edit")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if previewing {
                "Leave the preview first: this sends your own scene"
            } else {
                "Send your scene as it is now as a proposal, against the scene as last saved: \
                 an answer to a request waiting on you, or \"what would you do with this?\""
            },
            "click: propose the scene as it is",
        );
        if resp.clicked() {
            todo.push(Do::Propose(app.ui_state.collab.composer.trim().to_string()));
        }
    });
}

fn messages(ui: &mut egui::Ui, app: &mut App, todo: &mut Vec<Do>) {
    let Some(thread) = app.collab.thread.take() else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "No messages about this scene yet. Send a request below, then point an agent \
                 at it:\n\n  fracturize -s <scene> --thread\n  fracturize -s <scene> --wait-reply",
            )
            .weak(),
        );
        return;
    };
    let me = app.collab_me();
    egui::ScrollArea::vertical()
        .id_salt("fracturize_collab_thread")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for m in &thread.messages {
                message(ui, app, &thread, m, &me, todo);
                ui.separator();
            }
            for (id, e) in &thread.broken {
                ui.colored_label(ui.visuals().error_fg_color, format!("{id}: can't read it: {e}"));
            }
        });
    app.collab.thread = Some(thread);
}

fn message(ui: &mut egui::Ui, app: &mut App, thread: &Thread, m: &Message, me: &str, todo: &mut Vec<Do>) {
    let h = &m.header;
    let mine = h.from == me;
    let state = if m.withdrawn.is_some() {
        "withdrawn"
    } else if h.kind == Kind::Response {
        ""
    } else if thread.waiting(m) {
        if mine { "waiting on them" } else { "waiting on you" }
    } else {
        "answered"
    };
    ui.horizontal(|ui| {
        let who = egui::RichText::new(format!("{} · {} · {}", h.from, h.kind.name(), m.id)).small();
        ui.label(if mine { who.weak() } else { who.strong() });
        ui.label(egui::RichText::new(thread::when(h.created)).small().weak());
        if !state.is_empty() {
            let color = if state == "waiting on you" { ui.visuals().warn_fg_color } else { ui.visuals().weak_text_color() };
            ui.label(egui::RichText::new(state).small().color(color));
        }
        if mine && thread.waiting(m) {
            let resp = ui.small_button("Withdraw");
            if hinted(
                resp,
                &mut app.ui_state,
                "Take this back. It stays in the thread, greyed, so nobody answers into nothing.",
                "click: withdraw this message",
            )
            .clicked()
            {
                todo.push(Do::Withdraw(m.id.clone()));
            }
        }
    });
    let faded = m.withdrawn.is_some();
    let text = h.message.trim();
    if !text.is_empty() {
        let rt = egui::RichText::new(text);
        ui.label(if faded { rt.weak() } else { rt });
    }
    if let Some(w) = m.withdrawn.as_ref().filter(|w| !w.reason.is_empty()) {
        ui.label(egui::RichText::new(format!("withdrawn: {}", w.reason)).small().weak());
    }
    if let Some(r) = &h.revises {
        let (msg, opt) = r.split_once('/').unwrap_or((r.as_str(), ""));
        ui.label(egui::RichText::new(format!("a new round of {opt} from {msg}")).small().weak());
    }
    if !h.selection.is_empty() {
        ui.label(egui::RichText::new(format!("about {}", h.selection.join(", "))).small().weak());
    }
    if let Some(r) = &h.render {
        picture(ui, app, &m.file(r));
    }
    for a in &h.answers {
        let mut line = format!("{} {}", a.option, a.verdict.name());
        if !a.note.is_empty() {
            line.push_str(&format!(": {}", a.note));
        }
        if a.tweak.is_some() {
            line.push_str(" (with a tweak)");
        }
        ui.label(egui::RichText::new(line).small());
    }
    if h.kind == Kind::Proposal && !h.options.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for o in &h.options {
                card(ui, app, thread, m, o, todo);
            }
        });
    }
}

fn card(ui: &mut egui::Ui, app: &mut App, thread: &Thread, m: &Message, o: &thread::Opt, todo: &mut Vec<Do>) {
    let key = (m.id.clone(), o.id.clone());
    let previewing = app.previewing().is_some_and(|(pm, po)| pm == m.id && po == o.id);
    let tweaked = app.collab.tweaks.contains_key(&key) || (previewing && app.preview_tweaked());
    let verdict = thread.verdict(&m.id, &o.id).map(|(_, a)| a.verdict);
    let live = m.withdrawn.is_none();
    let frame = egui::Frame::group(ui.style()).fill(if previewing {
        ui.visuals().selection.bg_fill.gamma_multiply(0.35)
    } else {
        ui.visuals().faint_bg_color
    });
    frame.show(ui, |ui| {
        ui.set_width(CARD_W);
        ui.vertical(|ui| {
            match &o.render {
                Some(r) => picture(ui, app, &m.file(r)),
                None => {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(CARD_W, CARD_H), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "no render",
                        egui::FontId::proportional(11.0),
                        ui.visuals().weak_text_color(),
                    );
                }
            }
            let title = if o.note.is_empty() { o.id.clone() } else { format!("{} · {}", o.id, o.note) };
            ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
            let summary = summary(app, m, o);
            let resp = ui.add(egui::Label::new(egui::RichText::new(&summary).small().weak()).truncate().sense(egui::Sense::hover()));
            hinted(resp, &mut app.ui_state, format!("What it changes from the scene it was made from: {summary}"), "the option's changes");
            if tweaked {
                let t = if previewing { "tweaking…".to_string() } else { format!("tweaked: {}", summary_tweak(app, m, o)) };
                ui.label(egui::RichText::new(t).small().color(ui.visuals().warn_fg_color));
            }
            if let Some(v) = verdict {
                ui.label(egui::RichText::new(v.name()).small());
            }
            if !live {
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let resp = ui.selectable_label(previewing, "Preview");
                if hinted(
                    resp,
                    &mut app.ui_state,
                    "Put this option on screen; your scene is set aside, with its undo history, \
                     until you leave (Esc). Edits made here are a tweak of the option.",
                    "click: preview this option",
                )
                .clicked()
                {
                    todo.push(if previewing { Do::Leave } else { Do::Preview(m.id.clone(), o.id.clone()) });
                }
                let resp = ui.button("Apply");
                if hinted(
                    resp,
                    &mut app.ui_state,
                    "Lay this option's changes (or your tweak's) over your scene: edits you made \
                     since keep. One Ctrl+Z takes it back. Where you both changed something, \
                     you choose.",
                    "click: apply this option to your scene",
                )
                .clicked()
                {
                    todo.push(Do::Apply(m.id.clone(), o.id.clone()));
                }
                let resp = ui.button("Send back");
                if hinted(
                    resp,
                    &mut app.ui_state,
                    "Ask for another round of this one, with a note, and your tweak if you made one",
                    "click: send this option back with a note",
                )
                .clicked()
                {
                    open_note(app, m, o, Verdict::SentBack);
                }
                let resp = ui.button("Reject");
                if hinted(resp, &mut app.ui_state, "Say no to this one, with a reason if you like", "click: reject this option")
                    .clicked()
                {
                    open_note(app, m, o, Verdict::Rejected);
                }
                let resp = ui.button("Keep");
                if hinted(
                    resp,
                    &mut app.ui_state,
                    "Save this option (or your tweak of it) as a new file in scenes/. Your \
                     scene isn't touched.",
                    "click: keep this option as a scene file",
                )
                .clicked()
                {
                    todo.push(Do::Keep(m.id.clone(), o.id.clone()));
                }
                if tweaked && !previewing {
                    let resp = ui.small_button("Drop tweak");
                    if hinted(resp, &mut app.ui_state, "Forget your edits to this option", "click: forget the tweak")
                        .clicked()
                    {
                        todo.push(Do::DropTweak(m.id.clone(), o.id.clone()));
                    }
                }
            });
            note_editor(ui, app, m, o, todo);
        });
    });
}

fn open_note(app: &mut App, m: &Message, o: &thread::Opt, v: Verdict) {
    let c = &mut app.ui_state.collab;
    c.note_for = Some((m.id.clone(), o.id.clone(), v));
    c.note_text.clear();
}

fn note_editor(ui: &mut egui::Ui, app: &mut App, m: &Message, o: &thread::Opt, todo: &mut Vec<Do>) {
    let Some((nm, no, v)) = app.ui_state.collab.note_for.clone() else { return };
    if nm != m.id || no != o.id {
        return;
    }
    let hint = if v == Verdict::SentBack { "What to change" } else { "Why (optional)" };
    let resp = ui.add(egui::TextEdit::multiline(&mut app.ui_state.collab.note_text).desired_rows(2).hint_text(hint));
    hinted(resp, &mut app.ui_state, hint, "type a note");
    ui.horizontal(|ui| {
        let label = if v == Verdict::SentBack { "Send back" } else { "Reject" };
        let resp = ui.button(label);
        if hinted(resp, &mut app.ui_state, "Send this answer", "click: send").clicked() {
            let note = app.ui_state.collab.note_text.trim().to_string();
            todo.push(Do::Respond(m.id.clone(), o.id.clone(), v, note));
            app.ui_state.collab.note_for = None;
        }
        let resp = ui.button("Cancel");
        if hinted(resp, &mut app.ui_state, "Don't send anything", "click: cancel").clicked() {
            app.ui_state.collab.note_for = None;
        }
    });
}

/// The option's changes from the base, counted once.
fn summary(app: &mut App, m: &Message, o: &thread::Opt) -> String {
    let key = (m.dir.display().to_string(), o.id.clone(), false);
    if let Some((_, s)) = app.ui_state.collab.summaries.get(&key) {
        return s.clone();
    }
    let s = match (m.base(), m.option_scene(o)) {
        (Some(Ok(b)), Ok(s)) => thread::change_summary(&b, &s),
        (None, Ok(_)) => "no base to compare with".to_string(),
        (Some(Err(e)), _) | (_, Err(e)) => format!("doesn't load: {e}"),
    };
    app.ui_state.collab.summaries.insert(key, (0, s.clone()));
    s
}

/// The tweak's changes from the option, counted again when the scene moves.
fn summary_tweak(app: &mut App, m: &Message, o: &thread::Opt) -> String {
    let key = (m.dir.display().to_string(), o.id.clone(), true);
    let generation = app.matrix_generation();
    if let Some((g, s)) = app.ui_state.collab.summaries.get(&key)
        && *g == generation
    {
        return s.clone();
    }
    let s = match (app.collab.tweaks.get(&(m.id.clone(), o.id.clone())), m.option_scene(o)) {
        (Some(t), Ok(s)) => thread::change_summary(&s, t),
        _ => String::new(),
    };
    app.ui_state.collab.summaries.insert(key, (generation, s.clone()));
    s
}

/// A render, scaled to a card.
fn picture(ui: &mut egui::Ui, app: &mut App, path: &Path) {
    let cache = &mut app.ui_state.collab.textures;
    let tex = cache
        .entry(path.to_path_buf())
        .or_insert_with(|| {
            let img = image::open(path).ok()?.thumbnail(CARD_W as u32 * 2, CARD_H as u32 * 2).to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            Some(ui.ctx().load_texture(path.display().to_string(), color, egui::TextureOptions::LINEAR))
        })
        .clone();
    match tex {
        Some(t) => {
            let s = t.size_vec2();
            let scale = (CARD_W / s.x).min(CARD_H / s.y);
            let resp = ui.add(egui::Image::new(&t).fit_to_exact_size(s * scale).sense(egui::Sense::hover()));
            hinted(resp, &mut app.ui_state, path.display().to_string(), "a render that came with the message");
        }
        None => {
            ui.label(egui::RichText::new(format!("{} won't open", path.display())).small().weak());
        }
    }
}

/// Apply's question when both sides changed the same thing.
fn draw_conflicts(ctx: &egui::Context, app: &mut App) {
    let Some(p) = &app.collab.pending_apply else { return };
    let title = format!("Apply {}: you both changed {}", p.option, if p.conflicts.len() == 1 { "one thing" } else { "some things" });
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new(title)
        .id(egui::Id::new("fracturize_collab_conflicts"))
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("Since the request was sent, you and the proposal changed these differently. Pick a side for each.");
            ui.add_space(4.0);
            let Some(p) = &mut app.collab.pending_apply else { return };
            egui::Grid::new("fracturize_collab_conflict_grid").striped(true).num_columns(3).show(ui, |ui| {
                ui.label(egui::RichText::new("value").strong());
                ui.label(egui::RichText::new("yours").strong());
                ui.label(egui::RichText::new("theirs").strong());
                ui.end_row();
                for (c, take) in p.conflicts.iter().zip(p.take_theirs.iter_mut()) {
                    ui.label(egui::RichText::new(&c.path).monospace());
                    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".into());
                    let resp = ui.radio(!*take, show(&c.mine));
                    if hinted(resp, &mut app.ui_state, "Keep your value", "click: keep yours").clicked() {
                        *take = false;
                    }
                    let resp = ui.radio(*take, show(&c.theirs));
                    if hinted(resp, &mut app.ui_state, "Take the proposal's value", "click: take theirs").clicked() {
                        *take = true;
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let resp = ui.button("Apply");
                apply = hinted(resp, &mut app.ui_state, "Apply the option with these choices: one edit, one Ctrl+Z", "click: apply").clicked();
                let resp = ui.button("Cancel");
                cancel = hinted(resp, &mut app.ui_state, "Apply nothing (Esc)", "click: cancel").clicked();
            });
        });
    if apply {
        app.finish_apply();
    } else if cancel {
        app.collab.pending_apply = None;
    }
}
