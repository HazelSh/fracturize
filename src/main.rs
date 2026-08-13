mod app;
mod avif;
mod camera;
mod haze;
mod grade_file;
mod info;
mod gpu;
mod h264;
mod history;
mod indicators;
mod mutate;
mod offline;
mod palette;
mod path;
mod prefs;
mod trace;
mod pick;
mod scene;
mod set;
mod sweep;
mod symmetry;
mod glyphs;
mod randomize;
mod record;
mod renorm;
mod render_job;
mod rot;
mod ui;
mod version;
mod video;
mod view;

use std::sync::Arc;

use clap::Parser;
use glam::{Mat4, Vec3};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
    window::{WindowAttributes, WindowId},
};

use app::App;
use scene::{Scene, TransformSpec};
use view::View;

/// How to read the option list. Shown at the foot of both `-h` and `--help`,
/// because the two conventions it describes — the value name carries the
/// type, the trailing bracket carries the fallback — are the whole reason the
/// list can stay one line per flag.
const LEGEND: &str = "\
Reading the list: the value name is the type — <FILE> is a path, <X,Y,Z> is
three comma-separated numbers, <NAME|INDEX> takes either a transform's name or
its 0-based number, <0-1> and <1-24> are ranges. A trailing [bracket] says what
you get when you leave the option out; \"the scene\" means the value in the scene
file, \"--view\" means a loaded view file's, and both are overridden by the flag.";

/// Worked examples, shown under `--help` only. `-h` is a scannable index of
/// the flags; this is the "how do I actually do the thing" half, and it is
/// cheap to read precisely because it isn't in both.
const EXAMPLES: &str = "\
Examples:
  # Open a scene in the interactive window (-s is short for --scene)
  fracturize -s scenes/blossom.toml

  # Roll a random flame you can get back later (the seed is always printed)
  fracturize --random --seed 42

  # Read a scene without rendering it: transforms, weights, framing, zoom
  fracturize --scene scenes/blossom.toml --info

  # A still, at a quality preset (-r is short for --render)
  fracturize -s scenes/blossom.toml -r out.png --effort large

  # Endless zoom about a transform, named or numbered as --info lists them
  fracturize -s scenes/rimefall.toml --zoom descent --zoom-levels 18 -r deep.png

  # Eight views around one orbit, on a single contact sheet, to pick a framing
  fracturize --scene scenes/blossom.toml --render sheet.png --orbit-grid 4x2

  # The scene plus six mutations; each variant is written out as .mutN.toml
  fracturize --scene scenes/blossom.toml --render muts.png --mutations 6

  # Animate along the scene's camera path (.mp4 is H.264, .avif loops like a GIF)
  fracturize --scene scenes/blossom.toml --render loop.mp4 --seconds 8

  # Sweep a parameter without editing the file — -S/--set is repeatable
  for w in 0.5 1 2; do
    fracturize -s s.toml -S transform.0.weight=$w -r w-$w.png
  done

Scene files live in scenes/, saved views in views/. AGENTS.md documents the
scene format and the maths behind --zoom.";

/// 3D IFS Fractal Renderer inspired by Apophysis
#[derive(Parser, Debug)]
#[command(name = "fracturize")]
#[command(
    about = "3D IFS fractal renderer: an interactive window by default, stills and \
             animation with --render",
    long_about = None,
    after_help = LEGEND,
    after_long_help = format!("{LEGEND}\n\n{EXAMPLES}"),
)]
struct Args {
    // ---------------------------------------------------------------- Scene
    /// Scene file to load, TOML [the built-in default scene]
    ///
    /// Scenes live in scenes/. --info reads one without rendering it.
    #[arg(short, long, value_name = "FILE", help_heading = "Scene")]
    scene: Option<String>,

    /// Override any scene value without editing the file, repeatable [none]
    ///
    /// The path is dotted, and the section is required: `meta.haze=0.3`,
    /// `camera.distance=4`, `zoom.edge_guard=1`, `palette.rotate=0.2`,
    /// `transform.<name-or-index>.weight=0.5`,
    /// `transform.facet-1.variations.absfold=0.15`,
    /// `transform.0.translation.y=1.25`. Arrays index by x/y/z or 0/1/2, or
    /// take a whole value (`transform.a.scale=[0.05,0.6,0.05]`). This is the
    /// general form of --palette and --zoom, and it is what makes a parameter
    /// sweep a shell loop instead of a code-generation task. A path that does
    /// not resolve is an error, never a silent no-op.
    #[arg(short = 'S', long = "set", value_name = "PATH=VALUE", help_heading = "Scene")]
    set: Vec<String>,

    /// Start from a randomly generated flame instead of a scene file
    ///
    /// Quality-checked on the CPU before it's handed back, so it always
    /// renders. Pair with --seed to reproduce a roll (the seed used is
    /// always logged); combine with --render to explore offline.
    #[arg(long, conflicts_with = "scene", help_heading = "Scene")]
    random: bool,

    /// Start from a blank scene: two plain half-scale transforms
    ///
    /// Nothing else — for building an IFS up from nothing.
    #[arg(long, conflicts_with_all = ["scene", "random"], help_heading = "Scene")]
    blank: bool,

    /// Seed for --random, --mutations and --random-palette [time-based]
    ///
    /// Whichever seed is used is printed, so any roll can be reproduced
    /// exactly by passing it back.
    #[arg(long, value_name = "INT", help_heading = "Scene")]
    seed: Option<u64>,

    // --------------------------------------------------------------- Camera
    /// Load a saved view: framing, point size, haze [the scene's own]
    ///
    /// Views live in views/; press V in-app to write one. In windowed mode the
    /// orbit starts paused; press O to resume.
    #[arg(short, long, value_name = "FILE", help_heading = "Camera")]
    view: Option<String>,

    /// Camera orbit angle, in radians [--view, else the scene]
    ///
    /// This and the four below win over both the scene and any --view, so a
    /// framing can be tried without authoring a view file for it; --render
    /// prints the [camera] block it lands on.
    #[arg(long, value_name = "RADIANS", allow_hyphen_values = true, help_heading = "Camera")]
    yaw: Option<f32>,

    /// Camera elevation, in radians; positive is above [--view, else scene]
    #[arg(long, value_name = "RADIANS", allow_hyphen_values = true, help_heading = "Camera")]
    pitch: Option<f32>,

    /// Camera orbit radius, in world units [--view, else the scene]
    #[arg(long, value_name = "UNITS", help_heading = "Camera")]
    distance: Option<f32>,

    /// Camera roll about the view axis, in radians [--view, else scene]
    #[arg(long, value_name = "RADIANS", allow_hyphen_values = true, help_heading = "Camera")]
    roll: Option<f32>,

    /// Exact framing, as a rotation vector in radians [--view, else scene]
    ///
    /// Wins over --yaw/--pitch/--roll.
    ///
    /// Those three are a chart, and a chart has poles: looking straight up or
    /// down, yaw and roll become the same control and neither means anything
    /// on its own. Since the camera can now be dragged over the pole, those
    /// framings need a way to be named — this is it. `--render` prints the
    /// form it lands on, so a framing found by hand can be pasted back.
    #[arg(
        long,
        value_name = "X,Y,Z",
        value_parser = parse_vec3,
        allow_hyphen_values = true,
        help_heading = "Camera",
    )]
    rotvec: Option<Vec3>,

    /// Render the frame the camera path reaches at `t` in 0..1 [no]
    ///
    /// The framing the animation would have at that point of its flight,
    /// as a still. Under infinite zoom this is the only way to name a frame
    /// of the flight: the descent twists while it scales, so walking
    /// `--distance` down leaves the path a little further behind every step.
    /// A scene with no authored path is flown around its full orbit, the same
    /// one `--render out.mp4` uses. The other camera flags adjust whatever
    /// this lands on.
    #[arg(long, value_name = "0-1", help_heading = "Camera")]
    path_t: Option<f32>,

    /// Camera look-at point, in world units [--view, else the scene]
    // allow_hyphen_values, or a focus with a negative coordinate — which is
    // half of them — is read as a flag and the run dies on "unexpected
    // argument '-0'".
    #[arg(
        long,
        value_name = "X,Y,Z",
        value_parser = parse_vec3,
        allow_hyphen_values = true,
        help_heading = "Camera",
    )]
    focus: Option<Vec3>,

    // --------------------------------------------------------------- Colour
    /// Colour through an independent gradient [the scene's palette]
    ///
    /// Instead of the per-transform ring: a library name (see --palettes), a
    /// palette file (.toml, or Apophysis .ugr / .gradient / .flame), or
    /// `file.ugr#name` to pick one gradient out of a collection. Overrides the
    /// scene's own [palette] — restyling a scene shouldn't require editing it,
    /// same reasoning as --zoom.
    #[arg(long, value_name = "NAME|FILE", help_heading = "Colour")]
    palette: Option<String>,

    /// Where point colour comes from: transforms, palette or mix [scene's]
    ///
    /// `transforms` spreads the per-transform RGBs around a cyclic colormap;
    /// `palette` indexes an independent gradient (the default for scenes with
    /// a [palette], and switching to `transforms` keeps the palette so the two
    /// are A/B-able); `mix` carries the transform colours through the walk as
    /// RGB so they genuinely blend and transform *combinations* become
    /// distinguishable.
    #[arg(long, value_enum, value_name = "MODE", hide_possible_values = true, help_heading = "Colour")]
    color_mode: Option<ColorModeArg>,

    /// Roll a random gradient [cosine|harmony|library; any if bare]
    ///
    /// Honours --seed and prints the [palette] table so a good roll can be
    /// pasted into a scene (same convention as --random). The generator name
    /// is optional; without one, any of them may come up.
    #[arg(
        long,
        value_name = "GENERATOR",
        num_args = 0..=1,
        default_missing_value = "any",
        help_heading = "Colour",
    )]
    random_palette: Option<String>,

    /// Shift the palette along the colour index, wraps [the scene's]
    #[arg(long, value_name = "0-1", allow_hyphen_values = true, help_heading = "Colour")]
    palette_rotate: Option<f32>,

    /// Reverse the palette's direction
    #[arg(long, help_heading = "Colour")]
    palette_reverse: bool,

    /// Interpolate control points in rgb or oklab [the scene's, else rgb]
    ///
    /// `rgb` is flam3-compatible; `oklab` is perceptually even, with no grey
    /// midpoints.
    #[arg(long, value_name = "SPACE", help_heading = "Colour")]
    palette_interpolate: Option<String>,

    // ------------------------------------------------------- Offline render
    /// Render headlessly and exit — .png, .avif or .mp4 [else a window]
    ///
    /// A .png path renders stills (and grids); a .avif or .mp4 path renders an
    /// animation along the scene's [[camera.path]] (or a full-orbit loop when
    /// the scene has none). .avif is AV1 — small, loops like a GIF; .mp4 is
    /// H.264, which is what upload pipelines accept.
    /// Prints camera mapping (for grids) and a timing breakdown to stdout.
    #[arg(short, long, value_name = "FILE", help_heading = "Offline render")]
    render: Option<String>,

    /// Output width, per tile when a grid mode is used
    #[arg(long, default_value = "1920", value_name = "PX", help_heading = "Offline render")]
    width: u32,

    /// Output height, per tile when a grid mode is used
    #[arg(long, default_value = "1080", value_name = "PX", help_heading = "Offline render")]
    height: u32,

    /// Size tier: tiny, small, medium, large or huge [the scene's point count]
    ///
    /// Target samples per output pixel: 1, 10, 100, 1000, 10000. Named for
    /// size only — a duration would depend on the machine and an outcome
    /// ("converged") would assume you have no reason to go further. Since a
    /// sample density is resolution-independent, the same tier means the same
    /// thing at 720p and 4K.
    ///
    /// For --splat this accumulates, which is the only way to actually deliver
    /// a density; --spp goes past `huge`. The points renderer has no histogram,
    /// so there a tier degrades to the nearest point buffer and says so.
    /// Explicit --points / --spp override it.
    #[arg(long, value_enum, value_name = "PRESET", hide_possible_values = true, help_heading = "Offline render")]
    effort: Option<Effort>,

    /// Point buffer capacity — more points is denser [--effort, else scene]
    ///
    /// In windowed mode the Render window's value, saved to prefs, sits
    /// between this and the scene's own point_count.
    #[arg(short, long, value_name = "N", help_heading = "Offline render")]
    points: Option<usize>,

    /// Extra chaos-game frames after the buffer fills [--effort, else 32]
    #[arg(long, value_name = "FRAMES", help_heading = "Offline render")]
    accumulate: Option<u32>,

    /// Save the pre-tonemap linear density beside the render [none]
    ///
    /// The tonemap is a pure function of this buffer plus a few scalars, so a
    /// saved one can be re-graded with --retonemap in milliseconds instead of
    /// re-rendering. 16 bytes a pixel: 33 MB at 1080p, regardless of how long
    /// the render took or how much supersampling it used. Splat, single tile.
    ///
    /// Pass a path, or bare `--grade-out` with no value to write `<render>.fgrade`.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "",
          help_heading = "Offline render")]
    grade_out: Option<String>,

    /// Re-grade a saved .fgrade buffer instead of rendering [none]
    ///
    /// Applies --exposure / --gamma / --gamma-threshold / --vibrancy to an
    /// existing linear buffer and writes the PNG named by --render. No scene,
    /// no GPU work beyond one fullscreen pass. This is how a tonemap question
    /// gets answered by looking rather than by arguing: render once, grade
    /// sixteen times.
    #[arg(long, value_name = "FILE", help_heading = "Offline render")]
    retonemap: Option<String>,

    /// With --retonemap, walk one knob across a contact sheet
    ///
    /// `--grade-sweep gamma --grade-range 1:4 --sweep-steps 9` grades the same
    /// buffer nine ways into one sheet. This is how a tonemap question gets
    /// settled by looking: the whole sheet costs one device creation and nine
    /// fullscreen passes.
    #[arg(long, value_enum, value_name = "AXIS", help_heading = "Offline render")]
    grade_sweep: Option<offline::GradeAxis>,

    /// Range for --grade-sweep, as FROM:TO [depends on the axis]
    #[arg(long, value_name = "FROM:TO", help_heading = "Offline render")]
    grade_range: Option<String>,

    /// Tonemap gamma, >1 lifts the dim end [1 = off]
    ///
    /// There is no gamma curve at all without this — just the log and a fixed
    /// gain — and gamma is most of what people mean by the Apophysis look.
    /// Pair it with --gamma-threshold: lifting the dim end also lifts the
    /// single-sample speckle in the background, and the threshold is what
    /// stops that becoming a grey veil.
    #[arg(long, value_name = "G", help_heading = "Offline render")]
    gamma: Option<f32>,

    /// Coverage below which gamma flattens to a straight line [0 = off]
    ///
    /// The toe that keeps --gamma from lifting background speckle into a veil.
    ///
    /// Measured useful range is 0.05-0.5, which is *not* flam3's range for the
    /// nominally same knob: flam3's threshold is in raw density units, this one
    /// is in post-log coverage, where the whole scale is 0-1. On blossom at
    /// --gamma 2.5, threshold 0.5 recovers 80% of the lifted background for 5%
    /// of the bright detail.
    #[arg(long, value_name = "T", help_heading = "Offline render")]
    gamma_threshold: Option<f32>,

    /// How gamma reaches colour: 1 keeps hue, 0 rolls highlights to white [1]
    ///
    /// Inert at --gamma 1, because there is then no curve to route.
    #[arg(long, value_name = "V", help_heading = "Offline render")]
    vibrancy: Option<f32>,

    /// Target samples per output pixel, accumulated without a ceiling
    ///
    /// The ordinary render splats the point buffer once, so its sample count is
    /// that buffer's capacity and no amount of extra time changes it. With
    /// --spp the buffer becomes a working set: it is refilled and folded into a
    /// persistent histogram until the target is met, so quality is bounded by
    /// time rather than by memory. Needs --splat, and renders one still —
    /// contact sheets and animations would be one accumulation run per tile.
    ///
    /// Cancelling keeps the work: exposure is normalized by what was actually
    /// accumulated, so a run stopped at 40% is the same picture, noisier.
    #[arg(long, value_name = "N", help_heading = "Offline render")]
    spp: Option<u32>,

    /// Supersampling factor, 1-4 [2]
    ///
    /// Render the histogram at N x output resolution and filter down. The
    /// single biggest visible quality win available: at equal sample count a
    /// supersampled render beats a native one decisively, because what is
    /// wrong with an unfiltered histogram is aliasing rather than noise. Costs
    /// roughly N² fill. 1 turns it off.
    #[arg(long, value_name = "N", help_heading = "Offline render")]
    supersample: Option<u32>,

    /// Reconstruction kernel for the downsample [gaussian]
    ///
    /// Not lanczos by default: its negative lobes ring around small bright
    /// cores, which is what a flame image is full of.
    #[arg(long, value_enum, value_name = "KERNEL", hide_possible_values = true, help_heading = "Offline render")]
    filter: Option<crate::gpu::Filter>,

    /// Filter half-width in output pixels, 0.5-2.0 [0.5]
    ///
    /// At 0.5 with --filter box this is exactly an N x N block average. The
    /// tap radius in accumulation pixels is this times --supersample.
    #[arg(long, value_name = "PX", help_heading = "Offline render")]
    filter_radius: Option<f32>,

    /// Bits per channel in a PNG render: 8 or 16 [8]
    ///
    /// The render is identical either way — this is only how finely the file
    /// quantizes it. Worth 16 for a keeper, because supersampling produces
    /// exactly what 8 bits bands: smooth wide gradients, where the step
    /// between adjacent codes shows as a contour. Ignored for animation,
    /// which is 8-bit by codec.
    #[arg(long, value_enum, value_name = "BITS", help_heading = "Offline render")]
    bit_depth: Option<offline::BitDepth>,

    /// Chaos-game seed — a different sample stream [0]
    ///
    /// The chaos game is deterministic: the same command has always produced
    /// byte-identical output, because every walker was seeded from its index
    /// alone. This is how to ask for an *independent* deal of the same
    /// attractor — the same picture, sampled differently. Distinct from
    /// `--seed`, which seeds mutation and `--random`.
    #[arg(long, value_name = "N", help_heading = "Offline render")]
    chaos_seed: Option<u64>,

    /// Report GPU-busy time per chaos dispatch
    ///
    /// Measurement, not a render setting. Answers whether the chaos loop is
    /// bound by per-dispatch overhead or by the GPU itself — if the median is
    /// flat as the batch grows, bigger batches would amortize; if it scales,
    /// there is nothing to win. Silently does nothing on a device without
    /// TIMESTAMP_QUERY.
    #[arg(long, help_heading = "Offline render")]
    gpu_timing: bool,

    /// CPU threads for encoding [one less than this machine has]
    ///
    /// Describes the box, not the artwork: never scene or view data. The
    /// default holds a core back so the desktop stays usable while a long
    /// animation encodes.
    #[arg(long, value_name = "N", help_heading = "Offline render")]
    threads: Option<usize>,

    /// Use the splat renderer: additive log-density accumulation
    ///
    /// Flame-style, instead of opaque points. R toggles it in-app, and a view
    /// saved in splat mode selects it on load.
    #[arg(long, help_heading = "Offline render")]
    splat: bool,

    /// Splat-renderer exposure multiplier [--view, else 1.0]
    ///
    /// W / Shift+W adjust it in-app.
    #[arg(long, value_name = "MULT", help_heading = "Offline render")]
    exposure: Option<f32>,

    /// Render with a transparent background, for compositing
    ///
    /// The PNG gets an alpha channel carrying the fractal's own coverage. Not
    /// supported for animation output — neither AV1 nor H.264 carries an alpha
    /// plane here.
    #[arg(long, help_heading = "Offline render")]
    transparent: bool,

    /// Turn on atmospheric haze at the legacy default strength
    ///
    /// A scene's own `haze` value wins over this; see "Haze" in AGENTS.md.
    #[arg(long, help_heading = "Offline render")]
    fog: bool,

    // ------------------------------------------------------------ Animation
    /// Frame rate for .avif / .mp4 output
    #[arg(long, default_value = "30", value_name = "N", help_heading = "Animation")]
    fps: u32,

    /// Animation duration [the path's own: path_seconds, or 3s a segment]
    #[arg(long, value_name = "SECONDS", help_heading = "Animation")]
    seconds: Option<f32>,

    /// Higher is better quality and a bigger file
    ///
    /// Maps to the AV1 quantizer for .avif and to H.264's QP for .mp4.
    #[arg(long, default_value = "60", value_name = "0-100", help_heading = "Animation")]
    quality: u8,

    // ------------------------------------------------------- Contact sheets
    /// Sheet of views spaced around a full orbit, e.g. 4x2 [one view]
    ///
    /// One fill of the point buffer is shared by all tiles.
    #[arg(long, value_name = "COLSxROWS", help_heading = "Contact sheets")]
    orbit_grid: Option<String>,

    /// Sheet of views nudged across the view plane, e.g. 3x3 [one view]
    ///
    /// Left/right (columns) and up/down (rows), all still looking at the
    /// focus.
    #[arg(long, value_name = "COLSxROWS", help_heading = "Contact sheets")]
    move_grid: Option<String>,

    /// Nudge per --move-grid step, in orbit distances
    #[arg(long, default_value = "0.25", value_name = "FRACTION", help_heading = "Contact sheets")]
    move_step: f32,

    /// Sheet of the scene plus N mutations, tile 0 = original [no sheet]
    ///
    /// Each variant is saved as <out>.mutN.toml and described on stdout.
    /// Mutually exclusive with the camera grids.
    #[arg(long, value_name = "1-24", help_heading = "Contact sheets")]
    mutations: Option<u32>,

    /// Scale factor for --mutations perturbations
    #[arg(long, default_value = "1.0", value_name = "SCALE", help_heading = "Contact sheets")]
    mutation_strength: f32,

    /// Sheet varying one scene value, repeatable up to twice [no sheet]
    ///
    /// Takes a --set path and either a range or a list. Join paths with `+`
    /// to move them in lockstep (`transform.a.weight+transform.b.weight=0.5:2`),
    /// which is what you want when several maps must stay equal.
    /// `transform.facet-1.variations.absfold=0.05:0.55` walks --sweep-steps
    /// values between the ends; `palette.name=ember,abyss,peacock` uses the
    /// list verbatim (checked for first, so a value containing a comma is
    /// never read as a range). Give it twice and the first varies across
    /// columns, the second down rows. Composes with --set, which sets the
    /// base every tile starts from. Needs --scene, and each tile refills the
    /// point buffer, so prefer --effort tiny/small.
    #[arg(long, value_name = "PATH=A:B|A,B,C", help_heading = "Contact sheets")]
    sweep: Vec<String>,

    /// Values per --sweep range; ignored by the list form
    #[arg(long, default_value = "5", value_name = "2-12", help_heading = "Contact sheets")]
    sweep_steps: usize,

    /// Don't draw parameter labels into contact sheet tiles
    ///
    /// Labels are on by default: a sheet's per-tile parameters also go to
    /// stdout, but anything reading the PNG can't see those.
    #[arg(long, help_heading = "Contact sheets")]
    no_labels: bool,

    // -------------------------------------------------------- Infinite zoom
    /// Endless, scale-invariant zoom about a transform [the scene's]
    ///
    /// Names the map to renormalize about, either by its name in the scene or
    /// by its index (0-based, as --info lists them). The attractor then has no
    /// biggest or smallest feature, and zoom that never runs out. The map must
    /// be pure affine and contract on all three axes. Overrides the scene's
    /// [zoom]. See "Infinite Zoom" in AGENTS.md.
    #[arg(long, value_name = "NAME|INDEX", help_heading = "Infinite zoom")]
    zoom: Option<String>,

    /// Octaves of scale rendered [the scene's, else 15]
    ///
    /// More = deeper before the core empties out, at the cost of density in
    /// each one.
    #[arg(long, requires = "zoom", value_name = "OCTAVES", help_heading = "Infinite zoom")]
    zoom_levels: Option<f32>,

    /// Outer radius of the band, in camera distances [scene's, else 4.8]
    ///
    /// Below 2.42 the band's edge enters the frustum.
    #[arg(long, requires = "zoom", value_name = "MULTIPLE", help_heading = "Infinite zoom")]
    zoom_radius: Option<f32>,

    /// Point-budget falloff toward the fixed point [scene's, else 0 = flat]
    ///
    /// Given as a power of the contraction ratio. Non-zero makes a wrap step
    /// the density; it is for stills.
    #[arg(long, requires = "zoom", value_name = "POWER", help_heading = "Infinite zoom")]
    zoom_falloff: Option<f32>,

    /// Octaves the picture's outer edge fades over [scene's, else 1]
    ///
    /// The edge guard: material is taken to nothing over the outermost octave
    /// of the field, measured against the camera, so a wrap costs nothing and
    /// old structure leaves at a steady rate. 0 restores the hard edge, where
    /// a wrap drops a whole octave between two frames — for measuring the
    /// artifact, not for looking at. See "Infinite Zoom" in AGENTS.md.
    #[arg(
        long,
        alias = "zoom-fade",
        requires = "zoom",
        value_name = "OCTAVES",
        help_heading = "Infinite zoom"
    )]
    zoom_guard: Option<f32>,

    // ----------------------------------------------------------- Inspecting
    /// Print what this scene is, then exit
    ///
    /// Eleven labelled sections, no GPU device. Read `notes` first: it is
    /// every diagnostic the report found, or the word `none`, so "is this
    /// scene sound?" is one line rather than fifty.
    ///
    /// Then `shape` — where the attractor actually lands, with the camera
    /// distance and point size that measurement implies, printed as -S flags
    /// you can paste. Then the transforms with their share of the walk,
    /// contraction and rendered colour; the render and colour properties; the
    /// camera and its path; and the infinite-zoom band, or every map eligible
    /// to carry one as a --zoom command.
    ///
    /// Add --view or -S and it reports those too: what each one set, and what
    /// each value replaced. Add --color for a painted gradient.
    #[arg(short, long, help_heading = "Inspecting")]
    info: bool,

    /// List the built-in palette library, one per line, and exit
    #[arg(long, help_heading = "Inspecting")]
    palettes: bool,

    /// Paint --info and --palettes with 24-bit ANSI colour
    ///
    /// Off by default. Both commands are read far more often through a pipe,
    /// by something that receives escape codes as literal bytes and cannot see
    /// a gradient, than by a person at a terminal — and on a typical scene the
    /// swatch alone was a third of the report's bytes.
    ///
    /// This adds to the output rather than replacing part of it: the hex stops
    /// stay exactly where they are and wear their own colours, and --info
    /// gains a continuous swatch. Everything you get without it, you also get
    /// with it.
    #[arg(long, help_heading = "Inspecting")]
    color: bool,

    /// Analyze a rendered image and report statistics
    ///
    /// Reads a PNG file and reports: mean/max luminance, percentage of clipped
    /// (white) and empty (black) pixels, and a color-index histogram. This
    /// catches the monochrome-palette failure mode (a spike in the histogram)
    /// that is invisible in the picture itself.
    #[arg(long, value_name = "IMAGE", help_heading = "Inspecting")]
    stats: Option<std::path::PathBuf>,

    // -------------------------------------------------------- Windowed mode
    /// Capture a screenshot and exit after --delay frames
    #[arg(long, help_heading = "Windowed mode")]
    screenshot: bool,

    /// Frames to wait before screenshot capture
    #[arg(long, default_value = "120", value_name = "FRAMES", help_heading = "Windowed mode")]
    delay: u32,

    /// Disable vsync (uncapped frame rate, useful for benchmarking)
    #[arg(long, help_heading = "Windowed mode")]
    no_vsync: bool,
}

/// Parse `--focus x,y,z`
fn parse_vec3(s: &str) -> Result<Vec3, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return Err(format!("expected three comma-separated numbers, got '{}'", s));
    }
    let mut v = [0.0f32; 3];
    for (i, p) in parts.iter().enumerate() {
        v[i] = p.parse().map_err(|_| format!("'{}' is not a number", p))?;
    }
    Ok(Vec3::from(v))
}

impl Args {
    fn camera_override(&self) -> camera::CameraOverride {
        camera::CameraOverride {
            yaw: self.yaw,
            pitch: self.pitch,
            rotvec: self.rotvec,
            distance: self.distance,
            roll: self.roll,
            focus: self.focus,
            path_t: self.path_t,
        }
    }
}

/// Apply `--zoom` / `--zoom-levels` / `--zoom-radius` over whatever the scene
/// authored. Fails loudly rather than silently rendering without the feature
/// that was asked for: a scene that quietly isn't infinite looks like a bug in
/// the maths, and that is an expensive thing to go looking for.
fn apply_zoom_args(scene: &mut Scene, args: &Args, announce: bool) {
    if let Some(reference) = &args.zoom {
        let map = match scene::resolve_transform_ref(reference, &scene.transform_names) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("--zoom: {}", e);
                std::process::exit(1);
            }
        };
        scene.zoom = Some(renorm::ZoomSpec { map, ..scene.zoom.clone().unwrap_or_default() });
    }
    let Some(spec) = scene.zoom.as_mut() else { return };
    if let Some(l) = args.zoom_levels {
        spec.levels = l;
    }
    if let Some(r) = args.zoom_radius {
        spec.radius = r;
    }
    if let Some(f) = args.zoom_falloff {
        spec.octave_falloff = f;
    }
    if let Some(w) = args.zoom_guard {
        spec.edge_guard = w;
    }
    // Resolve once here so a bad map is a startup error with a clear message,
    // not a silently-disabled feature discovered in the output.
    match renorm::Renorm::build(spec, &scene.transforms, scene.camera_distance) {
        Ok(r) if announce => {
            let name = scene.transform_names.get(r.map).cloned().flatten();
            println!("{}", r.summary(name.as_deref()));
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("infinite zoom: {}", e);
            std::process::exit(1);
        }
    }
}

/// `--color-mode`, as clap sees it.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum ColorModeArg {
    Transforms,
    Palette,
    Mix,
}

impl From<ColorModeArg> for scene::ColorMode {
    fn from(a: ColorModeArg) -> Self {
        match a {
            ColorModeArg::Transforms => scene::ColorMode::Transforms,
            ColorModeArg::Palette => scene::ColorMode::Palette,
            ColorModeArg::Mix => scene::ColorMode::Mix,
        }
    }
}

/// Apply the `--palette*` flags over whatever the scene authored.
///
/// Order matters and is the same one the scene format uses: pick a gradient,
/// then adjust it. So `--palette ember --palette-reverse` reverses ember, and
/// a scene's own `reverse = true` is *replaced* rather than compounded when a
/// new palette is named — otherwise "give me ember" would silently hand back
/// ember-backwards depending on what the file happened to say.
///
/// Like `--zoom`, failures exit rather than falling back: a run that quietly
/// rendered the scene's original colours would look like the flag did nothing.
fn apply_palette_args(scene: &mut Scene, args: &Args, announce: bool) {
    let rolled = args.random_palette.as_ref().map(|which| {
        let generator = match which.as_str() {
            "any" => None,
            name => Some(palette::random::Generator::parse(name).unwrap_or_else(|| {
                eprintln!(
                    "--random-palette: unknown generator '{}'. Available: any, {}",
                    name,
                    palette::random::Generator::ALL
                        .iter()
                        .map(|g| g.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                std::process::exit(1);
            })),
        };
        let (p, seed) = roll_palette(generator, args.seed);
        if announce {
            println!("Random palette seed: {} ({})", seed, p.describe());
            println!("{}", palette::spec::PaletteDef::from_palette(&p, true).to_toml_fragment());
        }
        p
    });

    let named = args.palette.as_ref().map(|spec| {
        resolve_palette_arg(spec).unwrap_or_else(|e| {
            eprintln!("--palette: {}", e);
            std::process::exit(1);
        })
    });

    // A rolled palette and a named one is a contradiction; clap can't express
    // "either but not both" across an Option pair as cleanly as just saying so.
    if rolled.is_some() && named.is_some() {
        eprintln!("--palette and --random-palette both set a gradient; pick one");
        std::process::exit(1);
    }

    if let Some(p) = named.or(rolled) {
        scene.set_palette(p);
    }

    if let Some(p) = scene.palette.as_mut() {
        if let Some(r) = args.palette_rotate {
            p.rotate = r.rem_euclid(1.0);
        }
        if args.palette_reverse {
            p.reverse = !p.reverse;
        }
        if let Some(space) = &args.palette_interpolate {
            p.interpolate = palette::Interpolate::parse(space).unwrap_or_else(|| {
                eprintln!(
                    "--palette-interpolate: expected {}",
                    palette::Interpolate::ALL
                        .iter()
                        .map(|i| i.name())
                        .collect::<Vec<_>>()
                        .join(" or ")
                );
                std::process::exit(1);
            });
        }
    } else if args.palette_rotate.is_some() || args.palette_reverse
        || args.palette_interpolate.is_some()
    {
        eprintln!("--palette-rotate / --palette-reverse / --palette-interpolate need a palette: \
                   pass --palette or --random-palette, or use a scene with a [palette] table");
        std::process::exit(1);
    }

    // Last, so it can override the mode a --palette implied — `--palette ember
    // --color-mode transforms` loads ember and renders without it, which is
    // exactly the A/B the mode flag exists for.
    if let Some(mode) = args.color_mode {
        scene.set_color_mode(mode.into());
    }

    scene.regenerate_colormap();
}

/// A `--palette` value: a library name, a file, or `file#gradient-name`.
fn resolve_palette_arg(spec: &str) -> Result<palette::Palette, String> {
    // A bare library name wins over a same-named file, because the library is
    // the thing people will type by far the most often.
    let bare = spec.split('#').next().unwrap_or(spec);
    if !bare.contains(['/', '\\', '.']) {
        if let Some(p) = palette::library::get(bare) {
            return Ok(p);
        }
    }
    if std::path::Path::new(bare).exists() {
        return palette::import::load_one(spec);
    }
    Err(format!(
        "'{}' is neither a library palette nor a file. Library: {}",
        spec,
        palette::library::names().join(", ")
    ))
}

/// Roll a palette, honouring `--seed` and returning the seed used so it can
/// be printed and reproduced.
fn roll_palette(
    generator: Option<palette::random::Generator>,
    seed: Option<u64>,
) -> (palette::Palette, u64) {
    use rand::SeedableRng;
    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let p = match generator {
        Some(g) => palette::random::from(g, &mut rng),
        None => palette::random::palette(&mut rng),
    };
    (p, seed)
}

/// `--palettes`: the library, one per line. Printing twenty-four floats and
/// asking someone to imagine the gradient is not a listing, so each one comes
/// with its gradient — as hex stops, which survive a pipe, and with `--color`
/// as a swatch too.
fn print_palette_library(color: bool) {
    println!("{} built-in palettes:\n", palette::library::len());
    let width = palette::library::names().iter().map(|n| n.len()).max().unwrap_or(8);
    for p in palette::library::all() {
        let name = p.name.clone().unwrap_or_default();
        let blurb = palette::library::blurb(&name).unwrap_or("");
        match color {
            true => {
                println!("  {:<width$}  {}  {}", name, info::swatch(&p, 32), blurb, width = width)
            }
            false => println!("  {:<width$}  {}", name, blurb, width = width),
        }
        println!("  {:<width$}  {}", "", info::hex_ramp(&p, 8), width = width);
    }
    println!(
        "\nUse with --palette <name>. Apophysis .ugr / .gradient / .flame files\n\
         work too: --palette mygradients.ugr#twilight"
    );
}

/// Analyze a rendered image and print statistics about it.
///
/// Reports luminance distribution, clipping, and color-index histogram.
/// The color histogram catches the monochrome-palette failure mode that is
/// invisible in the picture itself (CRAFT §2.3's trap).
fn print_image_stats(path: &std::path::Path) {
    let img = match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("Failed to open '{}': {}", path.display(), e);
            std::process::exit(1);
        }
    };

    let (w, h) = img.dimensions();
    let total = (w * h) as f64;
    if total == 0.0 {
        eprintln!("Image has zero pixels");
        std::process::exit(1);
    }

    println!("stats    {}", path.display());
    println!("         {}x{} ({:.1}M pixels)", w, h, total / 1e6);

    // Luminance analysis
    let mut sum_lum = 0.0f64;
    let mut max_lum = 0.0f32;
    let mut clipped = 0u64; // R=G=B=255
    let mut empty = 0u64; // R=G=B=0 and A>0 (black), or A=0 (transparent)

    // Color histogram (quantized to 16 bins for readability)
    let mut hue_hist = [0u64; 16];
    let mut saturation_sum = 0.0f64;
    let mut saturated_pixels = 0u64;

    for pixel in img.pixels() {
        let [r, g, b, a] = pixel.0;
        let rf = r as f32 / 255.0;
        let gf = g as f32 / 255.0;
        let bf = b as f32 / 255.0;

        // Relative luminance (sRGB)
        let lum = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
        sum_lum += lum as f64;
        max_lum = max_lum.max(lum);

        if r == 255 && g == 255 && b == 255 {
            clipped += 1;
        }
        if (r == 0 && g == 0 && b == 0) || a == 0 {
            empty += 1;
        }

        // Hue and saturation for color distribution
        let max_c = rf.max(gf).max(bf);
        let min_c = rf.min(gf).min(bf);
        let delta = max_c - min_c;

        if delta > 0.05 {
            // Has some color
            saturated_pixels += 1;
            saturation_sum += (delta / max_c.max(0.001)) as f64;

            let hue = if max_c == rf {
                60.0 * (((gf - bf) / delta) % 6.0)
            } else if max_c == gf {
                60.0 * (((bf - rf) / delta) + 2.0)
            } else {
                60.0 * (((rf - gf) / delta) + 4.0)
            };
            let hue = if hue < 0.0 { hue + 360.0 } else { hue };
            let bin = ((hue / 360.0 * 16.0) as usize).min(15);
            hue_hist[bin] += 1;
        }
    }

    let mean_lum = sum_lum / total;
    let clipped_pct = clipped as f64 / total * 100.0;
    let empty_pct = empty as f64 / total * 100.0;

    println!();
    println!("lumin    mean {:.3}   max {:.3}", mean_lum, max_lum);
    println!(
        "         clipped {:>5.1}%   empty {:>5.1}%",
        clipped_pct, empty_pct
    );

    if clipped_pct > 5.0 {
        println!("         note: >5% clipped — consider lowering exposure");
    }
    if empty_pct > 80.0 {
        println!("         note: >80% empty — attractor may be too small in frame");
    }

    // Color distribution
    println!();
    if saturated_pixels > 0 {
        let mean_sat = saturation_sum / saturated_pixels as f64;
        println!(
            "color    mean saturation {:.2}   colored pixels {:>5.1}%",
            mean_sat,
            saturated_pixels as f64 / total * 100.0
        );

        // Find peaks in hue histogram
        let max_bin = *hue_hist.iter().max().unwrap_or(&0);
        if max_bin > 0 {
            let threshold = max_bin / 4;
            let peaks: Vec<usize> = hue_hist
                .iter()
                .enumerate()
                .filter(|&(_, &c)| c > threshold)
                .map(|(i, _)| i)
                .collect();

            if peaks.len() <= 2 && saturated_pixels as f64 / total > 0.1 {
                println!(
                    "         note: color concentrated in {} bin{} — may be monochrome trap",
                    peaks.len(),
                    if peaks.len() == 1 { "" } else { "s" }
                );
            }

            // Print histogram as sparkline
            let sparkline: String = hue_hist
                .iter()
                .map(|&c| {
                    let level = (c as f64 / max_bin as f64 * 7.0) as usize;
                    ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][level.min(7)]
                })
                .collect();
            println!("         hue histogram: {}", sparkline);
            // Tick letters sit under the bin their hue actually lands in:
            // 16 bins of 22.5°, so red 0° -> 0, yellow 60° -> 2, green
            // 120° -> 5, cyan 180° -> 8, blue 240° -> 10, magenta 300° -> 13.
            let mut ticks = [b' '; 16];
            for (hue_deg, letter) in
                [(0, b'R'), (60, b'Y'), (120, b'G'), (180, b'C'), (240, b'B'), (300, b'M')]
            {
                ticks[hue_deg * 16 / 360] = letter;
            }
            println!("                        {}", String::from_utf8_lossy(&ticks).trim_end());
        }
    } else {
        println!("color    no saturated pixels (grayscale or empty)");
    }
}

/// The scene a run starts from: `--scene`, else `--blank`, `--random`, or the
/// built-in default. One place, so `--info` reports on exactly what `--render`
/// would draw.
/// `--sweep`: build the tile list, then hand `offline` a closure that turns a
/// tile's extra `--set` arguments into a scene.
///
/// The closure matters. A tile's scene has to go through exactly the pipeline
/// the base scene did — load, `--zoom`, `--palette`, point count — and that
/// pipeline lives here. Reloading from disk inside `offline` would quietly drop
/// every one of those flags.
fn run_sweep(
    params: offline::OfflineParams,
    args: &Args,
    grid: offline::GridMode,
    effort_points: Option<usize>,
) -> Result<render_job::Outcome, String> {
    if !matches!(grid, offline::GridMode::Single) {
        eprintln!("--sweep cannot be combined with --orbit-grid/--move-grid");
        std::process::exit(1);
    }
    if args.mutations.is_some() {
        eprintln!("--sweep cannot be combined with --mutations");
        std::process::exit(1);
    }
    let Some(scene_path) = args.scene.clone() else {
        eprintln!("--sweep needs --scene: it varies values in a scene file");
        std::process::exit(1);
    };

    let axes: Vec<sweep::Axis> = args
        .sweep
        .iter()
        .map(|spec| sweep::parse_axis(spec, args.sweep_steps))
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| {
            eprintln!("{}", e);
            std::process::exit(1);
        });
    let (tiles, cols, rows) = sweep::tiles(&axes).unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(1);
    });

    let points = args.points.or(effort_points);
    let build = |extra: &[String]| -> Result<Scene, String> {
        let mut sets = args.set.clone();
        sets.extend_from_slice(extra);
        let mut scene = Scene::load_with(&scene_path, &sets)?;
        // Quiet: the base scene already reported its zoom and palette once.
        apply_zoom_args(&mut scene, args, false);
        apply_palette_args(&mut scene, args, false);
        if let Some(n) = points {
            scene.point_count = n;
        }
        Ok(scene)
    };

    offline::render_sweep(params, &tiles, cols, rows, &build)
}

fn load_scene(args: &Args) -> Scene {
    let (scene, _, seed) = load_scene_reporting(args);
    // Every caller but `--info` gets the seed here, on its own line. `--info`
    // carries it inside the report instead, so a captured report keeps it.
    if let Some(seed) = seed {
        println!("Random flame seed: {}", seed);
    }
    scene
}

/// [`load_scene`], plus what each `--set` displaced and the seed a `--random`
/// roll came from. Only `--info` wants the rest of the tuple; everything that
/// renders wants the scene.
fn load_scene_reporting(args: &Args) -> (Scene, Vec<set::Applied>, Option<u64>) {
    match &args.scene {
        Some(path) => {
            let (scene, applied) =
                Scene::load_reporting(path, &args.set).unwrap_or_else(|e| {
                    eprintln!("Failed to load scene '{}': {}", path, e);
                    std::process::exit(1);
                });
            (scene, applied, None)
        }
        None if args.blank => (Scene::blank(), warn_set_cannot_apply(args), None),
        None if args.random => {
            let (scene, seed) = random_scene(args.seed);
            (scene, warn_set_cannot_apply(args), Some(seed))
        }
        None => (default_scene(), warn_set_cannot_apply(args), None),
    }
}

/// `-S` patches a scene file's TOML text before it is parsed, so it has nothing
/// to reach on a scene that was generated rather than read.
///
/// `--set` promises that a path which doesn't resolve is an error and never a
/// silent no-op; this is the other half of that promise. Returns the empty
/// applied-list so the caller reads as a plain arm.
fn warn_set_cannot_apply(args: &Args) -> Vec<set::Applied> {
    if !args.set.is_empty() {
        eprintln!(
            "warning: --set had no effect. It rewrites a scene file's TOML before \n\
             parsing, and --blank/--random/the built-in default have no file to \n\
             rewrite. Save the scene first, or edit it after loading."
        );
    }
    Vec::new()
}

/// Roll a random flame for `--random`, honouring `--seed` and logging the
/// seed either way so any roll can be reproduced exactly.
///
/// The seed comes back rather than only going to stdout: `--info` carries it
/// inside the report, where a capture to a file keeps it.
fn random_scene(seed: Option<u64>) -> (Scene, u64) {
    use rand::SeedableRng;
    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let scene = randomize::random_flame(&mut rng);
    log::info!("Random flame seed: {} (reproduce with --random --seed {})", seed, seed);
    (scene, seed)
}

/// Supersampling for an offline render when nobody said otherwise.
///
/// 2 rather than 1, deliberately, and it changes what every `--render` writes:
/// the measurements say an unfiltered histogram's remaining harshness is
/// aliasing, not shot noise, so a filtered 2x render is simply a better picture
/// than a native one of the same scene at the same sample count. 2 rather than
/// 4 because it costs 4x fill instead of 16x, and the step from 1 to 2 is the
/// one you can see across the board. `--supersample 1` restores the old
/// behaviour exactly.
const DEFAULT_SUPERSAMPLE: u32 = 2;

/// Size tiers for offline rendering.
///
/// Named for **size and nothing else**. The previous ladder had `draft` and
/// `ultra` and then an accumulating tier that wanted calling `overnight` or
/// `converged`, and both of those names were promises the program cannot keep:
/// a duration depends on the machine (this runs on a GTX 1080 *and* a T490),
/// and "converged" asserts an outcome the user might have their own reasons to
/// push past. A size says what you asked for and nothing about what you get.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Effort {
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
}

impl Effort {
    /// Target **samples per output pixel** — the tier's actual meaning.
    ///
    /// Round decades from 1 to 10,000: four orders of magnitude across five
    /// tiers. `--spp` goes further for anything past `huge`, deliberately,
    /// because past here the right number is a judgement about a particular
    /// picture rather than a tier anyone can name.
    ///
    /// Samples *per pixel* rather than a raw point count because it is
    /// resolution-independent: the same tier means the same density at 720p and
    /// at 4K, which a point count cannot do. Apophysis users think this way too.
    fn spp(self) -> u32 {
        match self {
            Effort::Tiny => 1,
            Effort::Small => 10,
            Effort::Medium => 100,
            Effort::Large => 1_000,
            Effort::Huge => 10_000,
        }
    }

    /// The point buffer to back that target with.
    ///
    /// Two different jobs, so two different answers:
    ///
    /// * **Accumulating**, the ring is a *working set* whose only job is keeping
    ///   the GPU busy between folds. Bigger is not better past a point, and the
    ///   memory is worth more to the histogram (32 bytes a texel), which is what
    ///   decides whether a large supersampled render runs at all. Capped at 20M.
    /// * **Ring path**, the buffer *is* the sample count, so it has to hold the
    ///   whole target — and it caps at 100M, which is the ceiling slice 4b
    ///   exists to remove. Above `small` at 1080p the cap bites and the tier
    ///   cannot be delivered without accumulating; `--effort` says so out loud
    ///   rather than quietly under-rendering.
    fn points(self, pixels: u64, accumulating: bool) -> usize {
        let want = self.spp() as u64 * pixels;
        let cap = if accumulating { 20_000_000 } else { 100_000_000 };
        want.clamp(1_000_000, cap) as usize
    }

    /// Whether the ring path can actually reach this tier at this size.
    fn reachable_without_accumulating(self, pixels: u64) -> bool {
        self.spp() as u64 * pixels <= 100_000_000
    }
}

/// Parse `--grade-range FROM:TO`, defaulting per axis.
///
/// The defaults are the measured useful ranges rather than the clamp bounds:
/// a sweep is for looking at the part of the range where something happens,
/// and `--gamma-threshold` in particular has a range that is *not* flam3's
/// (it is in post-log coverage here). See AGENTS.md.
fn parse_grade_range(axis: offline::GradeAxis, spec: Option<&str>) -> Result<(f32, f32), String> {
    let default = match axis {
        offline::GradeAxis::Exposure => (0.5, 3.0),
        offline::GradeAxis::Gamma => (1.0, 4.0),
        offline::GradeAxis::GammaThreshold => (0.0, 0.5),
        offline::GradeAxis::Vibrancy => (0.0, 1.0),
    };
    let Some(spec) = spec else { return Ok(default) };
    let (a, b) = spec
        .split_once(':')
        .ok_or_else(|| format!("Invalid --grade-range '{}': expected FROM:TO, e.g. 1:4", spec))?;
    let from: f32 = a.trim().parse().map_err(|_| format!("Invalid range start '{}'", a))?;
    let to: f32 = b.trim().parse().map_err(|_| format!("Invalid range end '{}'", b))?;
    Ok((from, to))
}

/// Parse a "COLSxROWS" grid spec like "4x2"
fn parse_grid(spec: &str) -> Result<(u32, u32), String> {
    let (c, r) = spec
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("Invalid grid '{}': expected COLSxROWS, e.g. 4x2", spec))?;
    let cols: u32 = c.trim().parse().map_err(|_| format!("Invalid grid columns '{}'", c))?;
    let rows: u32 = r.trim().parse().map_err(|_| format!("Invalid grid rows '{}'", r))?;
    if cols == 0 || rows == 0 || cols * rows > 64 {
        return Err(format!("Grid {}x{} out of range (1..=64 tiles)", cols, rows));
    }
    Ok((cols, rows))
}

/// Wrapper to handle winit's async initialization pattern.
///
/// Owns the egui layer alongside `app` (rather than `App` owning it): the
/// per-frame UI closure needs `&mut App`, so `AppWrapper` must be able to
/// split-borrow `self.app` and `self.egui` independently.
struct AppWrapper {
    app: Option<App>,
    egui: Option<ui::EguiLayer>,
    args: Args,
}

impl AppWrapper {
    fn new(args: Args) -> Self {
        Self { app: None, egui: None, args }
    }
}

impl ApplicationHandler for AppWrapper {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }

        let window_attrs = WindowAttributes::default()
            .with_title("Fracturize - 3D IFS Fractal Renderer")
            // Roomy enough for the toolbar, a couple of floating panels and
            // the status bar without eating a 1080p desktop whole.
            .with_inner_size(PhysicalSize::new(1440u32, 860u32));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("Failed to create window"),
        );

        // Load scene - panic if provided path fails, use default if no path given
        let mut scene = match &self.args.scene {
            Some(path) => Scene::load_with(path, &self.args.set).unwrap_or_else(|e| {
                panic!("Failed to load scene '{}': {}", path, e);
            }),
            None if self.args.blank => Scene::blank(),
            None if self.args.random => {
                let (scene, seed) = random_scene(self.args.seed);
                println!("Random flame seed: {}", seed);
                scene
            }
            None => {
                log::info!("No scene specified, using built-in default");
                default_scene()
            }
        };
        // Point count is a render property, not scene data (todo.txt): the
        // Render window edits it and persists it to prefs, so it follows the
        // person across scenes. Precedence: --points > prefs > the scene
        // file's own point_count > the built-in default. `App::new` re-reads
        // prefs for its own copy; this read is only for the decision.
        if let Some(n) = self.args.points {
            scene.point_count = n;
        } else if let Some(n) = crate::prefs::Prefs::load().point_count {
            scene.point_count = n;
        }
        apply_zoom_args(&mut scene, &self.args, true);
        apply_palette_args(&mut scene, &self.args, true);

        let view = self.args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| panic!("Failed to load view '{}': {}", path, e))
        });

        // Create app (blocking on async)
        let app = pollster::block_on(App::new(
            window.clone(),
            scene,
            self.args.fog,
            !self.args.no_vsync,
            self.args.scene.clone(),
            view,
            self.args.splat,
            self.args.exposure,
        ));

        self.egui = Some(ui::EguiLayer::new(&window, &app.gpu.device, app.gpu.format));
        self.app = Some(app);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(app) = self.app.as_mut() else {
            return;
        };
        let Some(egui) = self.egui.as_mut() else {
            return;
        };

        // egui must see every event first — with one deliberate exception.
        // egui-winit's own doc comment on `EventResponse::consumed` says it
        // straight out: "egui uses `tab` to move focus between elements, so
        // this will always be `true` for tabs." Forwarding an unclaimed Tab
        // is what *creates* the claim: when nothing is focused, egui's own
        // focus-navigation (`memory/mod.rs`'s `FocusDirection::Next` branch)
        // hands keyboard focus to the first widget that wants it, purely
        // because a Tab keydown arrived — no click needed. That invented
        // focus then makes `egui_wants_keyboard_input()` true and silently
        // gates off every other keybind (G, O, Z, S, W, …) until something
        // surrenders it. So for Tab specifically, the gate is read *before*
        // forwarding — reading it after would only report the state that
        // forwarding itself just produced. When something legitimately has
        // focus already (typing in the scene name, Save-as, the palette
        // editor, a focused button), Tab is forwarded exactly as before and
        // egui's own focus navigation keeps working untouched.
        let suppress_tab = matches!(
            &event,
            WindowEvent::KeyboardInput { event: key, .. }
                if matches!(key.physical_key, PhysicalKey::Code(KeyCode::Tab))
        ) && !(egui.ctx.egui_wants_keyboard_input() || egui.ctx.egui_is_using_pointer());
        let resp = if suppress_tab {
            Default::default()
        } else {
            egui.state.on_window_event(&app.window, &event)
        };

        // Modifiers and cursor position always update app state, regardless
        // of whether egui consumed the event or a drag is in flight.
        if let WindowEvent::ModifiersChanged(mods) = &event {
            app.shift_held = mods.state().shift_key();
            app.ctrl_held = mods.state().control_key();
            app.alt_held = mods.state().alt_key();
        }
        if let WindowEvent::CursorMoved { position, .. } = &event {
            // Suppress gizmo hover picking while the pointer is over an egui
            // area, unless the app is already mid-drag (active viewport
            // drags keep receiving motion even over panels).
            let suppress_hover = egui.ctx.is_pointer_over_egui() && !app.has_active_drag();
            app.on_cursor_moved(position.x as f32, position.y as f32, suppress_hover);
        }

        match event {
            WindowEvent::CloseRequested => {
                // Asks rather than exits: with unsaved edits this puts up the
                // prompt and the actual exit happens (or doesn't) once it's
                // been answered.
                app.request_quit();
            }

            WindowEvent::ModifiersChanged(_) => {} // handled above

            WindowEvent::CursorMoved { .. } => {} // handled above

            WindowEvent::MouseInput { state, button, .. } => {
                let is_release = !state.is_pressed();
                let gated = resp.consumed || egui.ctx.egui_wants_pointer_input();
                // A release always reaches the app while a drag is active,
                // so drags ending over a panel don't stick.
                if !gated || (is_release && app.has_active_drag()) {
                    if state.is_pressed() {
                        app.on_mouse_press(button);
                    } else {
                        app.on_mouse_release(button);
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let gated = resp.consumed || egui.ctx.egui_wants_pointer_input();
                if !gated {
                    let steps = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                    };
                    app.on_scroll(steps);
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // egui owns keyboard focus (e.g. typing into a text field),
                // OR the pointer is mid-drag on an egui widget (e.g. a
                // Slider — which doesn't take keyboard focus, only pointer
                // capture) — either way, don't also run keybinds. Without
                // the second check, S/F/D etc. leak through while dragging
                // a Render-window slider.
                if egui.ctx.egui_wants_keyboard_input() || egui.ctx.egui_is_using_pointer() {
                } else if event.state.is_pressed() {
                    // Handle special keys by physical key (layout-independent)
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            // Escape means *cancel the thing in front of me*,
                            // and nothing else. It used to quit, which made the
                            // reflex that dismisses a popup the reflex that
                            // closed the app with an hour of work in it. Quit
                            // is Ctrl+Q and the window's close button now.
                            //
                            // The fall-through runs outermost-first and ends by
                            // dropping the transform selection — which is also
                            // the only way that selection could ever be
                            // cleared, and so the exit from Up/Down being stuck
                            // on "step transforms" forever.
                            if app.ui_state.transform_menu.is_some() {
                                app.ui_state.transform_menu = None;
                            } else if app.pending_action.is_some() {
                                app.pending_action = None;
                            } else if app.ui_state.save_as.is_open() {
                                app.ui_state.save_as = Default::default();
                            } else if app.ui_state.render_job.open {
                                app.ui_state.render_job.open = false;
                            } else if app.show_browser {
                                app.toggle_browser();
                            } else if app.selected_transform().is_some() {
                                app.select_transform(None);
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowUp) => {
                            if app.show_browser {
                                app.browser_move(false);
                            } else if app.selected_transform().is_some() {
                                app.select_prev_transform();
                            } else {
                                app.zoom_in();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            if app.show_browser {
                                app.browser_move(true);
                            } else if app.selected_transform().is_some() {
                                app.select_next_transform();
                            } else {
                                app.zoom_out();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            if app.show_browser {
                                app.browser_load_selected();
                            } else if app.selected_transform().is_some() {
                                app.toggle_selected_transform();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Delete) => {
                            app.delete_selected_transform();
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Home) => {
                            // "Frame selected", on the key everyone binds it to.
                            app.frame_selected_transform();
                            return;
                        }
                        PhysicalKey::Code(KeyCode::F1) => {
                            // The universal "what are the keys" key, alongside
                            // this app's own H.
                            app.toggle_help();
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Tab) => {
                            // Second binding for G (below), on the key most
                            // editors reach for to flip an "edit mode" on and
                            // off. Sits behind the same wants-keyboard gate as
                            // every other binding here, which is enough: egui
                            // treats Tab as its own focus-navigation key, and
                            // `egui_wants_keyboard_input` is true whenever any
                            // widget — not just a text field — holds focus, so
                            // a focused button or slider keeps Tab for itself.
                            app.toggle_gizmos();
                            return;
                        }
                        _ => {}
                    }

                    // Handle letter keys by logical key (respects keyboard layout)
                    match &event.logical_key {
                        Key::Named(NamedKey::Space) => {
                            // Same action as O and Z: start/stop the camera
                            // flying its path. No suppression block needed
                            // here the way Tab needed one above — egui only
                            // turns Space into a click when a widget already
                            // holds focus (`Context::interact`'s
                            // `memory.has_focus(id)` check), which is exactly
                            // what `egui_wants_keyboard_input()` tests, so the
                            // existing gate above already covers it.
                            app.toggle_camera_motion();
                        }
                        Key::Character(c) => match c.as_str() {
                            "s" | "S" => {
                                if app.ctrl_held && app.shift_held {
                                    ui::save_as::open(app);
                                } else if app.ctrl_held {
                                    app.save_scene();
                                } else {
                                    app.request_screenshot();
                                    log::info!("Screenshot requested");
                                }
                            }
                            "f" | "F" => {
                                app.adjust_haze_intensity(!app.shift_held);
                            }
                            "d" | "D" => {
                                app.adjust_color_falloff(!app.shift_held);
                            }
                            "c" | "C" => {
                                app.adjust_color_contrast(app.shift_held);
                            }
                            "g" | "G" => {
                                app.toggle_gizmos();
                            }
                            "h" | "H" | "?" => {
                                app.toggle_help();
                            }
                            "o" | "O" => {
                                if app.ctrl_held {
                                    // Ctrl+O opens, everywhere in the world.
                                    // The scene browser is what "open" shows.
                                    app.toggle_browser();
                                } else {
                                    app.toggle_camera_motion();
                                }
                            }
                            "n" | "N" => {
                                if app.ctrl_held {
                                    app.new_blank_scene();
                                }
                            }
                            "q" | "Q" => {
                                if app.ctrl_held {
                                    app.request_quit();
                                }
                            }
                            "z" | "Z" => {
                                // Ctrl must be checked first: Ctrl+Z / Ctrl+Shift+Z
                                // are undo/redo, not the path-play toggle.
                                if app.ctrl_held {
                                    if app.shift_held {
                                        app.redo();
                                    } else {
                                        app.undo();
                                    }
                                } else {
                                    // Same action as O. One motion, so one verb
                                    // — two keys for it, because both were in
                                    // people's fingers before they merged.
                                    app.toggle_camera_motion();
                                }
                            }
                            "y" | "Y" => {
                                if app.ctrl_held {
                                    // Ctrl+Y is Redo on Windows, so it's what
                                    // an Apophysis refugee's hand reaches for.
                                    // It used to toggle the camera-path loop —
                                    // an edit Ctrl+Z couldn't take back, which
                                    // is the worst thing a mistaken redo could
                                    // possibly have done. The loop is fully
                                    // served by the Camera window's four-way
                                    // radio, which can reach all four states
                                    // where the keystroke could reach one.
                                    app.redo();
                                } else if app.shift_held {
                                    app.remove_path_key();
                                } else {
                                    app.add_path_key();
                                }
                            }
                            "v" | "V" => {
                                app.save_view();
                            }
                            "[" => {
                                app.adjust_point_size(false);
                            }
                            "]" => {
                                app.adjust_point_size(true);
                            }
                            "a" | "A" => {
                                app.add_transform(app.shift_held);
                            }
                            "b" | "B" => {
                                app.toggle_browser();
                            }
                            "p" | "P" => {
                                ui::render_job::open(app);
                            }
                            "i" | "I" => {
                                app.toggle_invert_pitch();
                            }
                            "x" | "X" => {
                                app.toggle_traces(app.shift_held);
                            }
                            "u" | "U" => {
                                if app.shift_held {
                                    // Shift+U aliases the unified undo (kept for
                                    // muscle memory; identical to Ctrl+Z).
                                    app.undo();
                                } else {
                                    app.mutate_scene();
                                }
                            }
                            "," | "<" => {
                                app.adjust_weight(false);
                            }
                            "." | ">" => {
                                app.adjust_weight(true);
                            }
                            "j" | "J" => {
                                app.adjust_color(0, !app.shift_held);
                            }
                            "k" | "K" => {
                                app.adjust_color(1, !app.shift_held);
                            }
                            "l" | "L" => {
                                app.adjust_color(2, !app.shift_held);
                            }
                            "e" | "E" => {
                                app.cycle_variation(!app.shift_held);
                            }
                            "r" | "R" => {
                                app.toggle_render_mode();
                            }
                            "w" | "W" => {
                                app.adjust_exposure(!app.shift_held);
                            }
                            "-" | "_" => {
                                app.adjust_variation_weight(false);
                            }
                            "=" | "+" => {
                                app.adjust_variation_weight(true);
                            }
                            _ => {}
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::Resized(new_size) => {
                app.resize(new_size.width, new_size.height);
                app.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                app.update();
                // Last frame's egui layout is what says whether the pointer is
                // parked on a panel; a frame of lag is nothing against a
                // two-second timer, and asking before `run_ui` keeps this out
                // of the middle of the frame's own bookkeeping.
                app.update_cursor_visibility(egui.ctx.is_pointer_over_egui());

                // Handle --screenshot mode: take screenshot after delay and exit
                if self.args.screenshot && app.frame_count == self.args.delay {
                    app.request_screenshot();
                }

                // Frame flow: gather input -> run the egui pass -> hand
                // platform output back -> tessellate -> render (egui pass
                // replaces the old text-overlay pass inside App::render).
                let ui_start = std::time::Instant::now();
                let raw_input = egui.state.take_egui_input(&app.window);
                let full_output = egui.ctx.run_ui(raw_input, |ui| {
                    ui::draw(ui, app);
                });
                let t_run = ui_start.elapsed();
                egui.state.handle_platform_output(&app.window, full_output.platform_output);
                let t_plat = ui_start.elapsed();
                let pixels_per_point = full_output.pixels_per_point;
                let paint_jobs = egui.ctx.tessellate(full_output.shapes, pixels_per_point);
                static UI_PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                let ui_profile =
                    *UI_PROFILE.get_or_init(|| std::env::var_os("FRACTURIZE_UI_PROFILE").is_some());
                if ui_profile && app.frame_count % 120 == 0 {
                    let t_tess = ui_start.elapsed();
                    log::info!(
                        "ui: run_ui {:.2}ms, platform_output {:.2}ms, tessellate {:.2}ms",
                        t_run.as_secs_f32() * 1000.0,
                        (t_plat - t_run).as_secs_f32() * 1000.0,
                        (t_tess - t_plat).as_secs_f32() * 1000.0,
                    );
                }
                // The UI's own CPU cost, surfaced in the status bar: this is a
                // performance-sensitive app and "are the panels costing me
                // frames?" needs an answer you can read, not guess at.
                app.record_ui_time(ui_start.elapsed().as_secs_f32() * 1000.0);

                match app.render(
                    &mut egui.renderer,
                    &paint_jobs,
                    &full_output.textures_delta,
                    pixels_per_point,
                ) {
                    app::FrameOutcome::Presented | app::FrameOutcome::Skip => {}
                    app::FrameOutcome::Reconfigure => {
                        let (w, h) = app.gpu.size();
                        app.resize(w, h);
                    }
                }

                // Exit after screenshot in --screenshot mode
                if self.args.screenshot && app.frame_count > self.args.delay {
                    event_loop.exit();
                    return;
                }

                // Quitting is asked for, not done: Ctrl+Q, the window's close
                // button and the unsaved-changes prompt all set this flag, and
                // the loop leaves here — after the frame that put the prompt on
                // screen has actually been presented.
                if app.exit_requested {
                    event_loop.exit();
                    return;
                }

                // Request next frame
                app.window.request_redraw();
            }

            _ => {}
        }
    }
}

/// Default Sierpinski tetrahedron scene
fn default_scene() -> Scene {
    // Generate colormap from transform colors
    let colors = [
        Vec3::new(1.0, 0.2, 0.2), // Red
        Vec3::new(0.2, 1.0, 0.2), // Green
        Vec3::new(0.2, 0.2, 1.0), // Blue
        Vec3::new(1.0, 1.0, 0.2), // Yellow
    ];

    let mut colormap = [[0.0f32; 4]; 256];
    for i in 0..256 {
        let t = i as f32 / 255.0;
        let scaled = t * 3.0;
        let idx0 = (scaled.floor() as usize).min(2);
        let idx1 = idx0 + 1;
        let local_t = scaled - idx0 as f32;
        let c = colors[idx0] * (1.0 - local_t) + colors[idx1] * local_t;
        colormap[i] = [c.x, c.y, c.z, 1.0];
    }

    // Legacy elevated-eye framing, folded onto the orbit sphere
    let default_cam = camera::OrbitCamera::from_legacy(
        Vec3::ZERO,
        Vec3::new(0.0, 1.0, 0.0),
        3.0,
        0.0,
        0.0,
        0.0,
    );

    Scene {
        name: "Default Sierpinski".to_string(),
        author: "Claude Opus 4.5 (Claude Code 2.0.76)".to_string(),
        point_size: 0.002,
        points_per_frame: 100_000,
        point_count: 500_000,
        point_count_defaulted: true,
        decay: 0.8,
        color_speed: 0.5,
        color_falloff: 0.0,
        color_contrast: 1.0,
        haze: 0.0,
        exposure: 1.0,
        transform_names: vec![None; 4],
        colors: colors.to_vec(),
        transforms: [
            (Vec3::new(0.0, 0.0, 0.5), 0.0),      // red
            (Vec3::new(0.0, 0.47, -0.17), 0.333), // green
            (Vec3::new(-0.41, -0.24, -0.17), 0.667), // blue
            (Vec3::new(0.41, -0.24, -0.17), 1.0), // yellow
        ]
        .iter()
        .map(|&(translation, color_value)| TransformSpec {
            matrix: Mat4::from_scale_rotation_translation(
                Vec3::splat(0.5),
                glam::Quat::IDENTITY,
                translation,
            ),
            post_affine: Mat4::IDENTITY,
            color_value,
            weight: 1.0,
            color_speed: 0.5,
            explicit_color_speed: None,
            symmetry: None,
            variations: TransformSpec::linear_variations(),
        })
        .collect(),
        palette: None,
        color_mode: scene::ColorMode::Transforms,
        colormap,
        camera_focus: default_cam.focus,
        camera_distance: default_cam.distance,
        camera_orientation: default_cam.orientation,
        background: scene::DEFAULT_BACKGROUND,
        camera_path: None,
        zoom: None,
    }
}

fn main() {
    // Initialize logging
    env_logger::init();

    // Parse CLI args
    let args = Args::parse();

    // --palettes: show the library and stop. No scene, no device.
    if args.palettes {
        print_palette_library(args.color);
        return;
    }

    // --stats: analyze a rendered image and report metrics
    if let Some(path) = &args.stats {
        print_image_stats(path);
        return;
    }

    // --info: read a scene and say what it is, without opening a device
    if args.info {
        let source = args
            .scene
            .clone()
            .unwrap_or_else(|| match (args.blank, args.random) {
                (true, _) => "--blank".to_string(),
                (_, true) => "--random".to_string(),
                _ => "built-in default".to_string(),
            });
        // Reporting rather than plain loading, so the report can say what each
        // --set displaced. Nothing that renders wants the second half.
        let (mut scene, applied, seed) = load_scene_reporting(&args);
        // --info reports the zoom in its own section; don't say it twice
        apply_zoom_args(&mut scene, &args, false);
        // --info prints the palette in its own section; don't say it twice
        apply_palette_args(&mut scene, &args, false);
        // A --view is part of "what would render", so --info reports through
        // it rather than describing a framing the next --render wouldn't use.
        let view = args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| {
                eprintln!("Failed to load view '{}': {}", path, e);
                std::process::exit(1);
            })
        });
        let mut subject = info::Subject::new(&scene, &source)
            .with_camera(args.camera_override())
            .with_set(&applied)
            .with_seed(seed)
            .with_color(args.color);
        if let (Some(path), Some(v)) = (args.view.as_deref(), view.as_ref()) {
            subject = subject.with_view(path, v);
        }
        print!("{}", info::report(&subject));
        return;
    }

    // Re-grading needs no scene, so it comes before any scene is loaded.
    if let Some(src) = &args.retonemap {
        let Some(out) = &args.render else {
            eprintln!("--retonemap needs --render to say where the PNG goes");
            std::process::exit(1);
        };
        match offline::retonemap(
            std::path::Path::new(src),
            std::path::Path::new(out),
            args.exposure,
            args.gamma,
            args.gamma_threshold,
            args.vibrancy,
            args.bit_depth.unwrap_or_default(),
            match args.grade_sweep {
                Some(axis) => match parse_grade_range(axis, args.grade_range.as_deref()) {
                    Ok((from, to)) => Some(offline::GradeSweep {
                        axis,
                        from,
                        to,
                        steps: args.sweep_steps.max(1),
                    }),
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                },
                None => None,
            },
            !args.no_labels,
        ) {
            Ok(_) => return,
            Err(e) => {
                eprintln!("Re-grade failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Headless render mode: no window, no event loop
    if let Some(out) = &args.render {
        let mut scene = load_scene(&args);
        apply_zoom_args(&mut scene, &args, true);
        apply_palette_args(&mut scene, &args, true);

        // Effort presets set points + accumulation; explicit flags win
        // A tier names a sample density; accumulating is the only way to
        // actually deliver one, so `--effort` implies it for splat renders. The
        // points renderer has no histogram to accumulate into, so there the
        // tier degrades to the buffer that best approximates it.
        let pixels = args.width as u64 * args.height as u64;
        let splat_render = args.splat
            || args.view.as_ref().is_some_and(|p| {
                View::load(p).map(|v| v.is_splat()).unwrap_or(false)
            });
        let accumulating = args.spp.is_some() || (args.effort.is_some() && splat_render);
        let effort_points = args.effort.map(|e| e.points(pixels, accumulating));
        let effort_spp = args.effort.and_then(|e| splat_render.then(|| e.spp()));
        if let Some(e) = args.effort {
            if !splat_render && !e.reachable_without_accumulating(pixels) {
                eprintln!(
                    "note: --effort {:?} is {} samples/px, which at {}x{} needs a {}M point \
                     buffer — past the {}M cap. The points renderer has no histogram to \
                     accumulate into, so this renders at the cap. Use --splat for the real tier.",
                    e, e.spp(), args.width, args.height,
                    e.spp() as u64 * pixels / 1_000_000, 100,
                );
            }
        }
        if let Some(n) = args.points.or(effort_points) {
            scene.point_count = n;
        }
        let accumulate = args.accumulate.unwrap_or(offline::DEFAULT_ACCUMULATE);
        let spp = args.spp.or(effort_spp);

        let view = args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| panic!("Failed to load view '{}': {}", path, e))
        });

        // Flags over the view over neutral, field by field — the same ladder
        // `exposure` above uses, and per-field so a view that sets only gamma
        // does not drag the other two along with it.
        let from_view = view.as_ref().map(|v| v.grade()).unwrap_or_default();
        let grade = gpu::points::splat::Grade {
            gamma: args.gamma.unwrap_or(from_view.gamma),
            gamma_threshold: args.gamma_threshold.unwrap_or(from_view.gamma_threshold),
            vibrancy: args.vibrancy.unwrap_or(from_view.vibrancy),
        };

        let grid = match (&args.orbit_grid, &args.move_grid) {
            (Some(_), Some(_)) => {
                eprintln!("--orbit-grid and --move-grid are mutually exclusive");
                std::process::exit(1);
            }
            (Some(spec), None) => match parse_grid(spec) {
                Ok((cols, rows)) => offline::GridMode::Orbit { cols, rows },
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            },
            (None, Some(spec)) => match parse_grid(spec) {
                Ok((cols, rows)) => offline::GridMode::Move {
                    cols,
                    rows,
                    step: args.move_step,
                },
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            },
            (None, None) => offline::GridMode::Single,
        };

        // --splat wins; otherwise a splat-captured view selects the renderer
        let splat = args.splat || view.as_ref().is_some_and(|v| v.is_splat());
        let exposure = args
            .exposure
            .or(view.as_ref().and_then(|v| v.exposure))
            .unwrap_or(1.0);

        let params = offline::OfflineParams {
            scene,
            view,
            width: args.width,
            height: args.height,
            out_path: std::path::Path::new(out),
            accumulate,
            haze_enabled: args.fog,
            grid,
            splat,
            exposure,
            transparent: args.transparent,
            control: None,
            camera: args.camera_override(),
            // A single tile has nothing to be told apart from.
            labels: !args.no_labels,
            supersample: args
                .supersample
                .unwrap_or(DEFAULT_SUPERSAMPLE)
                .clamp(1, crate::gpu::points::downsample::MAX_SUPERSAMPLE),
            filter: args.filter.unwrap_or_default(),
            filter_radius: args.filter_radius.unwrap_or(
                crate::gpu::points::downsample::DEFAULT_FILTER_RADIUS,
            ),
            bit_depth: args.bit_depth.unwrap_or_default(),
            scene_path: args.scene.as_ref().map(std::path::PathBuf::from),
            // The CLI never reads prefs for render parameters — a headless
            // render stays reproducible from flags plus scene — so this is the
            // flag or the machine default, nothing else.
            threads: args
                .threads
                .map(|n| n.max(1))
                .unwrap_or_else(render_job::default_threads),
            gpu_timing: args.gpu_timing,
            chaos_seed: args.chaos_seed.unwrap_or(gpu::points::compute::DEFAULT_SEED),
            spp,
            grade,
            grade_out: args.grade_out.as_ref().map(|p| {
                // Bare `--grade-out` means "beside the render".
                if p.is_empty() {
                    crate::grade_file::GradeBuffer::path_for(std::path::Path::new(out))
                } else {
                    std::path::PathBuf::from(p)
                }
            }),
        };
        // The extension picks the codec as well as the container: .avif is
        // AV1, .mp4 is H.264. Anything else is a still.
        let format = crate::video::Format::from_path(std::path::Path::new(out));
        let result = if let Some(format) = format {
            if !matches!(grid, offline::GridMode::Single)
                || args.mutations.is_some()
                || !args.sweep.is_empty()
            {
                eprintln!(
                    "animation (.{}) cannot be combined with grid, mutation or sweep sheets",
                    format.extension(),
                );
                std::process::exit(1);
            }
            if args.transparent {
                // Fail rather than quietly hand back opaque video: the frames
                // are converted to YUV with r/g/b only, and neither codec here
                // carries an alpha plane.
                eprintln!(
                    "--transparent is not supported for .{} output ({} has no alpha plane \
                     here). Render a PNG sequence instead, or drop --transparent.",
                    format.extension(),
                    format.codec_label(),
                );
                std::process::exit(1);
            }
            offline::render_animation(
                params,
                offline::AnimParams {
                    fps: args.fps,
                    seconds: args.seconds,
                    quality: args.quality,
                    format,
                },
            )
        } else {
            match (args.sweep.is_empty(), args.mutations) {
                (false, _) => run_sweep(params, &args, grid, effort_points),
                (true, Some(n)) => {
                    if !matches!(grid, offline::GridMode::Single) {
                        eprintln!("--mutations cannot be combined with --orbit-grid/--move-grid");
                        std::process::exit(1);
                    }
                    if n == 0 || n > 24 {
                        eprintln!("--mutations must be 1..=24");
                        std::process::exit(1);
                    }
                    offline::render_mutations(params, n, args.mutation_strength, args.seed)
                }
                (true, None) => offline::render(params),
            }
        };
        if let Err(e) = result {
            eprintln!("Offline render failed: {}", e);
            std::process::exit(1);
        }
        return;
    }

    match &args.scene {
        Some(path) => log::info!("Starting Fracturize with scene: {}", path),
        None => log::info!("Starting Fracturize with built-in default scene"),
    }

    // Create event loop
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    // Run application
    let mut app_wrapper = AppWrapper::new(args);
    event_loop
        .run_app(&mut app_wrapper)
        .expect("Event loop error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Args::command().debug_assert();
    }

    /// `-h` is meant to be read top to bottom by a person, so every flag has
    /// to belong to a section — an ungrouped one lands in a stray "Options:"
    /// block above them all, where it reads as more important than it is.
    #[test]
    fn every_option_lives_in_a_section() {
        let cmd = Args::command();
        let loose: Vec<_> = cmd
            .get_arguments()
            .filter(|a| a.get_id() != "help" && a.get_help_heading().is_none())
            .map(|a| a.get_id().to_string())
            .collect();
        assert!(loose.is_empty(), "no help_heading on: {}", loose.join(", "));
    }

    /// The summary is the whole of `-h`, one line per flag. Past ~78
    /// characters it wraps on a normal terminal and the column stops being
    /// scannable — the detail belongs after the blank line, where only
    /// `--help` shows it.
    #[test]
    fn summaries_fit_on_one_line() {
        let cmd = Args::command();
        let long: Vec<_> = cmd
            .get_arguments()
            .filter_map(|a| {
                let help = a.get_help()?.to_string();
                (help.len() > 78).then(|| format!("{} ({} chars)", a.get_id(), help.len()))
            })
            .collect();
        assert!(long.is_empty(), "over-long -h summaries: {}", long.join(", "));
    }

    /// "What do I get if I leave this out?" is the question the option list
    /// exists to answer, and for most of these the answer isn't "nothing" —
    /// it's the scene file, or a view, or a preset. Every option that takes a
    /// value either carries a clap default (printed as `[default: x]`) or ends
    /// its summary with a bracket saying where the value comes from instead.
    #[test]
    fn every_value_says_what_it_falls_back_to() {
        let cmd = Args::command();
        let silent: Vec<_> = cmd
            .get_arguments()
            .filter(|a| a.get_num_args().map(|n| n.takes_values()).unwrap_or(false))
            .filter(|a| {
                let documented = a.get_help().is_some_and(|h| h.to_string().trim_end().ends_with(']'));
                a.get_default_values().is_empty() && !documented
            })
            .map(|a| a.get_id().to_string())
            .collect();
        assert!(silent.is_empty(), "no default and no [fallback] note: {}", silent.join(", "));
    }

    /// Short flags are rationed: they go to the handful of options typed over
    /// and over, where the letter is the obvious one. Every letter spent makes
    /// the next one less obvious, so this list is a decision, not a default —
    /// -S/--set pairs with -s/--scene (load a scene / override part of one).
    #[test]
    fn short_flags_are_the_frequent_ones() {
        let mut shorts: Vec<char> =
            Args::command().get_arguments().filter_map(|a| a.get_short()).collect();
        shorts.sort_unstable();
        // -h is clap's own and isn't listed here until the command is built.
        assert_eq!(shorts, vec!['S', 'i', 'p', 'r', 's', 'v']);
    }
}
