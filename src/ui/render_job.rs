//! The render-job dialog: set up a batch render, watch it, stop it.
//!
//! Everything here exists because a big render is a commitment. The old HQ
//! render was a button with no parameters and no exit; this asks what you
//! want, tells you what it will cost before you agree to it, shows where it
//! has got to, and takes two clicks to throw away.
//!
//! Quality settings are job-scoped — changing them here doesn't touch the
//! interactive point count or prefs, so a 100M-point batch and a comfortable
//! exploration buffer can coexist.

use std::path::PathBuf;
use std::time::Instant;

use crate::app::App;
use crate::render_job::{
    format_duration, format_estimate, human_bytes, JobKind, JobParams, Outcome,
};

use super::hints::hinted;
use super::icons;

/// The app's established "this is healthy/good" green — the p99 sparkline in
/// `ui::status_bar` uses the same value for a frametime under budget. Reused
/// here rather than invented fresh, so "done" reads the same everywhere.
const DONE_GREEN: egui::Color32 = egui::Color32::from_rgb(90, 220, 120);

/// Size presets, plus the custom escape hatch.
/// Output sizes, smallest first.
///
/// The low end matters more than the high end: most renders are a check on
/// whether the framing and the point count are right, and a 640x480 answers
/// that in a fraction of the time. The list used to start at 720p, which made
/// the cheapest available answer four times the cost of the one you wanted.
const SIZE_PRESETS: [(&str, u32, u32); 6] = [
    ("SD", 640, 480),
    ("480p", 854, 480),
    ("720p", 1280, 720),
    ("1080p", 1920, 1080),
    ("1440p", 2560, 1440),
    ("4K", 3840, 2160),
];

/// A frozen record of what a job was asked to do, captured the instant
/// "Start" is clicked — not read live off the form later, because the form
/// is the *next* job's setup the moment this one is running: `draw_form`'s
/// fields stay editable under a finished job's summary so the next render can
/// be queued up without closing the dialog, and a live read would let that
/// editing relabel a job already in flight or already done.
///
/// This is what lets the completed-job panel say something a bare
/// `Result<PathBuf, String>` (the only thing `App::job_done` carries — see
/// the report) can't: what kind of job it was, how many frames, how long it
/// took. `settled_secs` is filled in exactly once, the first frame this
/// dialog observes the job is no longer running — frozen there, so the
/// figure doesn't read as a live counter for as long as the dialog happens to
/// stay open afterward.
#[derive(Clone)]
pub struct StartedJob {
    pub kind: JobKind,
    pub clicked_at: Instant,
    pub settled_secs: Option<f32>,
}

/// The dialog's in-progress form. Kept whole rather than derived from
/// `JobParams` so switching modes doesn't lose the other mode's settings.
pub struct RenderJobForm {
    pub open: bool,
    pub mode: Mode,
    pub filename: String,
    /// Set once the user edits the filename, so mode changes stop rewriting it
    pub filename_touched: bool,
    pub width: u32,
    pub height: u32,
    pub points: usize,
    pub accumulate: u32,
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
    /// CPU threads the encoder may use. Seeded from prefs in [`open`] and
    /// written back when it changes: a machine setting, so it follows the
    /// person across scenes rather than living with the job's artwork.
    pub threads: usize,
    /// A snapshot of the job launched from this form, if `Start` has been
    /// clicked and the launch wasn't rejected. See `StartedJob`.
    pub started: Option<StartedJob>,
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
            mode: Mode::Still,
            filename: String::new(),
            filename_touched: false,
            // 720p, not 1440p: the first render of a scene is usually a check
            // rather than a keeper, and a default should cost what the common
            // case is worth.
            width: 1280,
            height: 720,
            points: 20_000_000,
            accumulate: 96,
            splat: false,
            exposure: 1.0,
            transparent: false,
            // Same default as `--render`: a filtered 2x image is simply a
            // better picture than a native one of the same scene, and the
            // 4x-fill cost is the one worth paying by default.
            supersample: 2,
            filter: crate::gpu::Filter::Gaussian,
            filter_radius: crate::gpu::points::downsample::DEFAULT_FILTER_RADIUS,
            bit_depth: crate::offline::BitDepth::Eight,
            fps: 30,
            seconds: 8.0,
            quality: 60,
            format: crate::video::Format::Avif,
            threads: crate::render_job::default_threads(),
            started: None,
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

    fn params(&self) -> JobParams {
        JobParams {
            kind: self.kind(),
            out_path: PathBuf::from(self.filename.trim()),
            points: self.points,
            accumulate: self.accumulate,
            splat: self.splat,
            exposure: self.exposure,
            transparent: self.transparent,
            supersample: self.supersample,
            filter: self.filter,
            filter_radius: self.filter_radius,
            bit_depth: self.bit_depth,
            threads: self.threads,
        }
    }
}

/// Open the dialog, seeding it from what's on screen so the obvious job — "the
/// thing I'm looking at, but properly" — takes one more click.
pub fn open(app: &mut App) {
    // A job in flight: don't reseed. The form isn't on screen right now
    // (`draw_running` reads the job handle, not the form) and a full reset
    // here would wipe `started` — the snapshot the completed-job panel needs
    // — out from under a render that hasn't finished yet, if P gets pressed
    // again while it's running (a plausible reflex, not just a corner case).
    if app.job().is_some() {
        app.ui_state.render_job.open = true;
        return;
    }
    let mut form = RenderJobForm {
        open: true,
        threads: app.render_threads(),
        splat: app.render_mode == crate::app::RenderMode::Splat,
        exposure: app.exposure,
        transparent: app.transparent_render,
        // Start well above the interactive buffer without being reckless: this
        // is the number the old one-click render used.
        points: (app.point_capacity() as usize * 4).clamp(8_000_000, 40_000_000),
        ..RenderJobForm::default()
    };
    if let Some(path) = app.scene.camera_path.as_ref() {
        form.seconds = path.duration();
    }
    form.filename = default_filename(app, form.mode, form.format);
    app.ui_state.render_job = form;
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

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if !app.ui_state.render_job.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Render job")
        .id(egui::Id::new("fracturize_render_job"))
        .open(&mut open)
        .collapsible(false)
        .default_width(440.0)
        .default_pos(egui::pos2(360.0, 90.0))
        .show(ctx, |ui| {
            if app.job().is_some() {
                draw_running(ui, app);
            } else {
                draw_form(ui, app);
            }
        });
    if !open {
        app.ui_state.render_job.open = false;
    }
}

fn draw_form(ui: &mut egui::Ui, app: &mut App) {
    draw_mode(ui, app);
    ui.separator();
    draw_filename(ui, app);
    ui.separator();

    let mode = app.ui_state.render_job.mode;
    if mode != Mode::ViewDescriptor {
        draw_quality(ui, app);
        ui.separator();
        draw_estimates(ui, app);
        ui.separator();
    } else {
        ui.label(
            egui::RichText::new(
                "Writes a view file describing this exact framing — no render. \
                 Load it later with --view, or render it from the command line \
                 on a machine with time to spare.",
            )
            .small()
            .weak(),
        );
        ui.separator();
    }

    // Freeze "how long did that take" the first frame this dialog sees the
    // job is no longer running, rather than recomputing it every frame the
    // dialog happens to stay open afterward — a live counter past the finish
    // line would read as the job still going. `draw_form` (this function) is
    // only ever called once `App::job` is already `None` — see `draw`'s
    // dispatch above — so reaching here at all is the signal.
    if let Some(started) = app.ui_state.render_job.started.as_mut() {
        if started.settled_secs.is_none() {
            started.settled_secs = Some(started.clicked_at.elapsed().as_secs_f32());
        }
    }

    if let Some(err) = app.job_error() {
        let err = err.to_string();
        ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color).small());
    } else if let Some(done) = app.job_done().cloned() {
        match done {
            Ok((path, outcome)) => draw_done_panel(ui, app, &path, outcome),
            Err(e) if e == crate::render_job::CANCELLED => {
                ui.label(
                    egui::RichText::new("Cancelled — nothing was written.")
                        .color(ui.visuals().weak_text_color())
                        .small(),
                );
            }
            Err(e) => {
                ui.label(egui::RichText::new(e).color(ui.visuals().error_fg_color).small());
            }
        }
    }

    ui.horizontal(|ui| {
        let params = app.ui_state.render_job.params();
        let named = !params.out_path.as_os_str().is_empty();
        let resp =
            ui.add_enabled(named, egui::Button::new(format!("{} Start", icons::PLAY)));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if named {
                "Run this job on a second GPU device — the app stays usable while it goes"
            } else {
                "Give the output a filename first"
            },
            "click: start the render job",
        );
        if resp.clicked() {
            // Only snapshot a launch that will actually run — the same check
            // `start_job` makes internally — so a rejected click (over the
            // GPU's buffer limit, say) doesn't leave a phantom `started` for
            // a job that never existed.
            let limit = app.max_point_capacity() as u64 * crate::render_job::BYTES_PER_POINT;
            if params.rejection(limit).is_none() {
                app.ui_state.render_job.started =
                    Some(StartedJob { kind: params.kind, clicked_at: Instant::now(), settled_secs: None });
            }
            app.start_job(params);
        }

        let resp = ui.button(format!("{} Close", icons::X));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Close this window. A running job keeps going.",
            "click: close",
        );
        if resp.clicked() {
            app.ui_state.render_job.open = false;
        }
    });
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
        "Fly the camera path and encode an animated AVIF (AV1): loops like a GIF at a \
         fraction of the size, and plays in a browser. The better file — but many upload \
         pipelines won't take it.",
    ),
    (
        Mode::Animation,
        crate::video::Format::Mp4,
        "mp4",
        "Fly the camera path and encode an MP4 (H.264): bigger than the AVIF, and what \
         platforms that loop short clips actually accept. Faststart, so it plays while it \
         downloads.",
    ),
    (
        Mode::ViewDescriptor,
        crate::video::Format::Avif, // unused for a view file
        "view",
        "Save the framing to a view file instead of rendering anything",
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

fn draw_mode(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Output");
        // A segmented radio, not four loose toggles: exactly one of these is
        // always in force, and "none of them" isn't a state the dialog can be
        // in. See `ui::radio`.
        let current = Output::of(app.ui_state.render_job.mode, app.ui_state.render_job.format);
        let mut chooser = super::radio::radio(&mut app.ui_state, "render_output", current);
        for (mode, format, label, tip) in OUTPUTS {
            chooser = chooser.option(
                Output::of(mode, format),
                label,
                tip,
                "click: choose the output kind",
            );
        }
        if let Some(chosen) = chooser.show(ui) {
            let (mode, format) = OUTPUTS
                .iter()
                .find(|(m, f, ..)| Output::of(*m, *f) == chosen)
                .map(|(m, f, ..)| (*m, *f))
                .unwrap_or((Mode::Still, crate::video::Format::Avif));
            app.ui_state.render_job.mode = mode;
            if mode == Mode::Animation {
                app.ui_state.render_job.format = format;
            }
            // The extension has to follow the choice, but not over a name the
            // user has typed — only the default gets rewritten.
            if !app.ui_state.render_job.filename_touched {
                let format = app.ui_state.render_job.format;
                app.ui_state.render_job.filename = default_filename(app, mode, format);
            }
        }
    });
}

fn draw_filename(ui: &mut egui::Ui, app: &mut App) {
    let mut name = app.ui_state.render_job.filename.clone();
    let resp = ui.add(
        egui::TextEdit::singleline(&mut name)
            .desired_width(f32::INFINITY)
            .hint_text("renders/name.png"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Where the output goes. Directories are created as needed; relative paths \
         are relative to where fracturize was launched.",
        "type: the output filename",
    );
    if resp.changed() {
        app.ui_state.render_job.filename_touched = true;
    }
    app.ui_state.render_job.filename = name;

    let path = PathBuf::from(app.ui_state.render_job.filename.trim());
    if path.exists() {
        ui.label(
            egui::RichText::new("This file exists and will be overwritten.")
                .color(ui.visuals().warn_fg_color)
                .small(),
        );
    }
}

fn draw_quality(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Size");
        for (label, w, h) in SIZE_PRESETS {
            let selected =
                app.ui_state.render_job.width == w && app.ui_state.render_job.height == h;
            let resp = ui.selectable_label(selected, label);
            let resp = hinted(
                resp,
                &mut app.ui_state,
                format!("{}x{}", w, h),
                "click: use this output size",
            );
            if resp.clicked() {
                app.ui_state.render_job.width = w;
                app.ui_state.render_job.height = h;
            }
        }
    });

    ui.horizontal(|ui| {
        let mut w = app.ui_state.render_job.width;
        let resp = ui.add(egui::DragValue::new(&mut w).range(16..=16384).prefix("w "));
        let resp = hinted(resp, &mut app.ui_state, "Output width in pixels", "drag: output width");
        if resp.changed() {
            app.ui_state.render_job.width = w;
        }
        let mut h = app.ui_state.render_job.height;
        let resp = ui.add(egui::DragValue::new(&mut h).range(16..=16384).prefix("h "));
        let resp = hinted(resp, &mut app.ui_state, "Output height in pixels", "drag: output height");
        if resp.changed() {
            app.ui_state.render_job.height = h;
        }
    });

    let max_m = app.max_point_capacity() as f32 / 1e6;
    let mut millions = app.ui_state.render_job.points as f32 / 1e6;
    let resp = ui.add(
        egui::Slider::new(&mut millions, 0.5..=max_m)
            .logarithmic(true)
            // Shared with the interactive point-count slider (render_panel.rs)
            // so a count reads the same k/M units in both dialogs, and so
            // typing an exact value actually parses back — a formatter with
            // no matching parser leaves egui trying to `f64::from_str` its
            // own "1.5M" on Enter, which fails silently.
            .custom_formatter(|v, _| super::render_panel::format_points(v))
            .custom_parser(super::render_panel::parse_points)
            .text("points"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Points for *this job only* — the interactive buffer and your prefs are \
         untouched, so the app stays as responsive as it was.",
        "drag: point count for this job",
    );
    if resp.changed() {
        app.ui_state.render_job.points = (millions * 1e6).round() as usize;
    }

    let mut accum = app.ui_state.render_job.accumulate;
    let resp = ui.add(
        egui::Slider::new(&mut accum, 1..=512)
            .logarithmic(true)
            .text("accumulate"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Extra chaos-game frames after the buffer fills. More means a denser, \
         smoother render for the same point count — and proportionally more time.",
        "drag: accumulation frames",
    );
    if resp.changed() {
        app.ui_state.render_job.accumulate = accum;
    }

    // Supersampling sits with `points` and `accumulate` because all three are
    // cost/quality at fixed artistic intent — nothing here changes what the
    // picture *is*. It is the one of the three that buys the most, though, so
    // the hint says what it is rather than only what it costs.
    ui.horizontal(|ui| {
        let max = crate::gpu::points::downsample::MAX_SUPERSAMPLE;
        let mut ss = app.ui_state.render_job.supersample;
        let resp = ui.add(
            egui::DragValue::new(&mut ss).range(1..=max).prefix("supersample ").suffix("x"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Render the histogram this many times larger in each axis and filter it down. \
             The largest visible quality win here — an unfiltered render's remaining \
             harshness is aliasing, not noise, so this beats simply adding points. Costs \
             roughly N² fill and N² memory. 1 turns it off.",
            "drag: supersampling factor",
        );
        if resp.changed() {
            app.ui_state.render_job.supersample = ss;
        }

        // Greyed rather than hidden at 1x, so the kernel choice can still say
        // it exists — and `hinted` is what makes a disabled widget explain
        // itself, since egui drops `on_hover_text` on one.
        ui.add_enabled_ui(ss > 1, |ui| {
            let mut filter = app.ui_state.render_job.filter;
            egui::ComboBox::from_id_salt("render_job_filter")
                .selected_text(filter.label())
                .show_ui(ui, |ui| {
                    use clap::ValueEnum;
                    for f in crate::gpu::Filter::value_variants() {
                        ui.selectable_value(&mut filter, *f, f.label());
                    }
                });
            if filter != app.ui_state.render_job.filter {
                app.ui_state.render_job.filter = filter;
            }

            let mut radius = app.ui_state.render_job.filter_radius;
            let resp = ui.add(
                egui::DragValue::new(&mut radius)
                    .speed(0.05)
                    .range(
                        crate::gpu::points::downsample::MIN_FILTER_RADIUS
                            ..=crate::gpu::points::downsample::MAX_FILTER_RADIUS,
                    )
                    .prefix("r "),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if ss > 1 {
                    "Filter half-width in output pixels. At 0.5 with the box kernel this is \
                     exactly an N x N block average; wider trades detail for smoothness \
                     uniformly across the whole picture."
                } else {
                    "Needs supersampling above 1x — there is nothing to filter down at 1x."
                },
                "drag: filter radius",
            );
            if resp.changed() {
                app.ui_state.render_job.filter_radius = radius;
            }
        });
    });

    ui.horizontal(|ui| {
        let mut splat = app.ui_state.render_job.splat;
        let resp = ui.checkbox(&mut splat, "splat");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Render with the log-density splat renderer instead of opaque points",
            "click: toggle the splat renderer for this job",
        );
        if resp.changed() {
            app.ui_state.render_job.splat = splat;
        }

        ui.add_enabled_ui(splat, |ui| {
            let mut exposure = app.ui_state.render_job.exposure;
            let resp = ui.add(
                egui::DragValue::new(&mut exposure)
                    .speed(0.05)
                    .range(0.01..=100.0)
                    .prefix("exp "),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Splat exposure for this job",
                "drag: splat exposure",
            );
            if resp.changed() {
                app.ui_state.render_job.exposure = exposure;
            }
        });

    });

    // Bit depth rides with transparency rather than with supersampling: both
    // describe the *file* that leaves the app, not how the picture is drawn.
    // Stills only — animation is 8-bit by codec, and a control that pretended
    // otherwise would be offering something the encoder cannot take.
    let still = app.ui_state.render_job.mode == Mode::Still;
    ui.add_enabled_ui(still, |ui| {
        let mut sixteen = app.ui_state.render_job.bit_depth == crate::offline::BitDepth::Sixteen;
        let resp = ui.checkbox(&mut sixteen, "16-bit PNG");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if still {
                "Twice the file, the same render — this is only how finely it quantizes. \
                 Worth it for a keeper: supersampling produces smooth wide gradients, which \
                 is exactly what 8 bits bands into visible contours."
            } else {
                "Stills only — both video codecs take 8-bit frames."
            },
            "click: toggle 16-bit PNG output",
        );
        if resp.changed() {
            app.ui_state.render_job.bit_depth = if sixteen {
                crate::offline::BitDepth::Sixteen
            } else {
                crate::offline::BitDepth::Eight
            };
        }
    });

    // Transparency gets its own row. It was sharing a line with `splat` and its
    // exposure, which put it inside that pair's enable/disable relationship and
    // read as a third splat setting — it isn't one. It says what leaves the app
    // (an alpha channel to composite against), not how the picture is drawn.
    let animation = app.ui_state.render_job.mode == Mode::Animation;
    ui.add_enabled_ui(!animation, |ui| {
        let mut transparent = app.ui_state.render_job.transparent;
        let resp = ui.checkbox(&mut transparent, "transparent background");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if animation {
                "Not available for animation — neither AV1 nor H.264 carries an alpha plane"
            } else {
                "Write an alpha channel for compositing"
            },
            "click: toggle transparent output",
        );
        if resp.changed() {
            app.ui_state.render_job.transparent = transparent;
        }
    });

    if app.ui_state.render_job.mode == Mode::Animation {
        ui.horizontal(|ui| {
            let mut fps = app.ui_state.render_job.fps;
            let resp = ui.add(egui::DragValue::new(&mut fps).range(1..=120).suffix(" fps"));
            let resp = hinted(resp, &mut app.ui_state, "Frames per second", "drag: frame rate");
            if resp.changed() {
                app.ui_state.render_job.fps = fps;
            }

            let mut seconds = app.ui_state.render_job.seconds;
            let resp = ui.add(
                egui::DragValue::new(&mut seconds)
                    .speed(0.1)
                    .range(0.1..=600.0)
                    .suffix("s"),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Duration. Defaults to the camera path's own.",
                "drag: duration",
            );
            if resp.changed() {
                app.ui_state.render_job.seconds = seconds;
            }

            let mut quality = app.ui_state.render_job.quality;
            let codec = app.ui_state.render_job.format.codec_label();
            let resp = ui.add(egui::DragValue::new(&mut quality).range(0..=100).prefix("q "));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                format!("{} quality, 0-100. Higher is better and bigger.", codec),
                &format!("drag: {} quality", codec),
            );
            if resp.changed() {
                app.ui_state.render_job.quality = quality;
            }
        });

        // A machine setting sitting among job settings, so it says so. It is
        // in the animation block because encoding is the only thing it steers
        // today — a still's PNG deflate is one short single-threaded pass, and
        // a control that claimed to spread it would be describing work that
        // isn't there.
        ui.horizontal(|ui| {
            let max = crate::app::App::max_render_threads();
            let mut threads = app.ui_state.render_job.threads;
            let resp = ui.add(
                egui::DragValue::new(&mut threads)
                    .range(1..=max)
                    .prefix("threads "),
            );
            let resp = hinted(
                resp,
                &mut app.ui_state,
                format!(
                    "CPU threads for encoding, out of this machine's {}. The default holds \
                     one back so the desktop stays usable while a long encode runs — the AV1 \
                     flush alone is ~75x the cost of rendering a frame. Follows you across \
                     scenes, saved to prefs; never written to a scene file.",
                    max
                ),
                "drag: CPU threads for encoding",
            );
            if resp.changed() {
                app.ui_state.render_job.threads = threads;
                app.set_render_threads(threads);
            }
        });
    }
}

/// What this job inherits rather than owns.
///
/// The grade is a live control in the Render window (see `render_panel`), and
/// the job renders what you were looking at — so it is not duplicated here as
/// three more sliders. But an inherited setting that is never shown is a trap:
/// you would tune a grade, open this dialog, see no mention of it, and have no
/// way to know whether the render was about to apply it. So it is stated, and
/// only when there is something to state — a neutral grade is the absence of
/// one, and a line saying so every time would be noise.
fn draw_inherited(ui: &mut egui::Ui, app: &mut App) {
    if !app.ui_state.render_job.splat || app.grade.is_neutral() {
        return;
    }
    let g = app.grade;
    let mut parts = vec![format!("gamma {:.2}", g.gamma)];
    if g.gamma_threshold > 0.0 {
        parts.push(format!("threshold {:.2}", g.gamma_threshold));
    }
    if g.vibrancy != 1.0 {
        parts.push(format!("vibrancy {:.2}", g.vibrancy));
    }
    ui.label(
        egui::RichText::new(format!("grade: {} — from the Render window", parts.join(" · ")))
            .small()
            .weak(),
    )
    .on_hover_text(
        "The tonemap grade is a live control, so it is set where you can see it working \
         rather than here. This job will render with the grade the window is showing.",
    );
}

fn draw_estimates(ui: &mut egui::Ui, app: &mut App) {
    draw_inherited(ui, app);

    let params = app.ui_state.render_job.params();
    let limit = app.max_point_capacity() as u64 * crate::render_job::BYTES_PER_POINT;

    ui.label(
        egui::RichText::new(format!(
            "GPU memory: {} ({} of it points), limit {}",
            human_bytes(params.total_bytes()),
            human_bytes(params.point_buffer_bytes()),
            human_bytes(limit),
        ))
        .small(),
    );

    match estimate_secs(app, &params) {
        Some((low, high)) => {
            ui.label(
                egui::RichText::new(format!(
                    "Estimated time: {} ({} frame{})",
                    format_estimate(low, high),
                    params.kind.frames(),
                    if params.kind.frames() == 1 { "" } else { "s" },
                ))
                .small(),
            )
            .on_hover_text(
                "Extrapolated from this session's own measured chaos-game throughput, at a \
                 different point count and resolution — hence a range rather than a number. \
                 It gets replaced by a real one once the job has made progress.",
            );
        }
        None => {
            ui.label(
                egui::RichText::new("Estimated time: unknown until the renderer has warmed up")
                    .small()
                    .weak(),
            );
        }
    }

    if let Some(reason) = params.rejection(limit) {
        ui.label(egui::RichText::new(reason).color(ui.visuals().error_fg_color).small());
    }
}

/// A low/high seconds range for the job, from measured throughput.
///
/// Three terms, because they have wildly different weights depending on what
/// is being rendered:
///
/// * **filling** the point buffer — points × frames ÷ measured throughput;
/// * **rendering** each frame — one pass over the points, so also ÷ throughput;
/// * **encoding** each frame — pixels × a per-format constant. For a still
///   this rounds to nothing. For an animation it is the whole job.
///
/// The ±40% spread is not decoration. The throughput comes from a different
/// workload at a different point count and resolution, so the honest thing is
/// to put the uncertainty in the width of the range rather than to quote a
/// number with a decimal point on it.
fn estimate_secs(app: &App, params: &JobParams) -> Option<(f32, f32)> {
    let throughput = app.measured_throughput()?;
    if throughput <= 0.0 {
        return None;
    }
    // Warmup is roughly the buffer refilling once before accumulation starts.
    let fill_frames = params.accumulate.max(1) as f32 + 8.0;
    let fill = params.points as f32 * fill_frames / throughput;

    let (w, h) = params.kind.size();
    let pixels = w as f32 * h as f32;
    // Per-codec: H.264 encodes about an order of magnitude faster than AV1,
    // and one shared constant would misquote whichever it wasn't measured on.
    let encode_per_pixel = params.kind.secs_per_pixel();
    let per_frame = params.points as f32 / throughput + pixels * encode_per_pixel;
    let render = per_frame * params.kind.frames() as f32;

    let mid = fill + render;
    Some((mid * 0.6, mid * 1.4))
}

/// The completed-this-session state: a green check, "Render done", and
/// whatever this dialog still genuinely knows about the job that wrote
/// `path`. Deliberately distinct from the plain warning `draw_filename`
/// shows when the chosen output path merely happens to exist on disk — that
/// warning is a fact about the filesystem and is unchanged by this; this
/// panel is a fact about what this session just did, and only appears when
/// that's true.
/// The finished-job summary.
///
/// `outcome` is not decoration. A job stopped partway still writes a file, and
/// the whole reason [`Outcome`] exists is that a noisier-than-asked-for render
/// must not be presented in the same green panel as a finished one — you would
/// come back to it a week later with no way to tell. So a partial render gets
/// the warning colour, its own heading, and no check mark: the mark means
/// *done*, and this isn't.
///
/// [`Outcome`]: crate::render_job::Outcome
fn draw_done_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    path: &std::path::Path,
    outcome: Outcome,
) {
    let started = app.ui_state.render_job.started.clone();
    let is_view = matches!(started.as_ref().map(|s| s.kind), Some(JobKind::ViewDescriptor));
    let partial = outcome.is_partial();
    let accent = if partial { ui.visuals().warn_fg_color } else { DONE_GREEN };
    let heading = match (partial, is_view) {
        (true, _) => "Stopped early",
        (false, true) => "View saved",
        (false, false) => "Render done",
    };

    egui::Frame::NONE
        .fill(accent.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.6)))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if !partial {
                    check_mark(ui, accent);
                }
                ui.label(egui::RichText::new(heading).color(accent).strong());
            });
            ui.label(egui::RichText::new(format!("Wrote {}", path.display())).small());
            if partial {
                // Say what it *is*, not only that it was interrupted: the file
                // is usable, and "run it again" is the whole of the fix.
                ui.label(
                    egui::RichText::new(
                        "This is what had accumulated when you stopped it — the same picture, \
                         noisier. Run the job again to get the full one.",
                    )
                    .small()
                    .weak(),
                );
            }

            // Everything past here is a bonus, shown only when it's actually
            // known — see `StartedJob`'s doc for why frame count and elapsed
            // time aren't always available (the form can be reseeded between
            // a job starting and its summary being read), and the report for
            // what it would take to make them unconditional.
            let mut details = Vec::new();
            if let Some(s) = &started {
                if !is_view {
                    details.push(s.kind.label().to_string());
                    // `kind.frames()` is what was *asked for*, which a partial
                    // clip does not have — quoting it here would be the one
                    // number in this panel that lies. The job log carries the
                    // real count.
                    let frames = s.kind.frames();
                    if frames > 1 && !partial {
                        details.push(format!("{} frames", frames));
                    }
                }
                if let Some(secs) = s.settled_secs {
                    details.push(format!("took {}", format_duration(secs)));
                }
            }
            if let Ok(meta) = std::fs::metadata(path) {
                details.push(human_bytes(meta.len()));
            }
            if !details.is_empty() {
                ui.label(egui::RichText::new(details.join(" · ")).small().weak());
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

/// Which of the two named progress bars (if either) a job's current phase
/// name belongs to.
///
/// Coupled to the literal phase strings `src/offline.rs` passes to
/// `JobControl::phase` — `"setting up"`, `"filling points"`, `"rendering"` /
/// `"rendering frames"`, `"saving"` / `"encoding"` — because there is no
/// shared enum between that file (owned elsewhere as this is written) and
/// this dialog. An unrecognised phase (including the handle's initial
/// `"starting"`) falls back to `Setup`, which just reads both bars as
/// not-started rather than misdrawing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Setup,
    Render,
    Encode,
}

fn stage_of(phase: &str) -> Stage {
    match phase {
        "rendering" | "rendering frames" => Stage::Render,
        "encoding" | "saving" => Stage::Encode,
        _ => Stage::Setup,
    }
}

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

fn bar_state(current: Stage, bar: Stage, fraction: Option<f32>) -> BarState {
    match current.cmp(&bar) {
        std::cmp::Ordering::Less => BarState::NotStarted,
        std::cmp::Ordering::Equal => BarState::Active(fraction),
        std::cmp::Ordering::Greater => BarState::Done,
    }
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
    let (phase, fraction, elapsed, remaining, paused, cancelling, cancel_arm, log, out, kind) = {
        let job = app.job().expect("caller checked");
        (
            job.phase,
            job.fraction(),
            job.started.elapsed().as_secs_f32(),
            job.remaining_secs(),
            job.paused(),
            job.cancelling(),
            job.cancel_arm,
            job.log.clone(),
            job.params.out_path.clone(),
            job.params.kind,
        )
    };

    ui.label(egui::RichText::new(out.display().to_string()).small().weak());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(phase).strong());
        if cancelling {
            ui.label(egui::RichText::new("stopping…").color(ui.visuals().warn_fg_color));
        } else if paused {
            ui.label(egui::RichText::new("paused").color(ui.visuals().warn_fg_color));
        }
    });

    // Two named bars, both visible for the whole job, so a person watching
    // can always tell which of the two genuinely slow phases it's in and how
    // far that one has got — a single bar that hits 100% and restarts for the
    // next phase reads as either finished or lying, and used to be exactly
    // that. `progress_row`'s `ProgressBar` gets an explicit available width,
    // not `f32::INFINITY`: a `TextEdit` clamps an infinite desired width to
    // what's there, but `ProgressBar` takes it literally and the auto-sized
    // window it's in ends up with a degenerate rect that never gets drawn.
    let stage = stage_of(phase);
    progress_row(ui, "Render", bar_state(stage, Stage::Render, fraction), false);
    // A still has no encode phase — the PNG write is fast and never reports
    // progress at all, so a bar for it could never move (see the report for
    // why animation's encode bar can't either, today).
    if matches!(kind, JobKind::Animation { .. }) {
        progress_row(ui, "Encode", bar_state(stage, Stage::Encode, fraction), !paused);
    }

    ui.label(
        egui::RichText::new(match remaining {
            // Only shown once progress can support it; never counts upward,
            // because a rising countdown is worse than no countdown.
            Some(r) => format!("{} elapsed · about {} left", format_duration(elapsed), format_duration(r)),
            None => format!("{} elapsed", format_duration(elapsed)),
        })
        .small(),
    );

    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("fracturize_job_log")
        .max_height(120.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &log {
                ui.label(egui::RichText::new(line).small().monospace());
            }
        });

    ui.separator();
    ui.horizontal(|ui| {
        let resp = ui.button(if paused {
            format!("{} Resume", icons::PLAY)
        } else {
            format!("{} Pause", icons::PAUSE)
        });
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Stop the job between frames without losing it. The GPU it holds is its own, \
             so pausing frees it for the interactive renderer.",
            "click: pause / resume the job",
        );
        if resp.clicked() {
            if let Some(job) = app.job_mut() {
                let p = job.paused();
                job.set_paused(!p);
            }
        }

        // Two stages: `Cancel` arms it, it counts down disabled, then it comes
        // back as a red `Abort render`. See `ui::confirm::danger_button`.
        if cancelling {
            let resp = ui.add_enabled(
                false,
                egui::Button::new(format!("{} Aborting…", icons::TRASH)),
            );
            hinted(
                resp,
                &mut app.ui_state,
                "The job is stopping. Nothing is written.",
                "the job is stopping",
            );
        } else {
            let (arm, fired) = super::confirm::danger_button(
                ui,
                &mut app.ui_state,
                cancel_arm,
                icons::TRASH,
                "Cancel",
                "Abort render",
                "Stop the render and throw it away",
                "Throw the job away. Nothing is written, and the time it has \
                 already spent is lost.",
            );
            if let Some(job) = app.job_mut() {
                job.cancel_arm = arm;
                if fired {
                    job.cancel_now();
                }
            }
        }

        let resp = ui.button(format!("{} Close", icons::X));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Close this window — the job keeps running, and the status bar keeps the spinner",
            "click: close",
        );
        if resp.clicked() {
            app.ui_state.render_job.open = false;
        }
    });
}
