//! `--info`: say everything about a scene, in text, without rendering it
//!
//! Written for the agent side of this tool, and by one. Reading a scene
//! previously meant `cat`ting its TOML and doing arithmetic in your head:
//! weights are unnormalized so you can't see what fraction of the chaos game a
//! transform actually gets, `scale` is authored per-axis but contraction is
//! the cube root of the determinant, and whether a map could serve as a zoom
//! symmetry is four separate conditions. All of that is already computed
//! somewhere in the codebase for its own reasons; this prints it.
//!
//! It also reports what the scene *measures* — where the attractor lands and
//! how big it is, from the same CPU walkers `randomize.rs` gates on — which
//! the file cannot tell you at all, and which is the thing you want before
//! choosing a camera distance or a point size.

use glam::Vec3;

use crate::camera::CameraOverride;
use crate::palette::{to_srgb8, Palette};
use crate::renorm::{Renorm, ZoomSpec};
use crate::scene::{ColorMode, Scene};
use crate::view::View;

/// What `--info` was asked about: the scene, plus whatever the command line
/// layered over it.
///
/// A struct rather than a widening argument list, so the next layer — another
/// file, another override — is one field and one block, not a signature change
/// at every call site.
pub struct Subject<'a> {
    pub scene: &'a Scene,
    /// Where the scene came from: a path, or `--random` / `--blank`
    pub source: &'a str,
    /// A loaded `--view`, with the path it was read from
    pub view: Option<(&'a str, &'a View)>,
    /// `--yaw` and friends
    pub camera: CameraOverride,
}

impl<'a> Subject<'a> {
    pub fn new(scene: &'a Scene, source: &'a str) -> Self {
        Self { scene, source, view: None, camera: CameraOverride::default() }
    }

    pub fn with_view(mut self, path: &'a str, view: &'a View) -> Self {
        self.view = Some((path, view));
        self
    }

    pub fn with_camera(mut self, camera: CameraOverride) -> Self {
        self.camera = camera;
        self
    }
}

// ---- how a quantity is written ------------------------------------------
//
// One function per kind of quantity, used everywhere that kind appears, so a
// position always looks like a position and an angle always carries its unit.
// A reader who has learned the shape once — human or 3B model — never has to
// learn it again, and two reports diff cleanly against each other.

/// A position or direction: `(x, y, z)`, three decimals, always three
/// components even when they are zero.
fn point(v: Vec3) -> String {
    format!("({:.3}, {:.3}, {:.3})", v.x, v.y, v.z)
}

/// An angle: radians, because that is what every flag and file uses, with the
/// degrees alongside because that is what people picture. Never one alone.
fn angle(radians: f32) -> String {
    format!("{:>8.4} rad ({:.1}°)", radians, radians.to_degrees())
}

/// A distance or radius in world units
fn length(x: f32) -> String {
    format!("{:>8.3}", x)
}

/// A 0-1 amount, a multiplier, an exponent — anything read as a dial
fn amount(x: f32) -> String {
    format!("{:>8.2}", x)
}

/// A point size: small, so it needs the extra places
fn size(x: f32) -> String {
    format!("{:>8.4}", x)
}

/// A word standing where a number would go — `unset`, `auto`, `pinned`. Right
/// aligned with the numbers so the value column has one edge to scan down,
/// and so "this row has no number" reads as a value rather than a gap.
fn word(s: &str) -> String {
    format!("{:>8}", s)
}

/// A block of `key   value   note` lines.
///
/// Fixed columns, one fact per line, and the **same keys every time** —
/// including ones the file left out, which print as `unset` rather than
/// vanishing. Anything that has read the block once knows where to look in the
/// next one, and two of them diff row for row. The columns are narrow on
/// purpose: a little alignment is worth a few spaces, a padded table is not.
struct Rows<'a> {
    out: &'a mut String,
}

impl Rows<'_> {
    const KEY: usize = 16;
    const VALUE: usize = 13;

    /// A row with nothing to say about where the value would otherwise come
    /// from — the camera framing, mostly, which a view simply *is*.
    fn row(&mut self, key: &str, value: &str) {
        self.out.push_str(&format!("  {:<k$}{}\n", key, value, k = Self::KEY));
    }

    /// A row whose note says what you would have got without this layer:
    /// `scene: 0.0024`, `default: points`. The reason the block exists is that
    /// "what does this file actually change?" is otherwise unanswerable
    /// without opening two files and comparing them by eye.
    fn note(&mut self, key: &str, value: &str, note: &str) {
        self.out.push_str(&format!(
            "  {:<k$}{:<v$}{}\n",
            key,
            value,
            note,
            k = Self::KEY,
            v = Self::VALUE
        ));
    }
}

/// The `view:` block: what a `--view` file sets, what it leaves alone, and
/// what each of its values replaced.
///
/// Every row is always printed. To add a field to a view, add one `r.note`
/// line here in the same order as the struct, and the layout takes care of
/// itself.
fn view_block(out: &mut String, path: &str, v: &View, scene: &Scene) {
    out.push_str(&format!("view: {}\n", path));
    let mut r = Rows { out };
    r.row("of scene", v.scene.as_deref().unwrap_or("unset"));
    r.row("yaw", &angle(v.rotation));
    r.row("pitch", &angle(v.pitch));
    r.row("roll", &angle(v.roll));
    r.row("distance", &length(v.distance));
    r.row("focus", &point(Vec3::from(v.focus)));
    if Vec3::from(v.offset) != Vec3::ZERO {
        // Legacy eye offset. Only a pre-orbit view carries one, and it is
        // already folded into the framing above, so it is a footnote rather
        // than part of the schema.
        r.note("offset", &point(Vec3::from(v.offset)), "legacy; folded into the framing");
    }
    r.note("point_size", &size(v.point_size), &format!("scene: {:.4}", scene.point_size));

    // A view written before haze became one control carries only the raw
    // shader values; the loader recovers an amount from them, so report the
    // number that will actually be used, and say where it came from.
    let (haze, recovered) = match v.haze {
        Some(a) => (a.clamp(0.0, 1.0), false),
        None => (crate::haze::amount_from_brightness(v.haze_transmittance), true),
    };
    r.note(
        "haze",
        &amount(haze),
        &format!(
            "scene: {:.2}{}",
            scene.haze,
            if recovered { "  (recovered from haze_transmittance)" } else { "" }
        ),
    );
    let pinned = v.haze.is_none() || v.haze_band_pinned;
    r.note(
        "haze band",
        &word(if pinned { "pinned" } else { "auto" }),
        &if pinned {
            format!("near {:.2}, far {:.2}", v.haze_near, v.haze_far)
        } else {
            "ranged from the camera distance".to_string()
        },
    );

    r.note(
        "color_falloff",
        &v.color_falloff.map_or(word("unset"), amount),
        &format!("scene: {:.2}", scene.color_falloff),
    );
    r.note(
        "color_contrast",
        &v.color_contrast.map_or(word("unset"), amount),
        &format!("scene: {:.2}", scene.color_contrast),
    );
    r.note("renderer", &word(v.renderer.as_deref().unwrap_or("unset")), "default: points");
    r.note(
        "exposure",
        &v.exposure.map_or(word("unset"), amount),
        "default: 1.00, splat only",
    );
    out.push_str(
        "  a view sets only the rows above; transforms, colours, background,\n  \
         point_count, camera path and zoom always come from the scene\n",
    );
}

/// A gradient as `width` 24-bit ANSI colour blocks.
///
/// Unconditionally colourised, including when stdout is a pipe. `--info` is a
/// diagnostic read by people and by agents, not a data format — and an agent
/// that can see the gradient makes better decisions about a scene than one
/// parsing floats and imagining it. Colours are encoded to sRGB on the way
/// out, because a terminal expects display values, not the linear ones the
/// GPU is handed.
pub fn swatch(palette: &Palette, width: usize) -> String {
    let mut out = String::new();
    for i in 0..width {
        let [r, g, b] = to_srgb8(palette.sample(i as f32 / width as f32));
        out.push_str(&format!("\x1b[48;2;{r};{g};{b}m "));
    }
    out.push_str("\x1b[0m");
    out
}

/// A list of colours as ANSI blocks, four columns each so a handful of
/// transform colours are still legible side by side.
fn color_blocks(colors: &[Vec3]) -> String {
    let mut out = String::new();
    for &c in colors {
        let [r, g, b] = to_srgb8(c);
        out.push_str(&format!("\x1b[48;2;{r};{g};{b}m    "));
    }
    out.push_str("\x1b[0m");
    out
}

/// The same gradient as `n` hex stops — the fallback for anywhere the escape
/// codes above don't render, and the form you can paste into a scene file.
fn hex_ramp(palette: &Palette, n: usize) -> String {
    (0..n)
        .map(|i| {
            let [r, g, b] = to_srgb8(palette.sample(i as f32 / n as f32));
            format!("#{r:02x}{g:02x}{b:02x}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Everything `--info` prints, as one string
pub fn report(subject: &Subject) -> String {
    let scene = subject.scene;
    let source = subject.source;
    // The framing that would actually render: the view's if there is one, the
    // scene's otherwise, with the camera flags over the top. Reported through
    // the same function `--render` frames with, so the two can't drift.
    let effective = crate::offline::effective_camera(
        subject.view.map(|(_, v)| v),
        scene,
        subject.camera,
    );
    let layered = subject.view.is_some() || !subject.camera.is_empty();

    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };

    line(format!("{}  —  {}", scene.name, source));
    if !scene.author.is_empty() && scene.author != "Unknown" {
        line(format!("author: {}", scene.author));
    }
    line(String::new());

    // ---- what the chaos game will do -------------------------------------
    let enabled = vec![true; scene.transforms.len()];
    let total: f32 = scene.transforms.iter().map(|t| t.weight).sum();
    // Rotations are re-derived from the matrix, so a transform authored as
    // (-26, 138, 0) can come back as (154, 42, -180) — the same rotation on
    // the other euler branch. Say so rather than have someone diff it against
    // the file and conclude the loader is wrong.
    line(format!(
        "{} transforms (rotations re-derived from the matrix; euler branch may differ from the file)",
        scene.transforms.len()
    ));
    for (i, t) in scene.transforms.iter().enumerate() {
        let name = scene
            .transform_names
            .get(i)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("#{}", i));
        let (scale, rot, trans) = t.matrix.to_scale_rotation_translation();
        let (rx, ry, rz) = rot.to_euler(glam::EulerRot::XYZ);
        let share = if total > 0.0 { t.weight / total * 100.0 } else { 0.0 };
        line(format!(
            "  [{}] {:<14} {:>5.1}% of the walk   contraction {:.3}",
            i,
            name,
            share,
            t.contraction()
        ));
        line(format!(
            "       scale {}  rot ({:.0}, {:.0}, {:.0})°  translate {}",
            fmt_scale(scale),
            rx.to_degrees(),
            ry.to_degrees(),
            rz.to_degrees(),
            point(trans),
        ));
        let vars = t.variation_summary();
        if vars != "linear" {
            line(format!("       {}", vars));
        }
    }
    line(String::new());

    // ---- what it actually looks like -------------------------------------
    // Nothing above this line needed to run the fractal; this does, and it's
    // the part a TOML file can't answer.
    match crate::trace::measure(&scene.transforms, &enabled) {
        Some(s) => {
            line(format!(
                "measured: centre {}, radius {:.3} (95th pct), spread {}, occupancy {:.1}%",
                point(s.center),
                s.radius,
                point(s.spread),
                s.occupancy * 100.0
            ));
            // The two numbers most often wrong in a hand-authored scene, and
            // the reason they're wrong is that both depend on this measurement
            // rather than on anything visible in the file.
            let framed = (s.radius * 2.4).clamp(0.8, 12.0);
            line(format!(
                "  suggests: camera distance ~{:.2} (fills the frame), \
                 point_size ≤ {:.4} (stays on the crisp 1px path at 1080p)",
                framed,
                1.5 * framed / 1080.0
            ));
            // Against the framing that would render, not the scene's authored
            // one — with a --view loaded those are different numbers, and the
            // useful one is what you'd get.
            let d = effective.distance;
            if (d / framed).max(framed / d) > 2.0 {
                line(format!("  NOTE: this frames at distance {:.2}, {:.1}x that", d, d / framed));
            }
        }
        None => line(
            "measured: the chaos game does not converge (all weights zero, or it diverges)"
                .to_string(),
        ),
    }
    line(String::new());

    // ---- render properties ------------------------------------------------
    line(format!(
        "point_size {}   point_count {}   haze {:.2}   background [{:.3}, {:.3}, {:.3}]",
        scene.point_size, scene.point_count, scene.haze,
        scene.background.x, scene.background.y, scene.background.z
    ));
    line(format!(
        "color: speed {:.2}, falloff {:.2}, contrast {:.2}",
        scene.color_speed, scene.color_falloff, scene.color_contrast
    ));

    // ---- colour source ----------------------------------------------------
    // The gradient is the one render property a file genuinely cannot convey:
    // `[palette] name = "ember"` says nothing about what ember looks like, and
    // twenty-four floats say less. So draw it.
    let resolved = scene.effective_palette();
    line(format!(
        "colormap [{}]: {}",
        scene.color_mode.name(),
        match scene.color_mode {
            ColorMode::Palette => resolved.describe(),
            ColorMode::Transforms => format!(
                "{} transform colours, evenly spread (adding a transform moves them all)",
                scene.colors.len()
            ),
            ColorMode::Mix => format!(
                "{} transform colours mixed through the walk as RGB — no colormap, \
                 so transform *combinations* are distinguishable and color_contrast \
                 does not apply",
                scene.colors.len()
            ),
        }
    ));
    if scene.color_mode == ColorMode::Mix {
        // The ring would be a lie here: mix mode never indexes it. Show the
        // colours that actually get blended, one block each.
        line(format!("  {}", color_blocks(&scene.colors)));
        line(format!(
            "  {}",
            scene
                .colors
                .iter()
                .map(|&c| {
                    let [r, g, b] = to_srgb8(c);
                    format!("#{r:02x}{g:02x}{b:02x}")
                })
                .collect::<Vec<_>>()
                .join(" ")
        ));
    } else {
        line(format!("  {}", swatch(&resolved, 48)));
        line(format!("  {}", hex_ramp(&resolved, 8)));
    }
    let (mean, swing) = resolved.luminance_profile();
    line(format!(
        "  luminance: mean {:.2}, swing {:.2}{}",
        mean,
        swing,
        // The palette is the only shading this renderer has, so a flat one is
        // worth saying out loud rather than leaving to be discovered.
        if swing < 0.15 { "  (flat — renders without shading)" } else { "" }
    ));
    if scene.color_mode == ColorMode::Transforms && scene.palette.is_some() {
        line("  (this scene also carries a [palette]; --color-mode palette renders it)".to_string());
    }
    if scene.color_contrast > 1.5 && scene.color_mode != ColorMode::Mix {
        line(format!(
            "  note: color_contrast {:.2} stretches the index, so only part of \
             this gradient is reached",
            scene.color_contrast
        ));
    }
    // ---- the view layered over it -----------------------------------------
    // Before the camera line rather than after it: this is where the framing
    // below comes from, and a reader meets the source before the result.
    if let Some((path, v)) = subject.view {
        let mut block = String::new();
        view_block(&mut block, path, v, scene);
        line(String::new());
        line(block.trim_end().to_string());
        line(String::new());
    }

    // The framing that would render, and — when something was layered over
    // the scene — the scene's own underneath it, so nothing is lost by asking
    // about a view.
    let chart = effective.chart();
    line(format!(
        "camera: yaw {:.4} pitch {:.4} distance {:.3} focus {}{}   from {}",
        chart.yaw.radians(),
        chart.pitch.radians(),
        effective.distance,
        point(effective.focus),
        // A quat round trip leaves roll at ±1e-8, which prints as an
        // eye-catching "-0.0000". Below what four places can show, it is level.
        match chart.roll.radians() {
            r if r.abs() < 5e-5 => String::new(),
            r => format!(" roll {:.4}", r),
        },
        match (subject.view.is_some(), subject.camera.is_empty()) {
            (true, false) => "--view, then the camera flags",
            (true, true) => "--view",
            (false, false) => "the scene, then the camera flags",
            (false, true) => "the scene",
        }
    ));
    if scene.zoom.is_some() {
        // Under infinite zoom a framing is only defined up to a zoom period,
        // and the renderer canonicalises it — so `--distance 6` can be
        // reported as 2.160 with the yaw turned by the twist. Say why, or it
        // reads as the flag having been ignored.
        line("  (infinite zoom: reported in the canonical period, as it renders)".to_string());
    }
    if layered {
        let scene_chart = scene.camera_orientation.yaw_pitch_roll();
        line(format!(
            "  the scene's own: yaw {:.4} pitch {:.4} distance {:.3} focus {}",
            scene_chart.yaw.radians(),
            scene_chart.pitch.radians(),
            scene.camera_distance,
            point(scene.camera_focus),
        ));
    }
    match &scene.camera_path {
        Some(p) if p.playable() => line(format!(
            "path: {} keypoint{}, {}, {:.1}s",
            p.keys.len(),
            if p.keys.len() == 1 { "" } else { "s" },
            match p.loops {
                crate::path::Loop::Zoom(z) => format!(
                    "zoom loop: {} period{} down per loop, closing on an identical frame",
                    z.periods,
                    if z.periods == 1 { "" } else { "s" }
                ),
                crate::path::Loop::PingPong =>
                    "ping-pong loop: out to the last key and back again".to_string(),
                l => l.kind().label().to_string(),
            },
            p.duration()
        )),
        _ => line("path: none authored (the default full-turn turntable applies)".to_string()),
    }
    line(String::new());

    // ---- infinite zoom ----------------------------------------------------
    // Eligibility is four conditions spread over two files; checking it by
    // eye is exactly the sort of thing this command exists to stop.
    line("infinite zoom:".to_string());
    match &scene.zoom {
        Some(spec) => match Renorm::build(spec, &scene.transforms, scene.camera_distance) {
            Ok(r) => line(format!(
                "  ON — {}",
                r.summary(scene.transform_names.get(r.map).cloned().flatten().as_deref())
            )),
            Err(e) => line(format!("  BROKEN — {}", e)),
        },
        None => line("  off; eligible maps:".to_string()),
    }
    if scene.zoom.is_none() {
        let mut any = false;
        for i in 0..scene.transforms.len() {
            let name = scene
                .transform_names
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_else(|| i.to_string());
            match Renorm::build(&ZoomSpec { map: i, ..Default::default() }, &scene.transforms, scene.camera_distance)
            {
                Ok(r) => {
                    any = true;
                    line(format!(
                        "    --zoom {:<12} {:.2} octaves/period, {:.0}° twist, fixed point {}{}",
                        name,
                        r.log_scale / std::f32::consts::LN_2,
                        r.twist_degrees(),
                        point(r.fixed_point),
                        match (r.defect > 0.02, r.band_covers_the_view()) {
                            (true, _) => "  [not a similarity — seam]",
                            (_, false) => "  [band too short — see renorm::MIN_RADIUS]",
                            _ => "",
                        },
                    ));
                }
                Err(e) => line(format!("    [{}] {:<12} no: {}", i, name, strip_prefix(&e))),
            }
        }
        if !any {
            line("    (none — infinite zoom needs a pure affine map that contracts on all three axes)".to_string());
        }
    }

    out
}

/// `zoom map 3 uses variations (...)` reads badly in a per-map list that has
/// already said which map it's talking about
fn strip_prefix(e: &str) -> String {
    match e.split_once("; ") {
        Some((head, _)) => head
            .split_once(" uses ")
            .map(|(_, rest)| format!("uses {}", rest))
            .unwrap_or_else(|| head.to_string()),
        None => e.to_string(),
    }
}

fn fmt_scale(s: Vec3) -> String {
    if (s.x - s.y).abs() < 1e-4 && (s.x - s.z).abs() < 1e-4 {
        format!("{:.3}", s.x)
    } else {
        format!("[{:.3}, {:.3}, {:.3}]", s.x, s.y, s.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A view with nothing optional set: the minimum a person can write by
    /// hand, and the case where "which rows are missing?" matters most.
    fn bare_view() -> View {
        View {
            scene: None,
            rotation: 1.25,
            pitch: 0.35,
            roll: 0.0,
            distance: 4.0,
            focus: [0.1, 0.2, 0.3],
            offset: [0.0; 3],
            point_size: 0.002,
            haze_near: 3.0,
            haze_far: 4.5,
            haze_transmittance: 1.0,
            haze_saturation: 1.0,
            haze: Some(0.0),
            haze_band_pinned: false,
            color_falloff: None,
            color_contrast: None,
            renderer: None,
            exposure: None,
        }
    }

    #[test]
    fn reports_every_transform_and_the_zoom_verdict() {
        let scene = Scene::blank();
        let text = report(&Subject::new(&scene, "blank"));
        assert!(text.contains("2 transforms"));
        // blank()'s two maps are plain half-scales: both eligible
        assert!(text.contains("--zoom 0"), "{}", text);
        assert!(text.contains("--zoom 1"), "{}", text);
        // and the measurement ran
        assert!(text.contains("measured: centre"), "{}", text);
    }

    #[test]
    fn weights_are_reported_as_shares_of_the_walk() {
        let mut scene = Scene::blank();
        scene.transforms[0].weight = 3.0;
        scene.transforms[1].weight = 1.0;
        let text = report(&Subject::new(&scene, "test"));
        assert!(text.contains("75.0%"), "{}", text);
        assert!(text.contains("25.0%"), "{}", text);
    }

    /// No view, no flags: no view block and no second camera line. `--info` on
    /// a plain scene must not grow a section that says nothing.
    #[test]
    fn a_plain_scene_reports_no_view() {
        let scene = Scene::blank();
        let text = report(&Subject::new(&scene, "blank"));
        assert!(!text.contains("view:"), "{}", text);
        assert!(!text.contains("the scene's own:"), "{}", text);
        assert!(text.contains("from the scene"), "{}", text);
    }

    /// The block's whole point: every key, every time, including the ones this
    /// view didn't set. A row that vanishes when unset is a row you can't
    /// learn to look for.
    #[test]
    fn the_view_block_has_a_fixed_shape() {
        let scene = Scene::blank();
        let v = bare_view();
        let text = report(&Subject::new(&scene, "blank").with_view("views/v.toml", &v));
        for key in [
            "of scene", "yaw", "pitch", "roll", "distance", "focus", "point_size",
            "haze", "haze band", "color_falloff", "color_contrast", "renderer", "exposure",
        ] {
            assert!(text.contains(key), "missing row {:?} in:\n{}", key, text);
        }
        // Unset optionals say so rather than disappearing
        assert_eq!(text.matches("unset").count(), 5, "{}", text);
        // and each override says what it replaced
        assert!(text.contains("scene: 0.0120"), "{}", text);
        assert!(text.contains("default: points"), "{}", text);
    }

    /// A view is the framing that would render, so the camera line reports it
    /// — with the scene's own kept underneath rather than overwritten.
    #[test]
    fn the_camera_line_follows_the_view() {
        let scene = Scene::blank();
        let v = bare_view();
        let text = report(&Subject::new(&scene, "blank").with_view("views/v.toml", &v));
        assert!(text.contains("yaw 1.2500"), "{}", text);
        assert!(text.contains("focus (0.100, 0.200, 0.300)"), "{}", text);
        assert!(text.contains("from --view"), "{}", text);
        assert!(text.contains("the scene's own:"), "{}", text);
    }

    /// Flags are the most specific thing anyone said, so they win over a view
    /// here exactly as they do at render time.
    #[test]
    fn camera_flags_win_over_the_view() {
        let scene = Scene::blank();
        let v = bare_view();
        let text = report(
            &Subject::new(&scene, "blank")
                .with_view("views/v.toml", &v)
                .with_camera(CameraOverride { distance: Some(9.5), ..Default::default() }),
        );
        assert!(text.contains("distance 9.500"), "{}", text);
        assert!(text.contains("--view, then the camera flags"), "{}", text);
    }

    /// Angles carry both units and positions always have three components:
    /// the two shapes everything else in the report is written against.
    #[test]
    fn quantities_are_written_one_way() {
        assert_eq!(point(Vec3::new(1.0, -0.5, 0.0)), "(1.000, -0.500, 0.000)");
        assert!(angle(std::f32::consts::PI).contains("rad"));
        assert!(angle(std::f32::consts::PI).contains("180.0°"));
    }
}
