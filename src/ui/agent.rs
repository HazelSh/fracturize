//! Who answers the Collaborate window (AGENT-COLLAB-PLAN.md §4.3, §4.6): the
//! connection light at the top of it, and the Agent settings window.
//!
//! The light says, in a line, who is listening and what they're doing: a
//! connected Claude session (green), a backend working on a message
//! (amber, with Stop beside it), a session that has gone quiet on a waiting
//! message ("check its terminal"), or nobody, with what happens next. It
//! never needs an agent to be running: with nobody connected, messages wait
//! on disk.
//!
//! Agent settings picks the backend and holds each one's few settings, with
//! a **Test** for each: connect, ask for one word, and show what came back,
//! or the error.

use std::sync::mpsc::{self, Receiver};

use crate::agent::{Backend, Vision};
use crate::app::App;
use crate::app::agent::LightState;

use super::hints::hinted;
use super::icons;

#[derive(Default)]
pub struct AgentUi {
    pub settings_open: bool,
    test: Option<Receiver<String>>,
    test_result: Option<String>,
    /// with-llm's word on the preset: (preset, need and free MiB or why not).
    fits: Option<(String, Result<(u64, u64), String>)>,
}

/// The light, in the Collaborate window's header.
pub fn light_row(ui: &mut egui::Ui, app: &mut App) {
    let light = app.agent_light();
    ui.horizontal(|ui| {
        let color = match light.state {
            LightState::Connected => egui::Color32::from_rgb(90, 200, 110),
            LightState::Working | LightState::Waiting => ui.visuals().warn_fg_color,
            LightState::Off => ui.visuals().weak_text_color(),
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        let resp = ui.add(egui::Label::new(egui::RichText::new(&light.text).small()).truncate().sense(egui::Sense::hover()));
        hinted(
            resp,
            &mut app.ui_state,
            "Who answers your messages. Agent settings chooses: a Claude Code session you started, \
             Claude run here without a terminal, a local model, or files only.",
            "who answers your messages",
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.small_button((icons::SLIDERS, "Agent"));
            if hinted(resp, &mut app.ui_state, "Agent settings: who answers, and how to reach them", "click: agent settings")
                .clicked()
            {
                let s = &mut app.ui_state.collab.agent.settings_open;
                *s = !*s;
            }
            if light.busy {
                let resp = ui.small_button((icons::X, "Stop"));
                if hinted(
                    resp,
                    &mut app.ui_state,
                    "End this request now. A local model stops after the reply it's writing; what it \
                     has already sent stays in the thread.",
                    "click: stop the agent",
                )
                .clicked()
                {
                    app.stop_agent();
                }
            }
        });
    });
}

pub fn draw_settings(ctx: &egui::Context, app: &mut App) {
    if !app.ui_state.collab.agent.settings_open {
        return;
    }
    if let Some(rx) = &app.ui_state.collab.agent.test
        && let Ok(r) = rx.try_recv()
    {
        app.ui_state.collab.agent.test_result = Some(r);
        app.ui_state.collab.agent.test = None;
    }
    let mut open = true;
    egui::Window::new("Agent settings")
        .id(egui::Id::new("fracturize_agent_settings"))
        .open(&mut open)
        .resizable(true)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Who answers").strong());
            let current = app.agent_prefs_mut().backend;
            let chosen = super::radio::radio(&mut app.ui_state, "agent_backend", current)
                .option(
                    Backend::ClaudeChannel,
                    Backend::ClaudeChannel.label(),
                    "A Claude Code session you started in a terminal, listening on the fracturize channel. \
                     Your messages reach it as they're sent.",
                    "click: a Claude Code session answers",
                )
                .option(
                    Backend::ClaudeHeadless,
                    Backend::ClaudeHeadless.label(),
                    "Fracturize runs Claude itself for each message, resuming one session so it remembers. \
                     No terminal needed; each message starts cold.",
                    "click: Claude runs headless per message",
                )
                .option(
                    Backend::Local,
                    Backend::Local.label(),
                    "A model on llama-server on this machine, for a few turns a message, then a short report.",
                    "click: a local model answers",
                )
                .option(
                    Backend::None,
                    Backend::None.label(),
                    "Nothing is started. Messages wait as files, for an agent that reads them with the CLI.",
                    "click: files only",
                )
                .show(ui);
            if let Some(b) = chosen {
                app.set_agent_backend(b);
                app.ui_state.collab.agent.test_result = None;
            }
            ui.add_space(6.0);
            ui.separator();
            match app.agent_prefs_mut().backend {
                Backend::ClaudeChannel => channel(ui, app),
                Backend::ClaudeHeadless => headless(ui, app),
                Backend::Local => local(ui, app),
                Backend::None => {
                    ui.label("An agent takes part with the CLI alone:");
                    code(ui, app, "fracturize -s <scene> --thread\nfracturize -s <scene> --wait-reply\nfracturize -s <scene> --propose <dir>");
                }
            }
            if let Some(r) = &app.ui_state.collab.agent.test_result {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(r).small());
            }
        });
    if !open {
        app.ui_state.collab.agent.settings_open = false;
    }
}

fn code(ui: &mut egui::Ui, app: &mut App, text: &str) {
    ui.horizontal(|ui| {
        let mut t = text.to_string();
        ui.add(egui::TextEdit::multiline(&mut t).code_editor().desired_rows(text.lines().count()).desired_width(380.0).interactive(true));
        let resp = ui.small_button("Copy");
        if hinted(resp, &mut app.ui_state, "Copy these commands", "click: copy").clicked() {
            ui.ctx().copy_text(text.to_string());
        }
    });
}

fn test_button(ui: &mut egui::Ui, app: &mut App, tooltip: &str, run: impl FnOnce() -> String + Send + 'static) {
    let busy = app.ui_state.collab.agent.test.is_some();
    let resp = ui.add_enabled(!busy, egui::Button::new(if busy { "Testing…" } else { "Test" }));
    if hinted(resp, &mut app.ui_state, tooltip, "click: test this backend").clicked() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run());
        });
        app.ui_state.collab.agent.test = Some(rx);
        app.ui_state.collab.agent.test_result = None;
    }
}

fn channel(ui: &mut egui::Ui, app: &mut App) {
    ui.label("Once, in the project:");
    code(ui, app, "claude mcp add --scope project fracturize -- fracturize mcp");
    ui.label("Then start a session that listens, and tell it what you're working on:");
    let name = app.agent_prefs_mut().claude.session_name.clone();
    code(ui, app, &format!("claude -n {name} --dangerously-load-development-channels server:fracturize"));
    ui.label(
        egui::RichText::new(
            "The flag is a research preview's and may change; Claude, headless works without it.",
        )
        .small()
        .weak(),
    );
    let connected = app.agent.bridge.as_ref().map(|b| (b.name.clone(), b.pid, b.since.elapsed().as_secs()));
    match connected {
        Some((n, pid, s)) => {
            ui.label(format!("{n} is connected (pid {pid}, for {}:{:02}).", s / 60, s % 60));
        }
        None => {
            ui.label(egui::RichText::new("No session is connected.").weak());
        }
    }
}

fn headless(ui: &mut egui::Ui, app: &mut App) {
    ui.label("Each message you send runs Claude Code here, without a terminal, as:");
    let args = &app.agent_prefs_mut().claude;
    let resume = match &args.session_id {
        Some(id) => format!("--resume {id}"),
        None => format!("-n {}", args.session_name),
    };
    let cmd = format!("claude -p {resume} --permission-mode auto --mcp-config <fracturize mcp> …");
    ui.label(egui::RichText::new(cmd).monospace().small());
    ui.horizontal(|ui| {
        let has = app.agent_prefs_mut().claude.session_id.is_some();
        let resp = ui.add_enabled(has, egui::Button::new("Start a new session"));
        if hinted(
            resp,
            &mut app.ui_state,
            if has {
                "Forget the session it resumes, so the next message starts one with nothing remembered"
            } else {
                "There's no session yet: the first message starts one"
            },
            "click: start afresh next time",
        )
        .clicked()
        {
            app.forget_headless_session();
        }
        test_button(ui, app, "Run claude -p once and ask for one word", || {
            match std::process::Command::new("claude").args(["-p", "Reply with the single word: ready"]).output() {
                Ok(o) if o.status.success() => format!("Claude said: {}", String::from_utf8_lossy(&o.stdout).trim()),
                Ok(o) => format!("claude failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                Err(e) => format!("couldn't run claude: {e}"),
            }
        });
    });
}

fn local(ui: &mut egui::Ui, app: &mut App) {
    let mut changed = false;
    egui::Grid::new("fracturize_agent_local").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Server");
        let p = &mut app.agent_prefs_mut().local;
        changed |= ui.add(egui::TextEdit::singleline(&mut p.base_url).desired_width(260.0)).changed();
        ui.end_row();
        ui.label("Preset");
        let resp = ui.add(egui::TextEdit::singleline(&mut app.agent_prefs_mut().local.preset).desired_width(160.0).hint_text("none: never start one"));
        changed |= resp.changed();
        hinted(
            resp,
            &mut app.ui_state,
            "The with-llm preset (in ~/code/lll/models.conf) to start when nothing answers at the server. \
             Leave it empty to only ever use a server you started.",
            "the model to start",
        );
        ui.end_row();
        ui.label("Model");
        let resp = ui.add(egui::TextEdit::singleline(&mut app.agent_prefs_mut().local.model).desired_width(160.0).hint_text("what the server reports"));
        changed |= resp.changed();
        hinted(resp, &mut app.ui_state, "The model name to ask for. Empty: whatever the server is running.", "the model's name");
        ui.end_row();
        ui.label("Turns");
        let resp = super::num::drag(ui, 3, 0, super::num::Drag::new(&mut app.agent_prefs_mut().local.max_turns).range(1..=20).speed(0.05));
        changed |= resp.changed();
        hinted(
            resp,
            &mut app.ui_state,
            "Replies the model gets per message before it's asked to report back. Your reply to it starts a fresh budget.",
            "drag: turns per message",
        );
        ui.end_row();
        ui.label("Sees renders");
        let v = app.agent_prefs_mut().local.vision;
        let chosen = super::radio::radio(&mut app.ui_state, "agent_vision", v)
            .option(Vision::Auto, "If it can", "Send renders when the server says the model has vision", "click: renders if it can see")
            .option(Vision::On, "Always", "Always send renders", "click: always send renders")
            .option(Vision::Off, "Never", "Never send renders", "click: never send renders")
            .show(ui);
        if let Some(v) = chosen {
            app.agent_prefs_mut().local.vision = v;
            changed = true;
        }
        ui.end_row();
    });
    ui.horizontal(|ui| {
        let resp = ui.checkbox(&mut app.agent_prefs_mut().local.thinking, "Thinking");
        changed |= hinted(resp, &mut app.ui_state, "Ask the model to reason before it answers", "toggle: thinking").changed();
        let resp = ui.checkbox(&mut app.agent_prefs_mut().local.grammar, "Grammar");
        changed |= hinted(
            resp,
            &mut app.ui_state,
            "Constrain replies to tools/llm-author/scene.gbnf, so every scene parses. Some models think worse under it.",
            "toggle: grammar-constrained replies",
        )
        .changed();
    });

    // with-llm's own VRAM rule, asked again when the preset changes.
    let preset = app.agent_prefs_mut().local.preset.clone();
    if !preset.is_empty() && app.ui_state.collab.agent.fits.as_ref().is_none_or(|(p, _)| *p != preset) {
        let r = crate::agent::fits(&app.agent_prefs_mut().local);
        app.ui_state.collab.agent.fits = Some((preset.clone(), r));
    }
    if let Some((_, r)) = &app.ui_state.collab.agent.fits
        && !preset.is_empty()
    {
        let text = match r {
            Ok((need, 0)) => format!("{preset} needs about {need} MiB of video memory"),
            Ok((need, free)) if free >= need => format!("{preset} needs about {need} MiB of video memory; {free} MiB are free"),
            Ok((need, free)) => format!("{preset} needs about {need} MiB of video memory, but only {free} MiB are free: it won't start"),
            Err(e) => format!("with-llm: {e}"),
        };
        ui.label(egui::RichText::new(text).small());
    }
    ui.horizontal(|ui| {
        let has = app.has_local_conversation();
        let resp = ui.add_enabled(has, egui::Button::new("Forget this conversation"));
        if hinted(
            resp,
            &mut app.ui_state,
            if has {
                "The model remembers this thread's earlier messages while the window is open; start it afresh"
            } else {
                "Nothing to forget yet"
            },
            "click: start the model afresh",
        )
        .clicked()
        {
            app.forget_local_conversation();
        }
        let url = app.agent_prefs_mut().local.base_url.clone();
        test_button(ui, app, "Ask the server for one word (it must already be running)", move || local_test(&url));
    });
    if changed {
        app.save_agent_prefs();
        app.ui_state.collab.agent.fits = None;
    }
}

fn local_test(url: &str) -> String {
    let base = url.trim_end_matches('/');
    let http = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let body = serde_json::json!({
        "messages": [{ "role": "user", "content": "Reply with the single word: ready" }],
        "max_tokens": 512,
        "chat_template_kwargs": { "enable_thinking": false },
    });
    match http.post(&format!("{base}/chat/completions")).send_json(&body) {
        Ok(mut r) => match r.body_mut().read_json::<serde_json::Value>() {
            Ok(v) => match v["choices"][0]["message"]["content"].as_str() {
                Some(t) => format!("{} said: {}", v["model"].as_str().unwrap_or("the model"), t.trim()),
                None => format!("the server answered oddly: {v}"),
            },
            Err(e) => format!("the server answered oddly: {e}"),
        },
        Err(e) => format!("nothing answers at {base}: {e}"),
    }
}
