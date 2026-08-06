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

use crate::app::App;
use crate::render_job::{
    format_duration, format_estimate, human_bytes, JobKind, JobParams,
};

use super::hints::hinted;

/// Size presets, plus the custom escape hatch.
const SIZE_PRESETS: [(&str, u32, u32); 4] = [
    ("720p", 1280, 720),
    ("1080p", 1920, 1080),
    ("1440p", 2560, 1440),
    ("4K", 3840, 2160),
];

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
    pub fps: u32,
    pub seconds: f32,
    pub quality: u8,
    /// Which animation file to write. Only read when `mode` is `Animation`,
    /// but kept across a switch to Still and back so the choice sticks.
    pub format: crate::video::Format,
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
            width: 2560,
            height: 1440,
            points: 20_000_000,
            accumulate: 96,
            splat: false,
            exposure: 1.0,
            transparent: false,
            fps: 30,
            seconds: 8.0,
            quality: 60,
            format: crate::video::Format::Avif,
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
        }
    }
}

/// Open the dialog, seeding it from what's on screen so the obvious job — "the
/// thing I'm looking at, but properly" — takes one more click.
pub fn open(app: &mut App) {
    let mut form = RenderJobForm {
        open: true,
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

    if let Some(err) = app.job_error() {
        let err = err.to_string();
        ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color).small());
    } else if let Some(done) = app.job_done() {
        let (text, color) = match done {
            Ok(p) => (format!("Wrote {}", p.display()), ui.visuals().weak_text_color()),
            Err(e) if e == crate::render_job::CANCELLED => {
                ("Cancelled — nothing was written.".to_string(), ui.visuals().weak_text_color())
            }
            Err(e) => (e.clone(), ui.visuals().error_fg_color),
        };
        ui.label(egui::RichText::new(text).color(color).small());
    }

    ui.horizontal(|ui| {
        let params = app.ui_state.render_job.params();
        let named = !params.out_path.as_os_str().is_empty();
        let resp = ui.add_enabled(named, egui::Button::new("Start"));
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
            app.start_job(params);
        }

        let resp = ui.button("Close");
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

fn draw_mode(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Output");
        for (mode, format, label, tip) in OUTPUTS {
            // An animation button is only lit when the format matches too,
            // otherwise both would look selected at once.
            let selected = app.ui_state.render_job.mode == mode
                && (mode != Mode::Animation || app.ui_state.render_job.format == format);
            let resp = ui.selectable_label(selected, label);
            let resp = hinted(resp, &mut app.ui_state, tip, "click: choose the output kind");
            if resp.clicked() && !selected {
                app.ui_state.render_job.mode = mode;
                if mode == Mode::Animation {
                    app.ui_state.render_job.format = format;
                }
                // The extension has to follow the choice, but not over a name
                // the user has typed — only the default gets rewritten.
                if !app.ui_state.render_job.filename_touched {
                    let format = app.ui_state.render_job.format;
                    app.ui_state.render_job.filename = default_filename(app, mode, format);
                }
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
    ui.horizontal(|ui| {
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
            .custom_formatter(|v, _| format!("{:.1}M", v))
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

        let animation = app.ui_state.render_job.mode == Mode::Animation;
        ui.add_enabled_ui(!animation, |ui| {
            let mut transparent = app.ui_state.render_job.transparent;
            let resp = ui.checkbox(&mut transparent, "transparent");
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
    }
}

fn draw_estimates(ui: &mut egui::Ui, app: &mut App) {
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

fn draw_running(ui: &mut egui::Ui, app: &mut App) {
    // Read what the display needs before taking `&mut` for the buttons.
    let (phase, fraction, elapsed, remaining, paused, cancelling, cancel_label, cancel_hint, log, out) = {
        let job = app.job().expect("caller checked");
        (
            job.phase,
            job.fraction(),
            job.started.elapsed().as_secs_f32(),
            job.remaining_secs(),
            job.paused(),
            job.cancelling(),
            job.cancel_arm.label("Cancel"),
            job.cancel_arm.hint("Cancel"),
            job.log.clone(),
            job.params.out_path.clone(),
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

    // Explicit available width, not `f32::INFINITY`: a `TextEdit` clamps an
    // infinite desired width to what's there, but `ProgressBar` takes it
    // literally and the auto-sized window it's in ends up with a degenerate
    // rect that never gets drawn at all.
    let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0))
        .desired_width(ui.available_width())
        .animate(fraction.is_none() && !paused);
    ui.add(bar);

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
        let resp = ui.button(if paused { "Resume" } else { "Pause" });
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

        // Three discrete labels — `Cancel` → `Cancel? wait…` → `Cancel? click
        // again` — rather than a fill animation, because this button is most
        // often reached for on a box that has stopped compositing, and a label
        // change still reads at one frame per second where a wipe does not.
        let (label, tip) = if cancelling {
            ("Cancelling…".to_string(), "The job is stopping. Nothing is written.".to_string())
        } else {
            (cancel_label, cancel_hint)
        };
        let resp = ui.add_enabled(!cancelling, egui::Button::new(label));
        let resp = hinted(resp, &mut app.ui_state, tip, "click twice: cancel the job");
        if resp.clicked() {
            if let Some(job) = app.job_mut() {
                job.click_cancel();
            }
        }

        let resp = ui.button("Close");
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
