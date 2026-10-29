//! The render-job dialog: set up a batch render, watch it, see what it made.
//!
//! Everything here exists because a big render is a commitment. The old HQ
//! render was a button with no parameters and no exit; this asks what you
//! want, tells you what it will cost before you agree to it, shows where it
//! has got to, takes two clicks to throw away, and when it's done, stays on the
//! result — one click opens it — until you ask for the next one.
//!
//! Three stages, one window: **setup** (the form), **running**, and
//! **finished**. Finished is the running view with the bar replaced by the
//! result, so the log stays where you were reading it. It used to drop
//! straight back to the form with a small notice above Start, which put the
//! answer to "did it work, and where is it?" in among the next job's settings.
//!
//! The form is in sections, ordered by the questions in the order people ask
//! them: what am I making (Output), how big (Size), how good (Quality), and
//! then the things that are inherited or remembered rather than chosen here
//! (Look, from the window; This machine, from prefs). A **preset** above them
//! sets Output, Size and Quality in one go (`crate::render_preset`).
//!
//! Quality settings are job-scoped — changing them here doesn't touch the
//! interactive point count or prefs, so a 100M-point batch and a comfortable
//! exploration buffer can coexist. The form keeps its settings between
//! openings for the rest of the session; the first opening starts from the
//! preset last chosen.

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::render_job::{
    format_duration, format_duration_cell, format_estimate, human_bytes, FinishedJob, JobKind, JobParams,
    Samples,
};
use crate::render_preset::{self, Preset};

use super::confirm::Arm;
use super::hints::hinted;
use super::icons;
use super::num;
use super::table::{self, Col};
use super::UiState;

/// Character budget for the elapsed/remaining readout: two seven-character
/// duration cells plus `" elapsed · about "` and `" left"`. Sized to the
/// longer of the two forms so the countdown appearing at 5% doesn't resize
/// the line it lands in.
const ELAPSED_CHARS: usize = 36;

/// "Done" reads the same everywhere: the theme's success green.
const DONE_GREEN: egui::Color32 = super::theme::SUCCESS;

/// Width of a table's value column: room for a slider and its number.
const VALUE_W: f32 = 210.0;

/// Section ids, kept apart from other windows' sections of the same name.
const SECTIONS: &str = "fracturize_render_job_section";

/// Output sizes, smallest first.
///
/// The low end matters more than the high end: most renders are a check on
/// whether the framing and the point count are right, and a 640x480 answers
/// that in a fraction of the time. The list used to start at 720p, which made
/// the cheapest available answer four times the cost of the one you wanted.
const SIZE_PRESETS: [(&str, u32, u32); 8] = [
    ("SD", 640, 480),
    ("480p", 854, 480),
    ("720p", 1280, 720),
    ("1080p", 1920, 1080),
    ("square 1080", 1080, 1080),
    ("1440p", 2560, 1440),
    ("4K", 3840, 2160),
    ("A3 at 300 ppi", 4961, 3508),
];

/// The dialog's form. Kept whole rather than derived from `JobParams` so
/// switching modes doesn't lose the other mode's settings.
#[derive(Clone)]
pub struct RenderJobForm {
    pub open: bool,
    /// Whether the form has been filled in this session. The first opening
    /// takes the remembered preset; later ones keep what you left.
    seeded: bool,
    /// Show the finished job rather than the form. Set by Start; New render
    /// clears it.
    showing_result: bool,
    pub mode: Mode,
    pub filename: String,
    /// Set once the user edits the filename, so mode changes stop rewriting it
    pub filename_touched: bool,
    pub width: u32,
    pub height: u32,
    pub points: usize,
    pub accumulate: u32,
    /// Accumulate into a histogram rather than splatting the ring once.
    ///
    /// Kept as a flag beside both of the numbers it chooses between, rather
    /// than as a `Samples` enum on the form, for the same reason `mode` is:
    /// switching back and forth must not lose the other option's setting.
    pub use_accumulate: bool,
    /// Samples per output pixel to accumulate to. Only read when
    /// `use_accumulate` is set.
    pub spp: u32,
    /// Strength of the density-estimation blur, 0-1. Only reaches the renderer
    /// on an accumulating splat render; see `samples`.
    pub density_estimation: f32,
    pub splat: bool,
    pub exposure: f32,
    pub transparent: bool,
    /// Render at N x and filter down. Job-scoped like the point count.
    pub supersample: u32,
    pub filter: crate::gpu::Filter,
    pub filter_radius: f32,
    /// Bits per channel in the PNG. Stills only — animation is 8-bit by codec.
    pub bit_depth: crate::offline::BitDepth,
    pub fps: u32,
    pub seconds: f32,
    pub quality: u8,
    /// Which animation file to write. Only read when `mode` is `Animation`,
    /// but kept across a switch to Still and back so the choice sticks.
    pub format: crate::video::Format,
    /// Fraction of the GPU the render may take. A machine setting, seeded
    /// from prefs in [`open`] and written back when it changes.
    pub gpu_share: f32,
    /// CPU threads the encoder may use. A machine setting like `gpu_share`.
    pub threads: usize,
    /// The preset these settings came from, and what it says: what the
    /// picker's `*` is measured against.
    preset: Option<(render_preset::Entry, Preset)>,
    /// The Save as name being typed, while it is.
    preset_save_as: Option<String>,
    preset_error: Option<String>,
    preset_delete: Arm,
    /// Why opening the finished file failed, if it did.
    open_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Still,
    Animation,
    ViewDescriptor,
}

impl Default for RenderJobForm {
    fn default() -> Self {
        Self {
            open: false,
            seeded: false,
            showing_result: false,
            mode: Mode::Still,
            filename: String::new(),
            filename_touched: false,
            // 720p, not 1440p: the first render of a scene is usually a check
            // rather than a keeper, and a default should cost what the common
            // case is worth. The `preview` preset says the same.
            width: 1280,
            height: 720,
            points: 20_000_000,
            accumulate: 96,
            // On by default. The ring path's density falls as the output grows
            // — 21.7 samples/px at 720p down to 2.4 at 4K on the old defaults —
            // so the previous default got *worse* the bigger you rendered,
            // which is backwards. Falls back to the ring on its own wherever
            // accumulation can't run; see `samples`.
            use_accumulate: true,
            // `medium` on the CLI's `--effort` ladder: the lowest tier that
            // beats the old default at every output size.
            spp: 100,
            // Off by default: it is the most distinctive thing here and also
            // the most opinionated, so it is opt-in rather than something a
            // render quietly arrives with.
            density_estimation: 0.0,
            splat: false,
            exposure: 1.0,
            transparent: false,
            // Same default as `--render`: a filtered 2x image is simply a
            // better picture than a native one of the same scene.
            supersample: 2,
            filter: crate::gpu::Filter::Gaussian,
            filter_radius: crate::gpu::points::downsample::DEFAULT_FILTER_RADIUS,
            bit_depth: crate::offline::BitDepth::Eight,
            fps: 30,
            seconds: 8.0,
            quality: 60,
            format: crate::video::Format::Avif,
            gpu_share: 1.0,
            threads: 1,
            preset: None,
            preset_save_as: None,
            preset_error: None,
            preset_delete: Arm::default(),
            open_error: None,
        }
    }
}

impl RenderJobForm {
    fn kind(&self) -> JobKind {
        match self.mode {
            Mode::Still => JobKind::Still { width: self.width, height: self.height },
            Mode::Animation => JobKind::Animation {
                width: self.width,
                height: self.height,
                fps: self.fps,
                seconds: self.seconds,
                quality: self.quality,
                format: self.format,
            },
            Mode::ViewDescriptor => JobKind::ViewDescriptor,
        }
    }

    /// Why this form can't accumulate, if it can't. Exactly the conditions
    /// [`samples`](Self::samples) enforces, in words for the greyed control.
    fn why_not_accumulate(&self) -> Option<&'static str> {
        if !self.splat {
            Some("Only with the splat renderer (Look, below).")
        } else if self.mode != Mode::Still {
            Some("Only for stills: every frame of an animation has its own camera.")
        } else {
            None
        }
    }

    /// What this form is asking the renderer for.
    ///
    /// The accumulating path is **still + splat only**, and that is enforced
    /// here rather than only in the control that sets the flag. An animation
    /// gives every frame its own camera, so there is no histogram to carry
    /// between them; the points renderer has no histogram at all. The same two
    /// conditions gate `--spp` on the command line, and a form that could
    /// build a `Samples::Accumulate` the renderer would refuse would be a
    /// second place for that rule to drift out of step.
    pub fn samples(&self) -> Samples {
        if self.use_accumulate && self.why_not_accumulate().is_none() {
            Samples::Accumulate { spp: self.spp.max(1) }
        } else {
            Samples::Ring { accumulate: self.accumulate }
        }
    }

    fn accumulating(&self) -> bool {
        self.samples().spp().is_some()
    }

    /// The density estimation this job will actually run.
    ///
    /// Gated on [`samples`](Self::samples) rather than tracked separately,
    /// because DE has no meaning without a histogram: `TARGET_DENSITY` is
    /// calibrated in raw accumulated units, so there must be accumulated units
    /// to read. A one-pass render silently ignoring a non-zero setting would be
    /// worse than the control being off — the slider is disabled and says why.
    pub fn density(&self) -> crate::gpu::points::density::DensityEstimation {
        let amount = if self.accumulating() { self.density_estimation } else { 0.0 };
        crate::gpu::points::density::DensityEstimation { amount }.clamped()
    }

    fn params(&self) -> JobParams {
        JobParams {
            kind: self.kind(),
            out_path: PathBuf::from(self.filename.trim()),
            points: self.points,
            samples: self.samples(),
            density_estimation: self.density(),
            splat: self.splat,
            exposure: self.exposure,
            transparent: self.transparent,
            supersample: self.supersample,
            filter: self.filter,
            filter_radius: self.filter_radius,
            bit_depth: self.bit_depth,
            gpu_share: self.gpu_share,
            threads: self.threads,
        }
    }

    /// Take what `preset` says, leaving everything it doesn't mention.
    fn apply(&mut self, p: &Preset) {
        use render_preset::{Output, Samples as S};
        match p.output {
            Some(Output::Still) => self.mode = Mode::Still,
            Some(Output::Avif) => (self.mode, self.format) = (Mode::Animation, crate::video::Format::Avif),
            Some(Output::Mp4) => (self.mode, self.format) = (Mode::Animation, crate::video::Format::Mp4),
            None => {}
        }
        fn set<T>(slot: &mut T, v: Option<T>) {
            if let Some(v) = v {
                *slot = v;
            }
        }
        set(&mut self.width, p.width);
        set(&mut self.height, p.height);
        set(&mut self.use_accumulate, p.samples.map(|s| s == S::Accumulate));
        set(&mut self.spp, p.spp);
        set(&mut self.accumulate, p.rounds);
        set(&mut self.points, p.points);
        set(&mut self.supersample, p.supersample);
        set(&mut self.filter, p.filter);
        set(&mut self.filter_radius, p.filter_radius.map(|r| r as f32));
        set(&mut self.density_estimation, p.density_estimation.map(|d| d as f32));
        set(
            &mut self.bit_depth,
            p.bit_depth.map(|b| {
                if b == 16 { crate::offline::BitDepth::Sixteen } else { crate::offline::BitDepth::Eight }
            }),
        );
        set(&mut self.fps, p.fps);
        set(&mut self.quality, p.quality);
    }

    /// Everything a preset can say, as this form has it.
    fn to_preset(&self, description: String) -> Preset {
        use render_preset::{Output, Samples as S};
        // Three decimals: what the controls show, and what reads back the same.
        let round = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
        Preset {
            description,
            output: match (self.mode, self.format) {
                (Mode::Still, _) => Some(Output::Still),
                (Mode::Animation, crate::video::Format::Avif) => Some(Output::Avif),
                (Mode::Animation, crate::video::Format::Mp4) => Some(Output::Mp4),
                (Mode::ViewDescriptor, _) => None,
            },
            width: Some(self.width),
            height: Some(self.height),
            samples: Some(if self.use_accumulate { S::Accumulate } else { S::OnePass }),
            spp: Some(self.spp),
            rounds: Some(self.accumulate),
            points: Some(self.points),
            supersample: Some(self.supersample),
            filter: Some(self.filter),
            filter_radius: Some(round(self.filter_radius)),
            density_estimation: Some(round(self.density_estimation)),
            bit_depth: Some(if self.bit_depth == crate::offline::BitDepth::Sixteen { 16 } else { 8 }),
            fps: Some(self.fps),
            quality: Some(self.quality),
        }
    }

    /// Whether the settings differ from what the preset says. Only the keys
    /// it says count: a built-in leaves the point count alone, so changing it
    /// isn't a change to the preset.
    fn preset_modified(&self) -> bool {
        let Some((_, p)) = &self.preset else { return false };
        let mut probe = self.clone();
        probe.apply(p);
        probe.to_preset(String::new()) != self.to_preset(String::new())
    }

    /// One line saying what these settings make, for a preset saved from them.
    fn summary(&self) -> String {
        let (w, h) = (self.width, self.height);
        let mut parts = vec![match self.mode {
            Mode::Animation => format!("{w}×{h} {} at {} fps", self.format.extension().to_uppercase(), self.fps),
            _ => format!("{w}×{h} still"),
        }];
        match self.samples() {
            Samples::Accumulate { spp } => parts.push(format!("{spp} samples a pixel")),
            Samples::Ring { .. } => parts.push(format!("{} points", super::render_panel::format_points(self.points as f64))),
        }
        if self.supersample > 1 {
            parts.push(format!("supersampled {}×", self.supersample));
        }
        if self.mode == Mode::Still && self.bit_depth == crate::offline::BitDepth::Sixteen {
            parts.push("16-bit".into());
        }
        parts.join(", ")
    }
}

/// Open the dialog.
///
/// A job running or a result not yet seen is shown as it is. Otherwise it
/// opens on the form: the first time from the remembered preset, after that
/// on the settings you left — with what follows the window (the look, the
/// timeline's length, the machine settings) read again, and a fresh filename.
pub fn open(app: &mut App) {
    let unseen = app.job_done().is_some_and(|d| !d.seen);
    let running = app.job().is_some();
    let form = &mut app.ui_state.render_job;
    if form.open && !unseen {
        return;
    }
    form.open = true;
    if running {
        return;
    }
    if unseen {
        form.showing_result = true;
        return;
    }
    form.showing_result = false;
    reseed(app);
}

/// Read again what the form takes from the window and prefs, and on the
/// session's first opening, apply the remembered preset.
fn reseed(app: &mut App) {
    if !app.ui_state.render_job.seeded {
        let name = app.render_preset().unwrap_or(render_preset::DEFAULT).to_string();
        let entry = render_preset::find(&name).unwrap_or_else(|| {
            render_preset::Entry { name: render_preset::DEFAULT.into(), path: None }
        });
        let points = (app.point_capacity() as usize * 4).clamp(8_000_000, 40_000_000);
        let form = &mut app.ui_state.render_job;
        // Start well above the interactive buffer without being reckless: this
        // is the number the old one-click render used. A preset may say
        // otherwise; the built-in stills leave it be.
        form.points = points;
        if let Ok(p) = render_preset::load(&entry) {
            form.apply(&p);
            form.preset = Some((entry, p));
        }
        form.seeded = true;
    }
    let threads = app.render_threads();
    let gpu_share = app.render_gpu_share();
    // Volume mode seeds this as splat, and the dialog says so: a job cannot
    // render volume yet.
    let splat = app.render_mode.is_splat_family();
    let (exposure, transparent) = (app.exposure, app.transparent_render);
    // The timeline's length: the scene's own path, or its tracks, whichever
    // runs longer. A scene with neither keeps what the form has.
    let seconds = (app.scene.camera_path.is_some() || !app.scene.tracks.is_empty()).then(|| app.timeline_length() as f32);
    let form = &mut app.ui_state.render_job;
    form.threads = threads;
    form.gpu_share = gpu_share;
    form.splat = splat;
    form.exposure = exposure;
    form.transparent = transparent;
    if let Some(s) = seconds {
        form.seconds = s;
    }
    form.filename_touched = false;
    form.open_error = None;
    let (mode, format) = (form.mode, form.format);
    app.ui_state.render_job.filename = default_filename(app, mode, format);
}

fn default_filename(app: &App, mode: Mode, format: crate::video::Format) -> String {
    let stamp = crate::app::unix_timestamp();
    let slug = app.scene_slug();
    // Extension from `JobKind`, so the name and the thing that decides how to
    // write the file can't disagree about what kind of file it is.
    let kind = RenderJobForm { mode, format, ..RenderJobForm::default() }.kind();
    let dir = match mode {
        Mode::ViewDescriptor => "views",
        _ => "renders",
    };
    format!("{}/{}-{}.{}", dir, slug, stamp, kind.extension())
}

/// Rewrite the filename to follow the output kind, unless it's been typed.
fn refresh_filename(app: &mut App) {
    let form = &app.ui_state.render_job;
    if !form.filename_touched {
        let (mode, format) = (form.mode, form.format);
        app.ui_state.render_job.filename = default_filename(app, mode, format);
    }
}

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if !app.ui_state.render_job.open {
        return;
    }
    let mut open = true;
    super::window(ctx, app, super::WindowKey::RenderJob, "Render job")
        .open(&mut open)
        .collapsible(false)
        .show(ctx, |ui| {
            if app.job().is_some() {
                draw_running(ui, app);
            } else if app.ui_state.render_job.showing_result && app.job_done().is_some() {
                draw_finished(ui, app);
            } else {
                draw_setup(ui, app);
            }
        });
    super::remember(ctx, app, super::WindowKey::RenderJob);
    if !open {
        app.ui_state.render_job.open = false;
    }
}

// --- Setup ------------------------------------------------------------------

/// What the form's sections read from the app, gathered first so the form can
/// be drawn on its own.
struct Env {
    max_points_m: f32,
    max_threads: usize,
    /// Buffer limit in bytes, for the tile plan.
    limit: u64,
    volume: bool,
    grade: Option<String>,
    /// The window's size, for the render's shape to be compared against.
    window: (u32, u32),
}

fn draw_setup(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("fracturize_render_job_preset").show(ui, |ui| {
        preset_bar(ui, app);
        ui.add_space(2.0);
    });
    egui::Panel::bottom("fracturize_render_job_foot").show(ui, |ui| {
        ui.add_space(4.0);
        footer(ui, app);
        ui.add_space(2.0);
    });

    let env = Env {
        max_points_m: app.max_point_capacity() as f32 / 1e6,
        max_threads: App::max_render_threads(),
        limit: app.max_point_capacity() as u64 * crate::render_job::BYTES_PER_POINT,
        volume: app.render_mode == crate::app::RenderMode::Volume,
        grade: inherited_grade(app),
        window: app.surface_size(),
    };
    // The form is drawn on its own, beside the hint state, rather than through
    // `app`: the tables hand their rows `&mut UiState`, which lives in `app`.
    let mut form = std::mem::take(&mut app.ui_state.render_job);
    let before = form.clone();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        sections(ui, &mut app.ui_state, &mut form, &env);
    });
    app.ui_state.render_job = form;

    let form = app.ui_state.render_job.clone();
    if form.gpu_share != before.gpu_share {
        app.set_render_gpu_share(form.gpu_share);
    }
    if form.threads != before.threads {
        app.set_render_threads(form.threads);
    }
    if (form.mode, form.format) != (before.mode, before.format) {
        refresh_filename(app);
    }
}

fn sections(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm, env: &Env) {
    super::section(ui, SECTIONS, "Output", |ui| output(ui, st, f));
    if f.mode == Mode::ViewDescriptor {
        caption(
            ui,
            "Saves this exact framing to a view file, without rendering anything. You can \
             load it later, or render it from the command line (--view).",
        );
        return;
    }
    super::section(ui, SECTIONS, "Size", |ui| size(ui, st, f, env));
    super::section(ui, SECTIONS, "Quality", |ui| quality(ui, st, f, env));
    super::section(ui, SECTIONS, "Animation", |ui| animation(ui, st, f));
    super::section(ui, SECTIONS, "Look", |ui| look(ui, st, f, env));
    super::section(ui, SECTIONS, "This machine", |ui| machine(ui, st, f, env));
}

fn caption(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(text).small().weak())
}

/// The two columns every table here has: what, and its control.
const SETTING_VALUE: [Col<'static>; 2] = [
    Col::name(
        "Setting",
        "What the control sets. A row indented under another depends on it; a greyed row says \
         what it needs beside its name.",
    ),
    Col::new("Value", "Drag, or click to type an exact value", VALUE_W),
];

/// A switch table: what, and on or off.
const SETTING_TICK: [Col<'static>; 2] = [
    Col::name("Setting", "What the switch does. A greyed row says what it needs beside its name."),
    Col::tick("On", "Ticked, it's on"),
];

/// How far a dependent row's name is indented, per level.
const INDENT: f32 = 12.0;

/// A row's name, and how it stands with the rows it depends on.
///
/// Which controls switch which others used to be said only on hover. Two
/// things say it on the page now: a row that depends on the one above is
/// **indented** under it, and a row that can't be used says **what it needs**
/// beside its name, in words that name the control that would free it
/// ("needs Samples: accumulate", "stills only"). The note is a short form of the
/// tooltip, which still has the whole reason.
#[derive(Clone, Copy)]
struct Name<'a> {
    text: &'a str,
    depth: u8,
    on: bool,
    note: Option<&'a str>,
}

fn name(text: &str) -> Name<'_> {
    Name { text, depth: 0, on: true, note: None }
}

impl<'a> Name<'a> {
    /// Indented under the row it depends on.
    fn under(self, depth: u8) -> Self {
        Self { depth, ..self }
    }

    /// Greyed, saying what it needs, unless `needs` is `None`.
    fn needs(self, needs: Option<&'a str>) -> Self {
        Self { on: needs.is_none(), note: needs, ..self }
    }

    /// A note shown without greying the row: for a control only part of
    /// which is unavailable, like one segment of a radio.
    fn note(self, note: Option<&'a str>) -> Self {
        Self { note, ..self }
    }

    /// The name, and the note beside it unless the caller shows it elsewhere.
    fn draw(self, ui: &mut egui::Ui, st: &mut UiState, tip: &str, with_note: bool) {
        ui.add_space(self.depth as f32 * INDENT);
        ui.add_enabled_ui(self.on, |ui| {
            let resp = ui.add(egui::Label::new(self.text).sense(egui::Sense::hover()));
            hinted(resp, st, tip, self.text);
        });
        if let Some(note) = self.note.filter(|_| with_note) {
            note_label(ui, st, note, tip, self.text);
        }
    }
}

fn note_label(ui: &mut egui::Ui, st: &mut UiState, note: &str, tip: &str, name: &str) {
    let resp = ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).sense(egui::Sense::hover()));
    hinted(resp, st, tip, name);
}

/// One row: its name, and its control.
///
/// A row that can't be used shows what it needs **in place of its control**:
/// the control's value means nothing while it doesn't apply, the value column
/// has the room to name the row that would free it, and beside a long name at
/// a deep indent the note was cut off. The row, its name and its tooltip stay,
/// so the feature still says it exists.
fn row(
    body: &mut egui_extras::TableBody<'_>,
    st: &mut UiState,
    name: Name<'_>,
    tip: &str,
    value: impl FnOnce(&mut egui::Ui, &mut UiState),
) {
    let h = table::row_height(body.ui_mut());
    let in_value = (!name.on).then_some(name.note).flatten();
    body.row(h, |mut row| {
        row.col(|ui| name.draw(ui, st, tip, in_value.is_none()));
        row.col(|ui| {
            ui.spacing_mut().slider_width = VALUE_W - 70.0;
            match in_value {
                Some(note) => note_label(ui, st, note, tip, name.text),
                None => {
                    ui.add_enabled_ui(name.on, |ui| value(ui, st));
                }
            }
        });
    });
}

/// A switch row.
fn tick_row(body: &mut egui_extras::TableBody<'_>, st: &mut UiState, name: Name<'_>, tip: &str, value: &mut bool) -> bool {
    let h = table::row_height(body.ui_mut());
    let mut changed = false;
    body.row(h, |mut row| {
        row.col(|ui| name.draw(ui, st, tip, true));
        row.col(|ui| {
            ui.add_enabled_ui(name.on, |ui| {
                let resp = table::tick(ui, value);
                changed = hinted(resp, st, tip, &format!("click: {}", name.text)).changed();
            });
        });
    });
    changed
}

/// What to make, where it goes, and the two things about the file itself.
fn output(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm) {
    // A segmented radio, not four loose toggles: exactly one of these is
    // always in force. See `ui::radio`.
    let current = Output::of(f.mode, f.format);
    let mut chooser = super::radio::radio(st, "render_output", current);
    for (mode, format, label, tip) in OUTPUTS {
        chooser = chooser.option(Output::of(mode, format), label, tip, "click: choose the output kind");
    }
    if let Some(chosen) = chooser.show(ui) {
        let (mode, format) = OUTPUTS
            .iter()
            .find(|(m, fm, ..)| Output::of(*m, *fm) == chosen)
            .map(|(m, fm, ..)| (*m, *fm))
            .unwrap_or((Mode::Still, crate::video::Format::Avif));
        f.mode = mode;
        if mode == Mode::Animation {
            f.format = format;
        }
    }

    let resp = ui.add(
        egui::TextEdit::singleline(&mut f.filename)
            .desired_width(f32::INFINITY)
            .hint_text("renders/name.png"),
    );
    let resp = hinted(
        resp,
        st,
        "Where to save the result. Missing folders are created. A relative path starts \
         from the folder Fracturize was launched in.",
        "type: the output filename",
    );
    if resp.changed() {
        f.filename_touched = true;
    }
    let path = PathBuf::from(f.filename.trim());
    let (text, color) = match filename_trouble(&path) {
        Some(t) => (t.message(), t.color(ui.visuals())),
        // A blank line rather than no line. **The row must always be there**,
        // because everything below it moves when it isn't: every absolute path
        // is typed through `/home`, `/home/you` and so on, so this row used to
        // blink in and out under the cursor and shuffle the form with it.
        None => (" ", ui.visuals().text_color()),
    };
    ui.label(egui::RichText::new(text).color(color).small());

    if f.mode == Mode::ViewDescriptor {
        return;
    }
    // Both describe the *file* that leaves the app, not how the picture is
    // drawn, so they sit with the filename rather than with quality.
    let still = f.mode == Mode::Still;
    table::show(ui, st, "fracturize_render_job_file", &SETTING_TICK, |body, st| {
        let mut sixteen = f.bit_depth == crate::offline::BitDepth::Sixteen;
        let tip = if still {
            "Save more precise colour, at twice the file size. Worth it for a final render: \
             smooth gradients can show visible bands at 8 bits."
        } else {
            "Only for stills: video is 8 bits by codec."
        };
        let only = (!still).then_some("stills only");
        if tick_row(body, st, name("16-bit PNG").needs(only), tip, &mut sixteen) {
            f.bit_depth =
                if sixteen { crate::offline::BitDepth::Sixteen } else { crate::offline::BitDepth::Eight };
        }
        let tip = if still {
            "Save with a transparent background, for layering over other images"
        } else {
            "Animations can't have a transparent background"
        };
        tick_row(body, st, name("Transparent background").needs(only), tip, &mut f.transparent);
    });
}

fn size(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm, env: &Env) {
    let (ww, wh) = env.window;
    table::show(ui, st, "fracturize_render_job_size", &SETTING_VALUE, |body, st| {
        row(body, st, name("Width × height"), "The output size in pixels", |ui, st| {
            let resp = ui.add(num::Drag::new(&mut f.width).range(16..=16384));
            hinted(resp, st, "Output width in pixels", "drag: output width");
            ui.label("×");
            let resp = ui.add(num::Drag::new(&mut f.height).range(16..=16384));
            hinted(resp, st, "Output height in pixels", "drag: output height");
        });
        row(body, st, name("Common size"), "Pick a standard size, or the window's shape", |ui, st| {
            let current = SIZE_PRESETS
                .iter()
                .find(|(_, w, h)| (*w, *h) == (f.width, f.height))
                .map_or("custom", |(name, ..)| name);
            let combo = egui::ComboBox::from_id_salt("fracturize_render_job_size_preset")
                .selected_text(current)
                .width(VALUE_W - 12.0)
                .show_ui(ui, |ui| {
                    let (w, h) = window_shaped(f.width, ww, wh);
                    let resp = ui.selectable_label(false, format!("the window's shape  ({w}×{h})"));
                    let tip = "Keep this width and set the height to give the window's shape, so the \
                               render frames exactly what you see";
                    if hinted(resp, st, tip, "click: match the window's shape").clicked() {
                        (f.width, f.height) = (w, h);
                    }
                    ui.separator();
                    for (label, w, h) in SIZE_PRESETS {
                        let resp = ui.selectable_label((w, h) == (f.width, f.height), format!("{label}  ({w}×{h})"));
                        if hinted(resp, st, &format!("{w}×{h}"), "click: use this output size").clicked() {
                            (f.width, f.height) = (w, h);
                        }
                    }
                });
            hinted(combo.response, st, "Pick a standard size, or the window's shape", "click: pick a size");
        });
        let tip = "Width over height. The render keeps the window's vertical view, so its shape \
                   decides how much is seen at the sides.";
        row(body, st, name("Aspect ratio"), tip, |ui, _| {
            ui.label(egui::RichText::new(aspect_label(f.width, f.height)).monospace());
        });
    });
    // Always one line, so changing the size never moves what's below.
    caption(ui, &against_window(f.width, f.height, ww, wh)).on_hover_text(
        "The render keeps the window's vertical field of view: top and bottom match what you \
         see, and a wider shape shows more at the left and right, a narrower one less.",
    );
}

/// `w:h` in lowest terms when that is a ratio people say (16:9, 4:3, 1:1),
/// else `1.41:1`.
fn aspect_label(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    let (a, b) = (w / g, h / g);
    if a <= 32 && b <= 32 {
        format!("{a}:{b}")
    } else {
        format!("{}:1", num::fixed(w as f32 / h.max(1) as f32, 2))
    }
}

/// The render's shape against the window's, in words.
fn against_window(w: u32, h: u32, ww: u32, wh: u32) -> String {
    let win = format!("the window ({})", aspect_label(ww, wh));
    let r = (w as f32 / h.max(1) as f32) / (ww as f32 / wh.max(1) as f32);
    if (r - 1.0).abs() < 0.01 {
        format!("Same shape as {win}: it frames what you see.")
    } else if r > 1.0 {
        format!("Wider than {win}: shows more at the sides than you see.")
    } else {
        format!("Narrower than {win}: cuts off some of the sides.")
    }
}

/// `width` kept, the height that gives the window's shape: even, since video
/// needs even sizes, and within what the size fields take.
fn window_shaped(width: u32, ww: u32, wh: u32) -> (u32, u32) {
    let h = (width as f32 * wh as f32 / ww.max(1) as f32 / 2.0).round() as u32 * 2;
    (width, h.clamp(16, 16384))
}

/// The renderer and the samples first, then the smoothing on top of them.
///
/// The renderer is here, not with the look, because it decides what the
/// sample choice under it can be: only splat accumulates. It used to sit at
/// the foot of the form, three sections away from the radio it greyed.
///
/// The two sample modes are a genuine choice rather than a preference, because
/// they scale in opposite directions and the dialog cannot hide which one you
/// are on:
///
/// * **Accumulate** targets samples per *output* pixel, so the same setting
///   means the same density at 720p and at A2.
/// * **One pass** splats the filled buffer once, so its density is
///   `points / pixels` and falls as the output grows — 21.7 samples/px at 720p
///   and 2.4 at 4K from the settings this dialog used to default to. That is
///   the grain, and it is why accumulation is the default.
fn quality(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm, env: &Env) {
    let why_not = f.why_not_accumulate();
    let accumulating = f.accumulating();
    table::show(ui, st, "fracturize_render_job_quality", &SETTING_VALUE, |body, st| {
        table::group_row(body, "Samples");
        let tip = "Which renderer draws the picture: starts as the window has it. Only splat can \
                   accumulate.";
        row(body, st, name("Renderer"), tip, |ui, st| {
            #[derive(Clone, Copy, PartialEq)]
            enum R {
                Points,
                Splat,
            }
            let chosen = super::radio::radio(st, "render_job_renderer", if f.splat { R::Splat } else { R::Points })
                .option(R::Points, "points", "Opaque points, every one at full brightness", "click: points renderer")
                .option(R::Splat, "splat", "Density, built up and toned like a flame", "click: splat renderer")
                .show(ui);
            if let Some(r) = chosen {
                f.splat = r == R::Splat;
            }
        });
        let short = match (f.splat, f.mode) {
            (false, _) => Some("accumulate needs splat"),
            (true, Mode::Still) => None,
            (true, _) => Some("accumulate: stills only"),
        };
        row(body, st, name("Samples").under(1).note(short), "How the render gathers its samples", |ui, st| {
            use render_preset::Samples as S;
            let current = if accumulating { S::Accumulate } else { S::OnePass };
            let chosen = super::radio::radio(st, "render_job_samples", current)
                .option_enabled(
                    why_not.is_none(),
                    S::Accumulate,
                    "accumulate",
                    why_not.unwrap_or(
                        "Keep adding points until each pixel has had the number of samples you ask \
                         for. The same setting gives the same quality at any size, and stopping \
                         early gives a noisier version of the same picture.",
                    ),
                    "click: render to a sample target",
                )
                .option(
                    S::OnePass,
                    "one pass",
                    "Render the points once. Quick, but the same settings look grainier the larger \
                     the image.",
                    "click: render the points once",
                )
                .show(ui);
            if let Some(c) = chosen {
                f.use_accumulate = c == S::Accumulate;
            }
        });
        if accumulating {
            // Logarithmic, and to 10,000 rather than to a comfortable number:
            // the useful artistic band is 10-1,000, which lands at 25%-75% of a
            // log track, and the top exists so the dial does not have to be
            // abandoned for the command line at exactly the point the picture
            // gets good.
            let tip = "Samples per pixel. More is cleaner and slower: 10× the samples is about 3× \
                       less noise. 100 is a good everyday value. Above about 30,000, also tick \
                       16-bit PNG, or the file can't hold the extra detail.";
            row(body, st, name("Samples per pixel").under(2), tip, |ui, st| {
                let resp = ui.add(egui::Slider::new(&mut f.spp, 1..=10_000).logarithmic(true));
                hinted(resp, st, tip, "drag: samples per output pixel");
            });
        } else {
            let tip = "Extra rounds of the chaos game after the points are filled in. Settles the \
                       points further without adding more of them. Costs almost nothing.";
            row(body, st, name("Extra rounds").under(2), tip, |ui, st| {
                let resp = ui.add(egui::Slider::new(&mut f.accumulate, 1..=512).logarithmic(true));
                hinted(resp, st, tip, "drag: extra rounds of the chaos game");
            });
        }
        // Density estimation sits with the samples rather than with the
        // filter, even though both are blurs: it needs a *histogram* to read a
        // density from, so it lives or dies with the choice above it.
        let tip = if accumulating {
            "Smooths the grainy, sparse parts of the picture while leaving the dense, detailed \
             parts sharp. Much of what gives Apophysis renders their look. Try around 0.3 before \
             going to 1.0."
        } else {
            "Needs Samples set to accumulate."
        };
        let needs = (!accumulating).then_some("needs Samples: accumulate");
        row(body, st, name("Density estimation").under(2).needs(needs), tip, |ui, st| {
            let resp = ui.add(egui::Slider::new(&mut f.density_estimation, 0.0..=1.0).fixed_decimals(2));
            hinted(resp, st, tip, "drag: density-estimation strength");
        });
        // The same slider means two different things, so it says which. Under
        // accumulation it is a working set — measured, varying it 6x at a
        // fixed sample target moved the wall clock under 25% — and telling
        // someone it is a quality dial there would send them tuning the one
        // control that isn't.
        let tip = if accumulating {
            "How many points are worked on at a time. This doesn't affect quality (samples per \
             pixel does), so leave it alone unless GPU memory is tight."
        } else {
            "How many points to render. More looks better. This only applies to the render; the \
             live view keeps its own point count."
        };
        row(body, st, name("Points"), tip, |ui, st| {
            let mut millions = f.points as f32 / 1e6;
            let resp = ui.add(
                egui::Slider::new(&mut millions, 0.5..=env.max_points_m)
                    .logarithmic(true)
                    // Shared with the interactive point-count slider so a count
                    // reads in the same k/M units in both, and so typing an
                    // exact value parses back.
                    .custom_formatter(|v, _| super::render_panel::format_points(v))
                    .custom_parser(super::render_panel::parse_points),
            );
            if hinted(resp, st, tip, "drag: point count for this job").changed() {
                f.points = (millions * 1e6).round() as usize;
            }
        });

        // Supersampling is cost/quality at fixed artistic intent, like the
        // rows above — nothing here changes what the picture *is*. It is the
        // one that buys the most, though, so the hint says what it is.
        table::group_row(body, "Smoothing edges");
        let tip = "Render at this many times the size, then scale down. Smooths jagged edges, and \
                   is the biggest quality improvement here. 2× costs about 4× the memory and time. \
                   1 turns it off.";
        row(body, st, name("Supersample"), tip, |ui, st| {
            let max = crate::gpu::points::downsample::MAX_SUPERSAMPLE;
            let resp = ui.add(num::Drag::new(&mut f.supersample).range(1..=max).suffix("×"));
            hinted(resp, st, tip, "drag: supersampling factor");
        });
        // Greyed rather than hidden at 1x, so the kernel choice can still say
        // it exists.
        let ss = f.supersample > 1;
        let needs = (!ss).then_some("needs Supersample 2× or more");
        let tip = if ss {
            "The shape used to scale down. Gaussian is soft and safe; lanczos is sharpest but rings \
             around small bright points."
        } else {
            "Only used when Supersample is above 1×"
        };
        row(body, st, name("Filter").under(1).needs(needs), tip, |ui, st| {
            let combo = egui::ComboBox::from_id_salt("render_job_filter")
                .selected_text(f.filter.label())
                .width(VALUE_W - 12.0)
                .show_ui(ui, |ui| {
                    use clap::ValueEnum;
                    for v in crate::gpu::Filter::value_variants() {
                        ui.selectable_value(&mut f.filter, *v, v.label());
                    }
                });
            hinted(combo.response, st, tip, "click: pick the filter");
        });
        let tip = if ss {
            "How soft the scaling-down is, in pixels. Higher is smoother; lower is sharper but more \
             jagged. 0.5 is the default."
        } else {
            "Only used when Supersample is above 1×"
        };
        row(body, st, name("Filter radius").under(1).needs(needs), tip, |ui, st| {
            use crate::gpu::points::downsample::{MAX_FILTER_RADIUS, MIN_FILTER_RADIUS};
            let resp = ui.add(
                num::Drag::new(&mut f.filter_radius).speed(0.05).range(MIN_FILTER_RADIUS..=MAX_FILTER_RADIUS).suffix(" px"),
            );
            hinted(resp, st, tip, "drag: filter radius");
        });
    });

    // Always one line, whichever way the samples are gathered: it used to
    // appear and vanish with accumulation, moving everything below it.
    let params = f.params();
    let text = if accumulating {
        // Tiles are not a detail to hide: past the binding limit the chaos
        // game runs once per tile, so this line is where a job quietly stops
        // costing seconds and starts costing minutes.
        let tiling = match params.tile_plan(env.limit) {
            Ok(p) if p.is_single() => String::new(),
            Ok(p) => format!(" · {} tiles ({}x{})", p.tiles.len(), p.cols, p.rows),
            Err(_) => String::from(" · too large"),
        };
        format!("{} laps · {} histogram{}", params.laps(), human_bytes(params.histogram_bytes()), tiling)
    } else {
        let pixels = (f.width as u64 * f.height as u64).max(1);
        format!("one pass · about {} samples/px, from the points", num::fixed(f.points as f32 / pixels as f32, 1))
    };
    caption(ui, &text).on_hover_text(if accumulating {
        "How many passes the render will take, and how much GPU memory the image needs. If it's \
         too big for the graphics card in one piece, it's rendered in tiles and stitched \
         together, which takes longer."
    } else {
        "One pass draws each point once, so the samples per pixel are the points over the \
         pixels: the same points look grainier the larger the image."
    });
}

/// Shown for every kind of render and greyed for a still, so switching the
/// output kind doesn't add and remove a section under the pointer.
fn animation(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm) {
    let only = (f.mode != Mode::Animation).then_some("for avif or mp4 output");
    let why = |tip: &str| if only.is_some() { "Only for animations: pick avif or mp4 under Output".to_string() } else { tip.to_string() };
    table::show(ui, st, "fracturize_render_job_animation", &SETTING_VALUE, |body, st| {
        let tip = why("Frames per second");
        row(body, st, name("Frame rate").needs(only), &tip, |ui, st| {
            let resp = ui.add(num::Drag::new(&mut f.fps).range(1..=120).suffix(" fps"));
            hinted(resp, st, &tip, "drag: frame rate");
        });
        let tip = why("Length of the animation. Starts at the timeline's own length");
        row(body, st, name("Length").needs(only), &tip, |ui, st| {
            let resp = ui.add(num::Drag::new(&mut f.seconds).speed(0.1).range(0.1..=600.0).suffix(" s"));
            hinted(resp, st, &tip, "drag: duration");
        });
        let codec = f.format.codec_label();
        let tip = why(&format!("{codec} quality, 0 to 100. Higher looks better and makes a bigger file."));
        row(body, st, name("Codec quality").needs(only), &tip, |ui, st| {
            let resp = ui.add(num::Drag::new(&mut f.quality).range(0..=100));
            hinted(resp, st, &tip, &format!("drag: {codec} quality"));
        });
    });
}

/// What the render looks like beyond the renderer: taken from the window each
/// time the dialog opens, and not part of a preset.
fn look(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm, env: &Env) {
    caption(ui, "As the window has it when this opens. Not part of a preset.");
    table::show(ui, st, "fracturize_render_job_look", &SETTING_VALUE, |body, st| {
        let tip = if f.splat {
            "Brightness, for the splat renderer"
        } else {
            "Only for the splat renderer: set Renderer to splat, under Quality"
        };
        let needs = (!f.splat).then_some("needs Renderer: splat");
        row(body, st, name("Exposure").needs(needs), tip, |ui, st| {
            let resp = ui.add(num::Drag::new(&mut f.exposure).speed(0.05).range(0.01..=100.0));
            hinted(resp, st, tip, "drag: splat exposure");
        });
    });
    // What this job inherits rather than owns. The grade is a live control in
    // the Render window, where you can see it working, so it is not
    // duplicated here as three more sliders — but an inherited setting that
    // is never shown is a trap, so it is stated when there is one, whatever
    // the renderer, so the line doesn't come and go with it.
    if let Some(grade) = &env.grade {
        let text = if f.splat { format!("Grade from the Render window: {grade}") } else { format!("Grade (splat only): {grade}") };
        caption(ui, &text).on_hover_text(
            "Gamma and vibrancy are set in the Render window, where you can see them working. A \
             splat render uses whatever is set there.",
        );
    }
    if env.volume {
        // Said out loud rather than leaving the job to render something other
        // than the window: accumulating volume renders are not built yet.
        caption(
            ui,
            "The window is in volume mode, but render jobs can't draw it yet. This job renders \
             splat, without the depth.",
        );
    }
}

/// Machine settings among job settings, and they say so: what is right here is
/// a fact about this box, so they are remembered, and no preset carries them.
fn machine(ui: &mut egui::Ui, st: &mut UiState, f: &mut RenderJobForm, env: &Env) {
    caption(ui, "Remembered for every render on this computer. Not part of a preset.");
    let accumulating = f.accumulating();
    table::show(ui, st, "fracturize_render_job_machine", &SETTING_VALUE, |body, st| {
        // This steers the *long* part of a still: the chaos game saturates the
        // card, and on a one-GPU desktop that is the card drawing the window
        // you are watching from.
        let tip = if accumulating {
            "How much of the graphics card the render may use. At 1.00 it's fastest, but the \
             desktop may stutter. 0.50 takes about twice as long and leaves room for everything else."
        } else {
            "Only paces an accumulating still, between its passes: set Samples to accumulate"
        };
        let needs = (!accumulating).then_some("needs Samples: accumulate");
        row(body, st, name("GPU share").needs(needs), tip, |ui, st| {
            let resp = ui.add(egui::Slider::new(&mut f.gpu_share, 0.05..=1.0).fixed_decimals(2));
            hinted(resp, st, tip, "drag: share of the GPU this render may take");
        });
        // Encoding is the only thing it steers today — a still's PNG deflate
        // is one short single-threaded pass.
        let animation = f.mode == Mode::Animation;
        let tip = if animation {
            format!(
                "How many CPU threads to use for encoding, out of this machine's {}. The default \
                 leaves one free so the desktop stays usable.",
                env.max_threads
            )
        } else {
            "Only used to encode animations: pick avif or mp4 under Output".to_string()
        };
        row(body, st, name("Encoder threads").needs((!animation).then_some("for avif or mp4 output")), &tip, |ui, st| {
            let resp = ui.add(num::Drag::new(&mut f.threads).range(1..=env.max_threads));
            hinted(resp, st, &tip, "drag: CPU threads for encoding");
        });
    });
}

/// The grade the Render window will hand the job, in words, or `None` for a
/// neutral one — the absence of a grade, which a line every time would only
/// make noise about.
fn inherited_grade(app: &App) -> Option<String> {
    if app.grade.is_neutral() {
        return None;
    }
    let g = app.grade;
    let mut parts = vec![format!("gamma {:.2}", g.gamma)];
    if g.gamma_threshold > 0.0 {
        parts.push(format!("threshold {:.2}", g.gamma_threshold));
    }
    if g.vibrancy != 1.0 {
        parts.push(format!("vibrancy {:.2}", g.vibrancy));
    }
    Some(parts.join(" · "))
}

/// The preset picker and its verbs: Save, Save as…, Revert, Delete. The name
/// carries a `*` while the settings differ from it, as a scene's does.
fn preset_bar(ui: &mut egui::Ui, app: &mut App) {
    let form = &app.ui_state.render_job;
    let selected = form.preset.clone();
    let modified = form.preset_modified();
    let mut picked: Option<render_preset::Entry> = None;

    ui.horizontal(|ui| {
        ui.label("Preset");
        let caption = match &selected {
            Some((e, _)) => format!("{}{}", e.name, if modified { " *" } else { "" }),
            None => "none".to_string(),
        };
        let combo = egui::ComboBox::from_id_salt("fracturize_render_job_preset")
            .selected_text(caption)
            .width(200.0)
            .show_ui(ui, |ui| {
                let entries = render_preset::list();
                let groups: [(&str, bool); 2] = [("Built in", true), ("Yours", false)];
                for (i, (heading, builtin)) in groups.into_iter().enumerate() {
                    let group: Vec<_> = entries.iter().filter(|e| e.is_builtin() == builtin).collect();
                    if group.is_empty() {
                        continue;
                    }
                    if i > 0 {
                        ui.separator();
                    }
                    ui.label(egui::RichText::new(heading).small().weak());
                    for entry in group {
                        let on = selected.as_ref().is_some_and(|(e, _)| e == entry);
                        let resp = ui.selectable_label(on, &entry.name);
                        // Read only for the row under the pointer.
                        let tip = if resp.hovered() {
                            render_preset::load(entry).map(|p| p.description).unwrap_or_else(|e| e)
                        } else {
                            String::new()
                        };
                        if hinted(resp, &mut app.ui_state, &tip, "click: use this preset").clicked() {
                            picked = Some(entry.clone());
                        }
                    }
                }
            });
        hinted(
            combo.response,
            &mut app.ui_state,
            "Sets the output, size and quality in one go. The look and this machine's settings \
             stay as they are.",
            "click: pick a render preset",
        );
    });
    if let Some((_, p)) = &selected {
        if !p.description.is_empty() {
            caption(ui, &p.description);
        }
    }

    let mut action = None;
    ui.horizontal(|ui| {
        let writable = selected.as_ref().is_some_and(|(e, _)| !e.is_builtin());
        let resp = ui.add_enabled(writable && modified, egui::Button::new((icons::SAVE, "Save")));
        let tip = if !writable {
            "Built-in presets can't be changed. Save as makes one of your own."
        } else if !modified {
            "Nothing to save: the settings are what the preset says"
        } else {
            "Write these settings into the preset"
        };
        if hinted(resp, &mut app.ui_state, tip, "click: save the preset").clicked() {
            action = Some(PresetAction::Save);
        }
        let resp = ui.button("Save as…");
        if hinted(resp, &mut app.ui_state, "Save these settings as a new preset of your own", "click: save as a new preset")
            .clicked()
        {
            app.ui_state.render_job.preset_save_as = Some(String::new());
            app.ui_state.render_job.preset_error = None;
        }
        let resp = ui.add_enabled(modified, egui::Button::new((icons::UNDO, "Revert")));
        if hinted(resp, &mut app.ui_state, "Put the settings back to what the preset says", "click: revert to the preset")
            .clicked()
        {
            action = Some(PresetAction::Revert);
        }
        if writable {
            let arm = app.ui_state.render_job.preset_delete;
            let (arm, fired) = super::confirm::danger_button(
                ui,
                &mut app.ui_state,
                arm,
                icons::TRASH,
                "Delete",
                "Delete preset",
                "Delete this preset's file",
                "Delete it now. The file is removed; this can't be undone.",
            );
            app.ui_state.render_job.preset_delete = arm;
            if fired {
                action = Some(PresetAction::Delete);
            }
        } else {
            let resp = ui.add_enabled(false, egui::Button::new((icons::TRASH, "Delete")));
            hinted(resp, &mut app.ui_state, "Built-in presets can't be deleted", "a built-in preset");
        }
    });

    if let Some(mut name) = app.ui_state.render_job.preset_save_as.take() {
        let mut keep = true;
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("name").desired_width(160.0));
            let field = hinted(field, &mut app.ui_state, "The new preset's name: letters, digits, '-', '_'", "type: preset name");
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let resp = ui.add_enabled(!name.trim().is_empty(), egui::Button::new((icons::SAVE, "Save")));
            if hinted(resp, &mut app.ui_state, "Save the preset under this name", "click: save as").clicked() || enter {
                action = Some(PresetAction::SaveAs(name.clone()));
                keep = false;
            }
            let resp = ui.button("Cancel");
            if hinted(resp, &mut app.ui_state, "Don't save a new preset", "click: cancel").clicked() {
                keep = false;
                app.ui_state.render_job.preset_error = None;
            }
        });
        if keep {
            app.ui_state.render_job.preset_save_as = Some(name);
        }
    }
    if let Some(err) = &app.ui_state.render_job.preset_error {
        ui.label(egui::RichText::new(err).color(super::theme::error(ui.visuals())).small());
    }

    if let Some(entry) = picked {
        action = Some(PresetAction::Load(entry));
    }
    if let Some(action) = action {
        preset_action(app, action);
    }
}

enum PresetAction {
    Load(render_preset::Entry),
    Save,
    SaveAs(String),
    Revert,
    Delete,
}

fn preset_action(app: &mut App, action: PresetAction) {
    let form = &mut app.ui_state.render_job;
    form.preset_error = None;
    let result: Result<(), String> = match action {
        PresetAction::Load(entry) => render_preset::load(&entry).map(|p| {
            form.apply(&p);
            form.preset = Some((entry, p));
        }),
        PresetAction::Revert => {
            if let Some((_, p)) = form.preset.clone() {
                form.apply(&p);
            }
            Ok(())
        }
        PresetAction::Save => match form.preset.clone() {
            Some((entry, old)) => {
                // Keep a description someone wrote; renew one that was only
                // ever the summary of the settings it replaces.
                let mut was = form.clone();
                was.apply(&old);
                let description =
                    if old.description.is_empty() || old.description == was.summary() { form.summary() } else { old.description };
                let p = form.to_preset(description);
                render_preset::save(&entry.name, &p).map(|entry| form.preset = Some((entry, p)))
            }
            None => Ok(()),
        },
        PresetAction::SaveAs(name) => {
            let p = form.to_preset(form.summary());
            render_preset::save(&name, &p).map(|entry| form.preset = Some((entry, p)))
        }
        PresetAction::Delete => match form.preset.take() {
            Some((entry, p)) => render_preset::delete(&entry).map_err(|e| {
                form.preset = Some((entry, p));
                e
            }),
            None => Ok(()),
        },
    };
    let form = &mut app.ui_state.render_job;
    if let Err(e) = result {
        form.preset_error = Some(e);
        return;
    }
    let name = form.preset.as_ref().map(|(e, _)| e.name.clone());
    app.set_render_preset(name);
    refresh_filename(app);
}

/// Cost, then Start. Pinned under the form, so the answer to "what will this
/// cost?" is beside the button that commits to it however far the form has
/// been scrolled.
fn footer(ui: &mut egui::Ui, app: &mut App) {
    let params = app.ui_state.render_job.params();
    let limit = app.max_point_capacity() as u64 * crate::render_job::BYTES_PER_POINT;
    if app.ui_state.render_job.mode != Mode::ViewDescriptor {
        caption(
            ui,
            &format!(
                "GPU memory: {} ({} of it points), limit {}",
                human_bytes(params.total_bytes()),
                human_bytes(params.point_buffer_bytes()),
                human_bytes(limit),
            ),
        );
        let frames = params.kind.frames();
        match estimate_secs(app, &params, limit) {
            Some((low, high)) => {
                ui.label(
                    egui::RichText::new(format!(
                        "Estimated time: {} ({} frame{})",
                        format_estimate(low, high),
                        frames,
                        if frames == 1 { "" } else { "s" },
                    ))
                    .small(),
                )
                .on_hover_text(
                    "A rough guess based on how fast things have run so far this session. Once the \
                     render starts, it's replaced by a better one.",
                );
            }
            None => {
                caption(ui, "Estimated time: not known yet").on_hover_text(
                    "Known once the window has drawn a few frames to measure this machine by",
                );
            }
        }
        if let Some(reason) = params.rejection(limit) {
            ui.label(egui::RichText::new(reason).color(super::theme::error(ui.visuals())).small());
        }
    }
    if let Some(err) = app.job_error() {
        let err = err.to_string();
        ui.label(egui::RichText::new(err).color(super::theme::error(ui.visuals())).small());
    }

    ui.horizontal(|ui| {
        let named = !params.out_path.as_os_str().is_empty();
        let blocked = filename_trouble(&params.out_path).is_some_and(|t| t.blocks_start());
        let resp = ui.add_enabled(named && !blocked, egui::Button::new((icons::PLAY, "Start")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if blocked {
                "That's a folder. Add a filename to the end."
            } else if named {
                "Start rendering. You can keep using the app while it runs"
            } else {
                "Enter a filename first"
            },
            "click: start the render job",
        );
        if resp.clicked() {
            app.start_job(params);
            // A refused launch leaves the form up with its reason; one that
            // ran (or a view file, which finishes at once) is reported on.
            app.ui_state.render_job.showing_result = app.job_error().is_none();
            app.ui_state.render_job.open_error = None;
        }
        let resp = ui.button((icons::X, "Close"));
        if hinted(resp, &mut app.ui_state, "Close this window", "click: close").clicked() {
            app.ui_state.render_job.open = false;
        }
    });
}

/// A low/high seconds range for the job, from this session's measured
/// throughput — `None` until the renderer has produced a frame to measure.
fn estimate_secs(app: &App, params: &JobParams, limit: u64) -> Option<(f32, f32)> {
    // A structure track refills the buffer every frame of an animation.
    let refills = app.scene.tracks.iter().any(|t| !t.mute && t.class().refills());
    crate::render_job::estimate_secs(app.measured_throughput()?, params, limit, refills)
}

/// The Output row is a file-kind chooser, so the two animation formats get a
/// button each rather than hiding behind a second control. They differ by
/// codec, not just extension (see `src/video.rs`), and which one you want is
/// decided by where the file is going — which is exactly the kind of thing
/// that should be visible at the top of the dialog rather than found later.
const OUTPUTS: [(Mode, crate::video::Format, &str, &str); 4] = [
    (
        Mode::Still,
        crate::video::Format::Avif, // unused for a still
        "still",
        "One high-quality frame (PNG)",
    ),
    (
        Mode::Animation,
        crate::video::Format::Avif,
        "avif",
        "An animation along the camera path, as AVIF. Loops like a GIF at a fraction \
         of the size and plays in browsers, but many websites won't accept it.",
    ),
    (
        Mode::Animation,
        crate::video::Format::Mp4,
        "mp4",
        "An animation along the camera path, as MP4. Bigger than AVIF, but accepted \
         almost everywhere you might upload it.",
    ),
    (
        Mode::ViewDescriptor,
        crate::video::Format::Avif, // unused for a view file
        "view",
        "Save the framing to a view file, without rendering anything",
    ),
];

/// The four things this dialog can produce, as the one-of-four choice it is.
///
/// A separate type rather than `(Mode, Format)` because it *is* one choice:
/// picking `mp4` picks both the mode and the codec, and the radio needs a
/// single value to compare against.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    Still,
    Avif,
    Mp4,
    View,
}

impl Output {
    fn of(mode: Mode, format: crate::video::Format) -> Self {
        match mode {
            Mode::Still => Self::Still,
            Mode::ViewDescriptor => Self::View,
            Mode::Animation => match format {
                crate::video::Format::Mp4 => Self::Mp4,
                _ => Self::Avif,
            },
        }
    }
}

/// What is wrong with the output path, if anything.
///
/// Split out from the drawing so the button below can refuse the same cases the
/// label describes — a dialog that says "this is a directory" and then starts
/// the job anyway has told you something it does not act on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum FilenameTrouble {
    /// An existing directory. Not an overwrite: the render cannot be written
    /// here at all, and nothing would be clobbered if it tried.
    IsDirectory,
    /// An existing file, which the render will replace.
    WillOverwrite,
}

impl FilenameTrouble {
    fn message(self) -> &'static str {
        match self {
            FilenameTrouble::IsDirectory => "That's a folder. Add a filename to the end.",
            FilenameTrouble::WillOverwrite => "This file exists and will be overwritten.",
        }
    }

    fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            // An error, not a warning: overwriting is a choice someone can
            // make, and writing to a directory is not a thing that can happen.
            FilenameTrouble::IsDirectory => visuals.error_fg_color,
            FilenameTrouble::WillOverwrite => visuals.warn_fg_color,
        }
    }

    /// Whether this stops the job. Overwriting does not — it is what the
    /// warning is for.
    fn blocks_start(self) -> bool {
        matches!(self, FilenameTrouble::IsDirectory)
    }
}

fn filename_trouble(path: &std::path::Path) -> Option<FilenameTrouble> {
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.is_dir() {
        return Some(FilenameTrouble::IsDirectory);
    }
    path.is_file().then_some(FilenameTrouble::WillOverwrite)
}

// --- Finished ---------------------------------------------------------------

/// Open `path` in whatever the system opens that kind of file with: the image
/// viewer for a PNG, the video player for a clip, the editor for a view file.
/// Detached, so a viewer left open doesn't hold anything here.
fn open_path(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))
}

/// The folder a file is in, opened in the file manager. Not "reveal": showing
/// the file selected needs a different call on every desktop, and the folder
/// is the part that is hard to find.
fn open_folder(path: &Path) -> Result<(), String> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    open_path(dir)
}

/// The finished job: what it made, one click to open it, and the log it wrote
/// on the way — in the place the running view had it.
fn draw_finished(ui: &mut egui::Ui, app: &mut App) {
    app.mark_job_seen();
    let Some(done) = app.job_done().cloned() else { return };

    egui::Panel::bottom("fracturize_render_job_done_foot").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let resp = ui.button((icons::PLUS, "New render"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Back to the settings, as they were for this one, with a fresh filename",
                "click: set up another render",
            );
            if resp.clicked() {
                app.ui_state.render_job.showing_result = false;
                reseed(app);
            }
            let resp = ui.button((icons::X, "Close"));
            if hinted(resp, &mut app.ui_state, "Close this window", "click: close").clicked() {
                app.ui_state.render_job.open = false;
            }
        });
        ui.add_space(2.0);
    });

    result_panel(ui, app, &done);
    if let Some(path) = done.path() {
        ui.horizontal(|ui| {
            let resp = ui.button((icons::FILE, "Open"));
            if hinted(resp, &mut app.ui_state, "Open it in your usual viewer", "click: open the file").clicked() {
                app.ui_state.render_job.open_error = open_path(path).err();
            }
            let resp = ui.button((icons::FOLDER_OPEN, "Show folder"));
            if hinted(resp, &mut app.ui_state, "Open the folder it's in", "click: open the folder").clicked() {
                app.ui_state.render_job.open_error = open_folder(path).err();
            }
        });
    }
    if let Some(err) = &app.ui_state.render_job.open_error {
        ui.label(egui::RichText::new(err).color(super::theme::error(ui.visuals())).small());
    }
    if !done.log.is_empty() {
        ui.separator();
        log_view(ui, &done.log);
    }
}

/// The finished job's summary, in its colour. A successful one is a button:
/// click it to open the file.
///
/// `Outcome` is not decoration. A job stopped partway still writes a file, and
/// the whole reason [`Outcome`] exists is that a noisier-than-asked-for render
/// must not be presented in the same green panel as a finished one — you would
/// come back to it a week later with no way to tell. So a partial render gets
/// the warning colour, its own heading, and no check mark: the mark means
/// *done*, and this isn't.
///
/// [`Outcome`]: crate::render_job::Outcome
fn result_panel(ui: &mut egui::Ui, app: &mut App, done: &FinishedJob) {
    let is_view = matches!(done.params.kind, JobKind::ViewDescriptor);
    let visuals = ui.visuals().clone();
    let (accent, heading, check) = match &done.result {
        Ok((_, o)) if o.is_partial() => (super::theme::warn(&visuals), "Stopped early", false),
        Ok(_) if is_view => (DONE_GREEN, "View saved", true),
        Ok(_) => (DONE_GREEN, "Render done", true),
        Err(e) if e == crate::render_job::CANCELLED => (visuals.weak_text_color(), "Stopped", false),
        Err(_) => (super::theme::error(&visuals), "Render failed", false),
    };
    let path = done.path().map(Path::to_path_buf);

    // Hover is last frame's, so the panel can light up before it is drawn.
    let id = ui.id().with("fracturize_render_job_result");
    let hovered = path.is_some() && ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let resp = egui::Frame::NONE
        .fill(accent.gamma_multiply(if hovered { 0.2 } else { 0.1 }))
        .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(if hovered { 1.0 } else { 0.6 })))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if check {
                    check_mark(ui, accent);
                }
                ui.label(egui::RichText::new(heading).color(accent).strong());
            });
            match &done.result {
                Ok((p, outcome)) => {
                    let text = egui::RichText::new(p.display().to_string()).small();
                    ui.label(if hovered { text.underline() } else { text });
                    if outcome.is_partial() {
                        // Say what it *is*, not only that it was interrupted:
                        // the file is usable, and "run it again" is the fix.
                        caption(
                            ui,
                            "This is the picture as it was when you stopped it: the same image, \
                             but noisier. Run it again for the full quality.",
                        );
                    }
                }
                Err(e) if e == crate::render_job::CANCELLED => {
                    caption(ui, "Stopped before any frames were rendered, so nothing was saved.");
                }
                Err(e) => {
                    ui.label(egui::RichText::new(e).small());
                }
            }
            let mut details = Vec::new();
            if !is_view {
                let (w, h) = done.params.kind.size();
                details.push(format!("{} {w}×{h}", done.params.kind.label()));
                // `kind.frames()` is what was *asked for*, which a partial
                // clip does not have — quoting it would be the one number here
                // that lies. The log carries the real count.
                let frames = done.params.kind.frames();
                if frames > 1 && matches!(done.result, Ok((_, crate::render_job::Outcome::Complete))) {
                    details.push(format!("{frames} frames"));
                }
                details.push(format!("took {}", format_duration(done.secs)));
            }
            if let Some(meta) = path.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
                details.push(human_bytes(meta.len()));
            }
            if !details.is_empty() {
                caption(ui, &details.join(" · "));
            }
        })
        .response;
    let Some(path) = path else { return };
    let resp = resp.interact(egui::Sense::click());
    ui.ctx().data_mut(|d| d.insert_temp(id, resp.hovered()));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let tip = if is_view { "Click to open the view file" } else { "Click to open it in your usual viewer" };
    if hinted(resp, &mut app.ui_state, tip, "click: open the file").clicked() {
        app.ui_state.render_job.open_error = open_path(&path).err();
    }
}

fn log_view(ui: &mut egui::Ui, log: &[String]) {
    egui::ScrollArea::vertical()
        .id_salt("fracturize_job_log")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in log {
                ui.label(egui::RichText::new(line).small().monospace());
            }
        });
}

/// A small checkmark, drawn rather than typed: this app's "drawn, not typed"
/// rule (see `ui::radio`'s ring-and-dot marks and AGENTS.md) exists because
/// the obvious Unicode glyphs (✓ U+2713, ✔ U+2714) aren't confirmed present
/// in any font this app ships or falls back to, and would risk tofu.
fn check_mark(ui: &mut egui::Ui, color: egui::Color32) {
    let size = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let stroke = egui::Stroke::new(2.0, color);
    let p0 = egui::pos2(rect.left() + rect.width() * 0.15, rect.top() + rect.height() * 0.55);
    let p1 = egui::pos2(rect.left() + rect.width() * 0.42, rect.bottom() - rect.height() * 0.15);
    let p2 = egui::pos2(rect.right() - rect.width() * 0.12, rect.top() + rect.height() * 0.2);
    ui.painter().line_segment([p0, p1], stroke);
    ui.painter().line_segment([p1, p2], stroke);
}

// --- Running ----------------------------------------------------------------

/// A progress bar's three states.
///
/// `NotStarted` and `Done` are what a *per-phase* bar needed, and the job now
/// draws one weighted bar for the whole render instead — but the states stay
/// because `progress_row` is still the shared drawing, and a future second bar
/// (a queue, a batch) would want them again. See `Ledger`.
#[allow(dead_code)]
enum BarState {
    NotStarted,
    Active(Option<f32>),
    Done,
}

/// A progress bar's percentage caption: `"100.00%"` down to `"  0.00%"`, in a
/// fixed six-character-plus-sign budget, monospace.
///
/// Six characters is the exact width of the widest value it can print, so the
/// caption never changes length and the two bars' readouts line up with each
/// other however far along they each are.
fn percent(fraction: f32) -> egui::WidgetText {
    egui::RichText::new(format!("{:>6}%", super::num::fixed(fraction * 100.0, 2)))
        .monospace()
        .into()
}


/// One labelled bar in the two-phase progress display.
///
/// `allow_pulse` governs the one deliberate exception to this app's
/// zero-animation rule (see AGENTS.md): a bar whose phase genuinely never
/// reports a total pulses instead of sitting dead at 0%, because motion is
/// the only way to say "working, extent unknown" that a static bar can't.
/// Every other state here — not-started, a real fraction, done — is static.
fn progress_row(ui: &mut egui::Ui, label: &str, state: BarState, allow_pulse: bool) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [60.0, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(label).small()),
        );
        let (fraction, text, animate, fill): (f32, egui::WidgetText, bool, Option<egui::Color32>) =
            match state {
                BarState::NotStarted => (0.0, "not started".into(), false, None),
                // Two decimal places, not whole percent. A whole-percent
                // readout on an hours-long render sits on the same number for
                // minutes at a time, which is indistinguishable from a job that
                // has wedged — precisely when you most want to know it hasn't.
                // Monospace and right-aligned in a fixed budget for the usual
                // reason (`ui::num`): these digits change every frame.
                BarState::Active(Some(f)) => (f, percent(f), false, None),
                // No fraction while active means the underlying phase hasn't
                // reported one — either it's the genuinely-unknown-extent case
                // (encoding) or a brief gap right as a phase starts, before its
                // first progress report arrives. Only the former pulses.
                BarState::Active(None) if allow_pulse => (0.0, "working…".into(), true, None),
                BarState::Active(None) => (0.0, percent(0.0), false, None),
                BarState::Done => (1.0, "done".into(), false, Some(DONE_GREEN)),
            };
        let mut bar = egui::ProgressBar::new(fraction)
            .desired_width(ui.available_width())
            .text(text)
            .animate(animate);
        if let Some(c) = fill {
            bar = bar.fill(c);
        }
        ui.add(bar);
    });
}

fn draw_running(ui: &mut egui::Ui, app: &mut App) {
    // Read what the display needs before taking `&mut` for the buttons.
    let (phase, overall, elapsed, remaining, paused, cancelling, cancel_arm, log, out) = {
        let job = app.job().expect("caller checked");
        (
            job.phase,
            job.overall(),
            job.started.elapsed().as_secs_f32(),
            job.remaining_secs(),
            job.paused(),
            job.cancelling(),
            job.cancel_arm,
            job.log.clone(),
            job.params.out_path.clone(),
        )
    };

    egui::Panel::bottom("fracturize_render_job_run_foot").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let resp = ui.button(if paused { (icons::PLAY, "Resume") } else { (icons::PAUSE, "Pause") });
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Pause the render without losing progress",
                "click: pause / resume the job",
            );
            if resp.clicked() {
                if let Some(job) = app.job_mut() {
                    let p = job.paused();
                    job.set_paused(!p);
                }
            }

            // Two stages: `Stop` arms it, it counts down disabled, then it
            // comes back as a red `Stop render`. See `ui::confirm::danger_button`.
            if cancelling {
                let resp = ui.add_enabled(false, egui::Button::new((icons::TRASH, "Stopping…")));
                hinted(
                    resp,
                    &mut app.ui_state,
                    "Stopping. What has been rendered so far is being saved.",
                    "the render is stopping",
                );
            } else {
                let (arm, fired) = super::confirm::danger_button(
                    ui,
                    &mut app.ui_state,
                    cancel_arm,
                    icons::TRASH,
                    "Stop",
                    "Stop render",
                    "Stop the render early and save what it has so far",
                    "Stop now. What's rendered so far is saved, noisier than a finished render. An \
                     animation stopped before its first frame saves nothing.",
                );
                if let Some(job) = app.job_mut() {
                    job.cancel_arm = arm;
                    if fired {
                        job.cancel_now();
                    }
                }
            }

            let resp = ui.button((icons::X, "Close"));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Close this window. The render keeps going: the status bar shows its progress, and \
                 says when it's done",
                "click: close",
            );
            if resp.clicked() {
                app.ui_state.render_job.open = false;
            }
        });
        ui.add_space(2.0);
    });

    caption(ui, &out.display().to_string());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(phase.label()).strong());
        // Where in the work, when there is an interior worth naming — `pass
        // 2/3` while accumulating, `tile 7/12` while resolving. A single-tile
        // render says nothing here rather than "pass 1/1".
        if let Some(pos) = phase.position() {
            ui.label(egui::RichText::new(pos).small().weak());
        }
        if cancelling {
            ui.label(egui::RichText::new("stopping…").color(super::theme::warn(ui.visuals())));
        } else if paused {
            ui.label(egui::RichText::new("paused").color(super::theme::warn(ui.visuals())));
        }
    });

    // **One bar for the whole job**, phases weighted by their estimated cost.
    //
    // There were two, named Render and Encode, chosen by matching phase strings
    // across a module boundary. That model does not survive tiling: a render is
    // now passes containing laps, then a resolve per tile, then a write, and
    // the phases are wildly unequal — an A2 poster spends 67s accumulating and
    // 5.7s saving. Weighting by measured seconds gives one bar that is
    // monotonic and roughly linear in time.
    progress_row(ui, "Progress", BarState::Active(Some(overall)), false);

    // Both durations in fixed-width cells, and the whole line in one: this is
    // the readout that ticks every frame for an hour, so every length change
    // in it is a shuffle you watch.
    let line = match remaining {
        // "about", because it is: the countdown is only shown once progress
        // can support it, and it is filtered, not exact.
        Some(r) => format!("{} elapsed · about {} left", format_duration_cell(elapsed), format_duration_cell(r)),
        None => format!("{} elapsed", format_duration_cell(elapsed)),
    };
    num::small_cell(ui, &line, ELAPSED_CHARS);

    ui.separator();
    log_view(ui, &log);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Accumulation is still-and-splat only, and the form is one of the two
    /// places that has to know it. A form that could hand the renderer a
    /// `Samples::Accumulate` it would refuse is a second copy of the rule,
    /// free to drift out of step with the one the CLI enforces.
    #[test]
    fn the_form_never_asks_for_an_accumulation_the_renderer_would_refuse() {
        let base = RenderJobForm { use_accumulate: true, splat: true, ..Default::default() };
        assert_eq!(base.samples(), Samples::Accumulate { spp: base.spp });

        // The points renderer has no histogram.
        let flat = RenderJobForm { splat: false, ..RenderJobForm::default() };
        assert!(matches!(flat.samples(), Samples::Ring { .. }));

        // Every frame of an animation has its own camera.
        for mode in [Mode::Animation, Mode::ViewDescriptor] {
            let anim = RenderJobForm { mode, ..RenderJobForm::default() };
            assert!(matches!(anim.samples(), Samples::Ring { .. }), "{mode:?} must fall back to the ring");
        }

        // Falling back must not *lose* the setting.
        let anim =
            RenderJobForm { mode: Mode::Animation, use_accumulate: true, splat: true, spp: 700, ..Default::default() };
        assert!(matches!(anim.samples(), Samples::Ring { .. }));
        let back = RenderJobForm { mode: Mode::Still, ..anim };
        assert_eq!(back.samples(), Samples::Accumulate { spp: 700 });
    }

    /// A still render out of this dialog accumulates unless told otherwise.
    /// The ring's density falls as the output grows, so the old default got
    /// worse the bigger you rendered.
    #[test]
    fn a_still_defaults_to_accumulating_at_the_medium_tier() {
        let form = RenderJobForm { splat: true, ..Default::default() };
        assert_eq!(form.samples(), Samples::Accumulate { spp: 100 });
    }

    /// The `preview` preset is the form's own default, so a first render from
    /// it is the render this dialog has always defaulted to.
    #[test]
    fn the_default_preset_is_the_default_form() {
        let entry = render_preset::find(render_preset::DEFAULT).expect("default preset");
        let p = render_preset::load(&entry).unwrap();
        let form = RenderJobForm::default();
        let mut applied = form.clone();
        applied.apply(&p);
        assert_eq!(applied.to_preset(String::new()), form.to_preset(String::new()));
    }

    /// A preset saved from the form, applied to a form that differs in every
    /// way, gives back the same settings — and only those settings: the look
    /// and the machine are the window's and the box's, never a preset's.
    #[test]
    fn a_preset_carries_size_and_quality_and_nothing_else() {
        let mine = RenderJobForm {
            mode: Mode::Animation,
            format: crate::video::Format::Mp4,
            width: 1080,
            height: 1350,
            use_accumulate: false,
            spp: 777,
            accumulate: 12,
            points: 33_000_000,
            supersample: 3,
            filter: crate::gpu::Filter::Mitchell,
            filter_radius: 0.8,
            density_estimation: 0.25,
            bit_depth: crate::offline::BitDepth::Sixteen,
            fps: 24,
            quality: 71,
            ..Default::default()
        };
        let p = mine.to_preset(mine.summary());
        let other = RenderJobForm { splat: true, exposure: 3.0, gpu_share: 0.4, threads: 7, ..Default::default() };
        let mut got = other.clone();
        got.apply(&p);
        assert_eq!(got.to_preset(String::new()), mine.to_preset(String::new()));
        assert!(got.splat && got.exposure == 3.0 && got.gpu_share == 0.4 && got.threads == 7);
    }

    /// The `*` measures only what the preset says. A built-in leaves the point
    /// count to the dialog, so changing it isn't a change to the preset.
    #[test]
    fn a_preset_is_modified_only_by_what_it_says() {
        let entry = render_preset::find("hd").unwrap();
        let p = render_preset::load(&entry).unwrap();
        let mut form = RenderJobForm::default();
        form.apply(&p);
        form.preset = Some((entry, p));
        assert!(!form.preset_modified());
        form.points = 1_234_567;
        assert!(!form.preset_modified(), "hd doesn't set points");
        form.spp = 5;
        assert!(form.preset_modified());
    }

    /// Density estimation reads a density off the accumulation histogram, so
    /// a one-pass render must not carry a setting it would silently ignore —
    /// and must not carry one out of range either.
    #[test]
    fn density_estimation_needs_a_histogram_to_read() {
        let acc = RenderJobForm { use_accumulate: true, splat: true, density_estimation: 0.3, ..Default::default() };
        assert_eq!(acc.density().amount, 0.3);

        let ring = RenderJobForm { use_accumulate: false, ..Default::default() };
        assert!(RenderJobForm { density_estimation: 0.3, ..ring }.density().is_off());
        let flat = RenderJobForm { splat: false, density_estimation: 0.3, ..Default::default() };
        assert!(flat.density().is_off());
        let anim = RenderJobForm { mode: Mode::Animation, splat: true, density_estimation: 0.3, ..Default::default() };
        assert!(anim.density().is_off());

        let hot = RenderJobForm { density_estimation: 4.0, ..acc };
        assert_eq!(hot.density().amount, 1.0);
    }

    /// A directory is not a file waiting to be overwritten: nothing would be
    /// clobbered, the render simply cannot be written there.
    #[test]
    fn a_directory_is_refused_rather_than_overwritten() {
        let dir = std::env::temp_dir();
        assert_eq!(filename_trouble(&dir), Some(FilenameTrouble::IsDirectory));
        assert!(FilenameTrouble::IsDirectory.blocks_start());

        let file = dir.join("fracturize-filename-trouble.png");
        std::fs::write(&file, b"x").expect("write a temp file");
        assert_eq!(filename_trouble(&file), Some(FilenameTrouble::WillOverwrite));
        assert!(!FilenameTrouble::WillOverwrite.blocks_start());
        let _ = std::fs::remove_file(&file);

        assert_eq!(filename_trouble(&dir.join("nothing-here.png")), None);
        assert_eq!(filename_trouble(std::path::Path::new("")), None);
    }

    /// A ratio people say is said that way; one they don't is a decimal, not
    /// `427:240` or `4961:3508`.
    #[test]
    fn an_aspect_ratio_reads_as_people_say_it() {
        assert_eq!(aspect_label(1920, 1080), "16:9");
        assert_eq!(aspect_label(640, 480), "4:3");
        assert_eq!(aspect_label(1080, 1080), "1:1");
        assert_eq!(aspect_label(1080, 1350), "4:5");
        assert_eq!(aspect_label(854, 480), "1.78:1");
        assert_eq!(aspect_label(4961, 3508), "1.41:1");
    }

    /// The window keeps its vertical view in a render, so wider shows more at
    /// the sides and narrower cuts some off; matching it is one click.
    #[test]
    fn the_shape_is_judged_against_the_window() {
        assert!(against_window(1920, 1080, 1280, 720).starts_with("Same shape"));
        assert!(against_window(1920, 1080, 1000, 1000).starts_with("Wider"));
        assert!(against_window(1080, 1080, 1920, 1080).starts_with("Narrower"));
        let (w, h) = window_shaped(1920, 1440, 860);
        assert_eq!((w, h % 2), (1920, 0), "keeps the width, and an even height for video");
        assert!(against_window(w, h, 1440, 860).starts_with("Same shape"));
    }

    /// Zero samples per pixel is not a render. The slider cannot reach it, but
    /// a preset file could.
    #[test]
    fn a_sample_target_is_at_least_one() {
        let form = RenderJobForm { use_accumulate: true, splat: true, spp: 0, ..Default::default() };
        assert_eq!(form.samples(), Samples::Accumulate { spp: 1 });
    }
}
