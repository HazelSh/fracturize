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
//!
//! # The shape of the page
//!
//! The report is a [`Report`]: a list of [`Section`]s of [`Row`]s, rendered to
//! text at the end. Nothing appends to a string in the middle of computing
//! something, which is what makes the `notes` block collectable (a diagnostic
//! raised while measuring can be printed second) and what makes a second
//! renderer — JSON, for a small model with an awkward tokenizer — an
//! afternoon rather than a rewrite.
//!
//! Seven rules govern the layout, and they exist because the same page has to
//! serve a model reading piped text and a person skimming a terminal:
//!
//! 1. A section keyword in a fixed [`GUTTER`]-column left gutter, with
//!    continuation lines indented into it. A hanging-indent definition list is
//!    the most scannable layout in text and the easiest to parse: `^(\w+)\s+`.
//! 2. Importance by position. What is this (1-2 lines), is it sound and what
//!    do I do next (the `notes` and `shape` blocks), then detail.
//! 3. Short blocks label in the header; long blocks label in the row. The eye
//!    tracks a column header twelve rows up for free and a model does not, so
//!    the fifteen-row transform list self-labels and the six-row `render`
//!    block does not.
//! 4. Decimal points line up and the sign gets its own column — which is what
//!    the quantity writers below are for.
//! 5. [`WIDTH`] columns, hard. A row that wraps is worse than no table at all,
//!    because wrapping destroys exactly the alignment the table provided.
//! 6. Blank lines are one token each and the strongest segmentation signal
//!    there is. One per section.
//! 7. Every row prints every time — a row that vanishes when unset is a row
//!    you cannot learn to look for — and prose footnotes go at the *end* of
//!    their section rather than welded to its header.
//!
//! ANSI colour appears if and only if `--color` was passed, and adds to the
//! page rather than replacing part of it: the coloured report is a strict
//! superset of the default one. An agent reading this through a pipe cannot
//! see escape codes — it receives them as literal bytes — and on a typical
//! scene the swatch alone was a third of the report.

use glam::Vec3;

use crate::camera::CameraOverride;
use crate::palette::{luminance, to_srgb8, Palette};
use crate::renorm::{Renorm, ZoomSpec, MIN_RADIUS};
use crate::scene::{ColorMode, Scene, TransformSpec};
use crate::set::Applied;
use crate::view::View;

/// The hard right margin. An 80-column terminal is still the human floor, and
/// a wrapped row costs a model the alignment it was reading.
pub const WIDTH: usize = 78;

/// Width of the section-keyword gutter, including its trailing space
pub const GUTTER: usize = 9;

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
    /// `--set` overrides that were applied, and what each displaced
    pub set: &'a [Applied],
    /// The seed a `--random` scene was rolled from
    pub seed: Option<u64>,
    /// `--color`: the one switch that decides whether any escape code is
    /// emitted anywhere in this report
    pub color: bool,
}

impl<'a> Subject<'a> {
    pub fn new(scene: &'a Scene, source: &'a str) -> Self {
        Self {
            scene,
            source,
            view: None,
            camera: CameraOverride::default(),
            set: &[],
            seed: None,
            color: false,
        }
    }

    pub fn with_view(mut self, path: &'a str, view: &'a View) -> Self {
        self.view = Some((path, view));
        self
    }

    pub fn with_camera(mut self, camera: CameraOverride) -> Self {
        self.camera = camera;
        self
    }

    pub fn with_set(mut self, set: &'a [Applied]) -> Self {
        self.set = set;
        self
    }

    pub fn with_seed(mut self, seed: Option<u64>) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_color(mut self, color: bool) -> Self {
        self.color = color;
        self
    }
}

// ---- the report as a value ----------------------------------------------

/// A `label  value` pair. The label is padded to its column's width across the
/// whole section, so a column of values has one edge to scan down.
struct Cell {
    label: String,
    value: String,
    /// Whether this cell pays for its column's label width. A value does; a
    /// remark trailing the value beside it does not, and would otherwise be
    /// pushed a dozen columns right by a long label two rows up.
    columned: bool,
}

impl Cell {
    fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: value.into(), columned: true }
    }

    /// A value in a labelled column that has nothing to say this row. It still
    /// takes the label column's width, so the value stays under its neighbours.
    fn bare(value: impl Into<String>) -> Self {
        Self { label: String::new(), value: value.into(), columned: true }
    }

    /// Prose that qualifies the cell before it — a unit, a range, a caveat.
    /// Starts where it starts; nothing lines up under it.
    fn note(value: impl Into<String>) -> Self {
        Self { label: String::new(), value: value.into(), columned: false }
    }
}

enum Row {
    /// Columnised cells, indented into the gutter
    Cells(Vec<Cell>),
    /// Free text, indented into the gutter: headers, footnotes, and anything
    /// whose shape would distort the section's columns if it joined them
    Text(String),
}

struct Section {
    key: &'static str,
    rows: Vec<Row>,
}

impl Section {
    fn new(key: &'static str) -> Self {
        Self { key, rows: Vec::new() }
    }

    fn cells(&mut self, cells: Vec<Cell>) -> &mut Self {
        self.rows.push(Row::Cells(cells));
        self
    }

    fn pair(&mut self, label: &str, value: impl Into<String>) -> &mut Self {
        self.cells(vec![Cell::new(label, value)])
    }

    fn text(&mut self, text: impl Into<String>) -> &mut Self {
        self.rows.push(Row::Text(text.into()));
        self
    }

    /// Label-column widths, taken across every columnised row in the section.
    /// This is rule 4: one edge per column, computed rather than guessed.
    fn widths(&self) -> Vec<usize> {
        let mut w: Vec<usize> = Vec::new();
        for row in &self.rows {
            if let Row::Cells(cells) = row {
                for (c, cell) in cells.iter().enumerate() {
                    if w.len() <= c {
                        w.push(0);
                    }
                    if cell.columned {
                        w[c] = w[c].max(cell.label.chars().count());
                    }
                }
            }
        }
        w
    }

    fn render(&self, out: &mut String) {
        let widths = self.widths();
        for (i, row) in self.rows.iter().enumerate() {
            if i == 0 {
                out.push_str(&format!("{:<w$}", self.key, w = GUTTER));
            } else {
                out.push_str(&" ".repeat(GUTTER));
            }
            match row {
                Row::Text(text) => out.push_str(text.trim_end()),
                Row::Cells(cells) => {
                    let mut line = String::new();
                    for (c, cell) in cells.iter().enumerate() {
                        if c > 0 {
                            line.push_str("   ");
                        }
                        if cell.columned && widths[c] > 0 {
                            line.push_str(&format!("{:<w$}  ", cell.label, w = widths[c]));
                        }
                        line.push_str(&cell.value);
                    }
                    out.push_str(line.trim_end());
                }
            }
            out.push('\n');
        }
    }
}

/// Everything `--info` knows, before it is text
struct Report {
    sections: Vec<Section>,
}

impl Report {
    /// Rule 6: exactly one blank line between sections, and none inside one.
    fn to_text(&self) -> String {
        let mut out = String::new();
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            section.render(&mut out);
        }
        out
    }
}

/// A diagnostic, tagged with the section it is about so a reader knows where
/// to look. Collected as the report is built and printed second, where an
/// agent deciding whether to spend a render will actually see it.
struct Note {
    tag: &'static str,
    text: String,
}

/// Somewhere to put notes raised while computing a section that is printed
/// later than they are.
#[derive(Default)]
struct Notes(Vec<Note>);

impl Notes {
    fn add(&mut self, tag: &'static str, text: impl Into<String>) {
        self.0.push(Note { tag, text: text.into() });
    }
}

// ---- how a quantity is written ------------------------------------------
//
// One function per kind of quantity, used everywhere that kind appears, so a
// position always looks like a position and an angle always carries its unit.
// A reader who has learned the shape once — human or 3B model — never has to
// learn it again, and two reports diff cleanly against each other.
//
// Every one of them is fixed width and right aligned, which is what makes the
// decimal points line up without any column knowing what is in it. The sign
// gets its own column: without that, one negative value shifts its whole row
// a character over and the eye loses the edge it was following.

/// A position or direction: `(x, y, z)`, three decimals, always three
/// components even when they are zero, always 24 characters wide.
fn point(v: Vec3) -> String {
    format!("({:>6.3}, {:>6.3}, {:>6.3})", v.x, v.y, v.z)
}

/// An angle: radians, because that is what every flag and file uses, with the
/// degrees alongside because that is what people picture. Never one alone.
fn angle(radians: f32) -> String {
    let r = if radians.abs() < 5e-5 { 0.0 } else { radians };
    format!("{:>8.4} rad ({:>6.1}°)", r, degrees(r))
}

/// Degrees for display, with the near-zero snapped flat.
///
/// A quat round trip leaves an angle at ±1e-8, which prints as an eye-catching
/// `-0.0` and reads as something the loader got wrong. Below what one decimal
/// can show, it is zero.
fn degrees(radians: f32) -> f32 {
    let d = radians.to_degrees();
    if d.abs() < 5e-2 {
        0.0
    } else {
        d
    }
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

/// A whole number of things
fn count(n: usize) -> String {
    format!("{:>8}", n)
}

/// A word standing where a number would go — `unset`, `auto`, `pinned`. Right
/// aligned with the numbers so the value column has one edge to scan down,
/// and so "this row has no number" reads as a value rather than a gap.
fn word(s: &str) -> String {
    format!("{:>8}", s)
}

/// A share of the walk
fn percent(x: f32) -> String {
    format!("{:>6.1}%", x)
}

/// A point count at a glance: `8.0M`, `500k`
fn magnitude(n: usize) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{}k", n / 1_000),
        n => n.to_string(),
    }
}

/// Two things on one line, the second pushed to the right margin. Used once,
/// for the scene's name and where it was read from — the one row where the
/// two halves are equally the answer to "what am I looking at".
fn spread(left: &str, right: &str) -> String {
    let used = left.chars().count() + right.chars().count();
    let room = WIDTH - GUTTER;
    match room.checked_sub(used) {
        Some(gap) if gap >= 2 => format!("{}{}{}", left, " ".repeat(gap), right),
        _ => format!("{}  {}", left, right),
    }
}

/// Break `text` into lines that fit the margin: `first` in front of the first
/// line, `hang` spaces in front of the rest.
///
/// Rule 5 is not a target, it is a constraint, and prose is where it actually
/// bites — an author line reading "by original by Claude Opus 4.5 (modified
/// with Gemini)" is 111 columns before anything else joins it. Wrapping with a
/// hanging indent keeps the continuation visibly subordinate to the row it
/// belongs to, which is what stops a wrapped line reading as a new fact.
fn wrap(first: &str, hang: usize, text: &str) -> Vec<String> {
    let room = WIDTH - GUTTER;
    let mut out = Vec::new();
    let mut line = first.to_string();
    let mut empty = true;
    for word in text.split_whitespace() {
        if !empty && line.chars().count() + 1 + word.chars().count() > room {
            out.push(std::mem::replace(&mut line, " ".repeat(hang)));
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    out.push(line);
    out
}

/// A colour as the hex you could paste into a scene file.
fn hex(c: Vec3) -> String {
    let [r, g, b] = to_srgb8(c);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The same hex, wearing the colour it names when `--color` is on.
///
/// Additive, not substitutive: the text is unchanged and the escape codes
/// wrap it, so the coloured report is the plain one plus paint. The colour
/// goes on the background with a contrasting foreground rather than tinting
/// the glyphs, because half a gradient is too dark to read as text on a dark
/// terminal and the near-invisible half is usually the interesting one.
fn hex_tinted(c: Vec3, color: bool) -> String {
    let text = hex(c);
    if !color {
        return text;
    }
    let [r, g, b] = to_srgb8(c);
    let ink = if luminance(c) > 0.2 { 0 } else { 255 };
    format!("\x1b[48;2;{r};{g};{b}m\x1b[38;2;{ink};{ink};{ink}m{text}\x1b[0m")
}

/// A gradient as `width` 24-bit ANSI colour blocks.
///
/// Only ever reached with `--color`. Colours are encoded to sRGB on the way
/// out, because a terminal expects display values, not the linear ones the GPU
/// is handed.
pub fn swatch(palette: &Palette, width: usize) -> String {
    let mut out = String::new();
    for i in 0..width {
        let [r, g, b] = to_srgb8(palette.sample(i as f32 / width as f32));
        out.push_str(&format!("\x1b[48;2;{r};{g};{b}m "));
    }
    out.push_str("\x1b[0m");
    out
}

/// The gradient as `n` hex stops — the channel that works everywhere, and the
/// form you can paste into a scene file. Twelve rather than eight: this is the
/// only colour most readers of this report will ever get, so it should carry
/// the resolution.
const RAMP_STOPS: usize = 12;

/// A handful of separate colours as ANSI blocks, wide enough to read side by
/// side. Mix mode's colours are a set, not a gradient, so a continuous swatch
/// would be a lie about how they are used.
fn color_swatch(colors: &[Vec3]) -> String {
    let mut out = String::new();
    for &c in colors {
        let [r, g, b] = to_srgb8(c);
        out.push_str(&format!("\x1b[48;2;{r};{g};{b}m       "));
    }
    out.push_str("\x1b[0m");
    out
}

/// The gradient as `n` hex stops on one line, for `--palettes`, where the
/// listing is one palette per row and the stops are the row.
pub fn hex_ramp(palette: &Palette, n: usize) -> String {
    (0..n)
        .map(|i| hex(palette.sample(i as f32 / n as f32)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Hex stops laid out `per_line` at a time, so a wide ramp never wraps.
fn ramp_rows(stops: &[Vec3], color: bool, indent: &str, per_line: usize) -> Vec<String> {
    stops
        .chunks(per_line)
        .map(|chunk| {
            let cells: Vec<String> = chunk.iter().map(|&c| hex_tinted(c, color)).collect();
            format!("{}{}", indent, cells.join(" "))
        })
        .collect()
}

/// The affine part's contraction: the *signed* cube root of the determinant,
/// before the clamp [`TransformSpec::contraction`] applies for the colour
/// system's benefit.
///
/// The clamped value is right for resolving colour speeds and wrong for a
/// report: it prints an expanding map as `0.950` and a mirrored one exactly
/// like a plain one, which throws away the two facts most worth knowing about
/// a map that is misbehaving. Signed rather than flagged because that is what
/// the number *is* — a map with a negative determinant turns space inside out,
/// and the cube root of a negative determinant is negative. It costs no column
/// and there is nothing to look up.
fn contraction_of(t: &TransformSpec) -> f32 {
    let det = t.matrix.determinant();
    det.abs().powf(1.0 / 3.0) * det.signum()
}

/// Everything `--info` prints, as one string
pub fn report(subject: &Subject) -> String {
    build(subject).to_text()
}

fn build(subject: &Subject) -> Report {
    let scene = subject.scene;
    // The framing that would actually render: the view's if there is one, the
    // scene's otherwise, with the camera flags over the top. Reported through
    // the same function `--render` frames with, so the two can't drift.
    let effective = crate::offline::effective_camera(
        subject.view.map(|(_, v)| v),
        scene,
        subject.camera,
    );
    let layered = subject.view.is_some() || !subject.camera.is_empty();
    let resolved = scene.effective_palette();
    let mut notes = Notes::default();

    // Sections are built in printed order except `notes`, which is spliced in
    // second once everything that could raise one has run.
    let mut sections = vec![scene_section(subject, &notes)];

    if !subject.set.is_empty() {
        sections.push(set_section(subject));
    }
    if let Some((path, v)) = subject.view {
        sections.push(view_section(path, v, scene, &mut notes));
    }
    sections.push(shape_section(subject, &effective, &mut notes));
    sections.push(maps_section(subject, &resolved, &mut notes));
    sections.push(render_section(scene));
    sections.push(colour_section(subject, &resolved, &mut notes));
    sections.push(camera_section(subject, &effective, layered, &mut notes));
    sections.push(path_section(scene));
    sections.push(zoom_section(scene, &mut notes));

    sections.insert(1, notes_section(&notes));
    Report { sections }
}

/// What am I looking at. Two lines, because that is how long a reader who is
/// going to stop reading will keep going.
fn scene_section(subject: &Subject, _notes: &Notes) -> Section {
    let scene = subject.scene;
    let mut s = Section::new("scene");
    s.text(spread(&scene.name, subject.source));

    let mut facts = Vec::new();
    if !scene.author.is_empty() && scene.author != "Unknown" {
        facts.push(format!("by {}", scene.author));
    }
    facts.push(format!("{} maps", scene.transforms.len()));
    facts.push(format!("{} colour", scene.color_mode.name()));
    facts.push(match &scene.zoom {
        Some(_) => "zoom on".to_string(),
        None => "no zoom".to_string(),
    });
    facts.push(format!("{} points", magnitude(scene.point_count)));
    if scene.camera_path.as_ref().is_some_and(|p| p.playable()) {
        facts.push("camera path".to_string());
    }
    for line in wrap("", 0, &facts.join(", ")) {
        s.text(line);
    }

    // A rolled scene exists nowhere on disk, so the seed is the only way back
    // to it. It goes here rather than only on stderr, where a report captured
    // to a file loses it.
    if let Some(seed) = subject.seed {
        s.text(format!("reproduce with  --random --seed {}", seed));
    }
    s
}

/// Is it sound, and what should I do about it. One block, second, with a count
/// on the first line so "nothing is wrong" is a single cheap signal.
fn notes_section(notes: &Notes) -> Section {
    let mut s = Section::new("notes");
    if notes.0.is_empty() {
        s.text("none");
        return s;
    }
    s.text(notes.0.len().to_string());
    let tw = notes.0.iter().map(|n| n.tag.len()).max().unwrap_or(4) + 2;
    for n in &notes.0 {
        for line in wrap(&format!("{:<tw$}", n.tag), tw, &n.text) {
            s.text(line);
        }
    }
    s
}

/// What `--set` moved, and what it moved it from.
///
/// The same question the `view` block answers about a view file. Without it a
/// report shows the result of an override with no trace that the command line
/// was involved, which makes "did my edit land?" unanswerable from the one
/// command that exists to answer it.
fn set_section(subject: &Subject) -> Section {
    let mut s = Section::new("set");
    s.text(format!(
        "{} override{} applied to {}",
        subject.set.len(),
        if subject.set.len() == 1 { "" } else { "s" },
        subject.source
    ));
    let vw = subject.set.iter().map(|a| a.to.chars().count()).max().unwrap_or(0);
    for a in subject.set {
        s.cells(vec![
            Cell::new(&a.path, format!("{:>vw$}", a.to)),
            Cell::new("was", a.from.as_deref().unwrap_or("(unset)")),
        ]);
    }
    s
}

/// The `view` block: what a `--view` file sets, what it leaves alone, and what
/// each of its values replaced.
///
/// Every row is always printed. To add a field to a view, add one row here in
/// the same order as the struct, and the layout takes care of itself.
fn view_section(path: &str, v: &View, scene: &Scene, notes: &mut Notes) -> Section {
    let mut s = Section::new("view");
    s.text(path);
    s.cells(vec![Cell::new("of scene", v.scene.as_deref().unwrap_or("unset"))]);
    s.pair("yaw", angle(v.rotation));
    s.pair("pitch", angle(v.pitch));
    s.pair("roll", angle(v.roll));
    s.pair("distance", length(v.distance));
    s.pair("focus", point(Vec3::from(v.focus)));
    if Vec3::from(v.offset) != Vec3::ZERO {
        // Legacy eye offset. Only a pre-orbit view carries one, and it is
        // already folded into the framing above.
        s.cells(vec![
            Cell::new("offset", point(Vec3::from(v.offset))),
            Cell::bare("legacy; folded into the framing"),
        ]);
    }
    s.cells(vec![
        Cell::new("point_size", size(v.point_size)),
        Cell::new("scene", size(scene.point_size)),
    ]);

    // A view written before haze became one control carries only the raw
    // shader values; the loader recovers an amount from them, so report the
    // number that will actually be used, and say where it came from.
    let (haze, recovered) = match v.haze {
        Some(a) => (a.clamp(0.0, 1.0), false),
        None => (crate::haze::amount_from_brightness(v.haze_transmittance), true),
    };
    s.cells(vec![
        Cell::new("haze", amount(haze)),
        Cell::new("scene", amount(scene.haze)),
    ]);
    let pinned = v.haze.is_none() || v.haze_band_pinned;
    s.cells(vec![
        Cell::new("haze band", word(if pinned { "pinned" } else { "auto" })),
        Cell::note(if pinned {
            format!("near {:.2}, far {:.2}", v.haze_near, v.haze_far)
        } else {
            "ranged from the camera distance".to_string()
        }),
    ]);
    s.cells(vec![
        Cell::new("color_falloff", v.color_falloff.map_or(word("unset"), amount)),
        Cell::new("scene", amount(scene.color_falloff)),
    ]);
    s.cells(vec![
        Cell::new("color_contrast", v.color_contrast.map_or(word("unset"), amount)),
        Cell::new("scene", amount(scene.color_contrast)),
    ]);
    s.cells(vec![
        Cell::new("renderer", word(v.renderer.as_deref().unwrap_or("unset"))),
        Cell::new("default", "points"),
    ]);
    s.cells(vec![
        Cell::new("exposure", v.exposure.map_or(word("unset"), amount)),
        Cell::new("default", "1.00, splat only"),
    ]);
    if recovered {
        s.text("haze was recovered from this view's haze_transmittance: it predates");
        s.text("haze becoming one control, and carries only the shader values");
    }
    s.text("a view sets only the rows above; transforms, colours, background,");
    s.text("point_count, camera path and zoom always come from the scene");

    // A view is a framing measured against one attractor. Loading it over a
    // different scene silently produces a camera pointed at nothing in
    // particular, and nothing else in the report would say so.
    if let Some(named) = v.scene.as_deref() {
        let same = std::path::Path::new(named).file_stem()
            == std::path::Path::new(&scene.name.to_lowercase().replace(' ', "-")).file_stem();
        if !same {
            notes.add("view", format!("this view was framed against {}", named));
        }
    }
    s
}

/// Where the attractor actually lands, and the two numbers most often wrong in
/// a hand-authored scene — as commands rather than as facts, because a
/// suggestion you have to reassemble into a flag is a suggestion half-given.
fn shape_section(subject: &Subject, effective: &crate::camera::OrbitCamera, notes: &mut Notes) -> Section {
    let scene = subject.scene;
    let mut s = Section::new("shape");
    let enabled = vec![true; scene.transforms.len()];

    let measured = crate::trace::measure(&scene.transforms, &enabled);
    let Some(m) = measured else {
        s.text("the chaos game does not converge");
        s.text("every weight is zero, or the walk runs away");
        notes.add("shape", "the chaos game does not converge — nothing to render");
        // Naming the map that did it is the whole difference between a report
        // that says something is wrong and one you can act on.
        let expanding: Vec<String> = scene
            .transforms
            .iter()
            .enumerate()
            .filter(|(_, t)| contraction_of(t).abs() >= 1.0)
            .map(|(i, t)| {
                format!("[{}] {} ({:.3})", i, name_of(scene, i), contraction_of(t).abs())
            })
            .collect();
        if !expanding.is_empty() {
            s.text(format!("maps that expand:  {}", expanding.join(", ")));
        }
        return s;
    };

    s.pair("centre", point(m.center));
    s.pair("spread", point(m.spread));
    s.cells(vec![
        Cell::new("radius", length(m.radius)),
        Cell::new("occupancy", format!("{:>7.1}%", m.occupancy * 100.0)),
    ]);

    let framed = (m.radius * 2.4).clamp(0.8, 12.0);
    let crisp = 1.5 * framed / 1080.0;
    s.text(format!("{:<19} -S camera.distance={:.2}", "to fill the frame", framed));
    s.text(format!("{:<19} -S meta.point_size={:.4}", "for crisp points", crisp));
    s.text(format!(
        "{:<19} distance {:.3}, point_size {:.4}",
        "the scene sets", scene.camera_distance, scene.point_size
    ));
    s.text("radius is the 95th percentile of the walk's distance from the centre");

    // Against the framing that would render, not the scene's authored one —
    // with a --view loaded those are different numbers, and the useful one is
    // what you would get.
    let d = effective.distance;
    if (d / framed).max(framed / d) > 2.0 {
        notes.add(
            "shape",
            format!(
                "frames at distance {:.2}, {:.1}x the {:.2} that fills the frame",
                d,
                d / framed,
                framed
            ),
        );
    }
    // The bound was already being computed and compared against nothing. Fat
    // points are the most common hand-authoring mistake there is.
    if scene.point_size > crisp * 1.5 {
        notes.add(
            "render",
            format!(
                "point_size {:.4} is over the {:.4} crisp bound at 1080p",
                scene.point_size, crisp
            ),
        );
    }
    // Both numbers have been in the report for as long as it has existed and
    // were never compared. A focus outside the attractor points the camera at
    // empty space, which reads as "the scene is broken" from the image alone.
    let off = (effective.focus - m.center).length();
    if off > m.radius {
        notes.add(
            "camera",
            format!("focus is {:.2} from the centre, outside the {:.2} radius", off, m.radius),
        );
    }
    s
}

fn name_of(scene: &Scene, i: usize) -> String {
    scene
        .transform_names
        .get(i)
        .cloned()
        .flatten()
        .unwrap_or_else(|| format!("#{}", i))
}

/// The transforms: three lines each, always, in a block that labels every row
/// because by map 5 its header is twelve lines up.
fn maps_section(subject: &Subject, resolved: &Palette, notes: &mut Notes) -> Section {
    let scene = subject.scene;
    let mut s = Section::new("maps");
    let total: f32 = scene.transforms.iter().map(|t| t.weight).sum();

    s.text(format!(
        "{}   share of the walk, contraction, colour as rendered",
        scene.transforms.len()
    ));

    let names: Vec<String> = (0..scene.transforms.len()).map(|i| name_of(scene, i)).collect();
    let nw = names.iter().map(|n| n.chars().count()).max().unwrap_or(6).clamp(6, 14);
    let scales: Vec<String> = scene
        .transforms
        .iter()
        .map(|t| fmt_scale(t.matrix.to_scale_rotation_translation().0))
        .collect();
    let sw = scales.iter().map(|s| s.chars().count()).max().unwrap_or(5).clamp(5, 22);

    for (i, t) in scene.transforms.iter().enumerate() {
        let (_, rot, trans) = t.matrix.to_scale_rotation_translation();
        let (rx, ry, rz) = rot.to_euler(glam::EulerRot::XYZ);
        let share = if total > 0.0 { t.weight / total * 100.0 } else { 0.0 };
        let c = contraction_of(t);

        // The colour a map actually renders as, which is the thing you cannot
        // get from the file: in palette mode the gradient sampled at the map's
        // own index, in the other two the map's own RGB.
        let rendered = match scene.color_mode {
            ColorMode::Palette => resolved.sample(t.color_value),
            _ => scene.colors.get(i).copied().unwrap_or(Vec3::ZERO),
        };

        // A map that expands has no fixed point to walk toward and a map that
        // never fires is dead weight. Both go in `notes` rather than in a
        // marker column here: the judgement belongs at the top of the report,
        // and a column that is empty on every healthy scene is a column that
        // costs fifteen rows to say nothing.
        if c.abs() >= 1.0 {
            notes.add(
                "maps",
                format!("[{}] {} expands ({:.3}) — the walk will not settle", i, names[i], c.abs()),
            );
        }
        if t.weight <= 0.0 {
            notes.add("maps", format!("[{}] {} has weight 0 and never fires", i, names[i]));
        }

        s.text(format!(
            "[{}] {:<nw$} {}   {} {:>6.3}   {} @{:.2}",
            i,
            names[i],
            percent(share),
            if c.abs() >= 1.0 { "expansion  " } else { "contraction" },
            c,
            hex_tinted(rendered, subject.color),
            t.color_value,
            nw = nw
        ));

        // A per-map colour speed that differs from the global one is either
        // explicit in the file or derived from color_falloff, and either way it
        // is a lever CRAFT.md names and the report never showed.
        let speed = if (t.color_speed - scene.color_speed).abs() > 5e-3 {
            format!("   speed {:.2}", t.color_speed)
        } else {
            String::new()
        };
        s.text(format!(
            "    scale {:<sw$}   rot ({:>4.0},{:>4.0},{:>4.0})°{}",
            scales[i],
            degrees(rx),
            degrees(ry),
            degrees(rz),
            speed,
            sw = sw
        ));
        s.text(format!("    move  {}", point(trans)));
        // Always printed, `linear` and all, and on its own line: two reports
        // that diff row for row is the entire reason the fixed-schema
        // convention exists, and a line that appears only sometimes breaks it.
        // Its own line because a real summary runs to 41 columns — longer than
        // anything it could share with.
        s.text(format!("    vars  {}", t.variation_summary()));
    }

    // Rotations are re-derived from the matrix, so a transform authored as
    // (-26, 138, 0) can come back as (154, 42, -180) — the same rotation on
    // the other euler branch. Say so rather than have someone diff it against
    // the file and conclude the loader is wrong. At the end, because it is a
    // footnote and the top of a block is the best real estate in it.
    s.text("a negative contraction is a map that reflects; one at or past 1.0");
    s.text("expands. rotations are re-derived from the matrix, so the euler");
    s.text("branch may differ from the file — the rotation itself is the same");
    s
}

fn render_section(scene: &Scene) -> Section {
    let mut s = Section::new("render");
    s.cells(vec![
        Cell::new("point_size", size(scene.point_size)),
        Cell::new("point_count", count(scene.point_count)),
        Cell::note(if scene.point_count_defaulted { "(default)" } else { "" }),
    ]);
    s.cells(vec![
        Cell::new("haze", amount(scene.haze)),
        Cell::new("decay", amount(scene.decay)),
    ]);
    s.cells(vec![
        Cell::new("exposure", amount(scene.exposure)),
        Cell::new("background", point(scene.background)),
    ]);
    s.text("background is linear RGB, as the shader takes it, not sRGB");
    s
}

fn colour_section(subject: &Subject, resolved: &Palette, notes: &mut Notes) -> Section {
    let scene = subject.scene;
    let mut s = Section::new("colour");

    // The gradient is the one render property a file genuinely cannot convey:
    // `[palette] name = "ember"` says nothing about what ember looks like, and
    // twenty-four floats say less. So print it, as hex that survives a pipe.
    s.cells(vec![
        Cell::new("mode", word(scene.color_mode.name())),
        Cell::note(match scene.color_mode {
            ColorMode::Palette => resolved.describe(),
            ColorMode::Transforms => {
                format!("{} transform colours, evenly spread", scene.colors.len())
            }
            ColorMode::Mix => {
                format!("{} transform colours mixed as RGB", scene.colors.len())
            }
        }),
    ]);
    s.cells(vec![
        Cell::new("speed", amount(scene.color_speed)),
        Cell::new("falloff", amount(scene.color_falloff)),
        Cell::new("contrast", amount(scene.color_contrast)),
    ]);

    let (mean, swing) = resolved.luminance_profile();
    s.cells(vec![
        Cell::new("luminance", amount(mean)),
        Cell::new("swing", amount(swing)),
    ]);

    let stops: Vec<Vec3> = match scene.color_mode {
        // The ring would be a lie in mix mode: it never indexes one. Show the
        // colours that actually get blended.
        ColorMode::Mix => scene.colors.clone(),
        _ => (0..RAMP_STOPS)
            .map(|i| resolved.sample(i as f32 / RAMP_STOPS as f32))
            .collect(),
    };
    let ramp_label = match scene.color_mode {
        ColorMode::Mix => "colours",
        _ => "ramp",
    };
    for (i, row) in ramp_rows(&stops, subject.color, "", 6).into_iter().enumerate() {
        s.text(format!("{:<8}{}", if i == 0 { ramp_label } else { "" }, row));
    }
    if subject.color && scene.color_mode == ColorMode::Mix {
        s.text(format!("{:<8}{}", "", color_swatch(&stops)));
    }
    if subject.color && scene.color_mode != ColorMode::Mix {
        s.text(format!("{:<8}{}", "", swatch(resolved, 48)));
    }

    // The palette is the only shading this renderer has, so a flat one is
    // worth saying out loud rather than leaving to be discovered.
    if swing < 0.15 {
        notes.add("colour", format!("the gradient is flat (swing {:.2}) — renders unshaded", swing));
    }
    if scene.color_mode == ColorMode::Transforms && scene.palette.is_some() {
        notes.add("colour", "this scene carries a [palette]; --color-mode palette renders it");
    }
    if scene.color_contrast > 1.5 && scene.color_mode != ColorMode::Mix {
        notes.add(
            "colour",
            format!(
                "color_contrast {:.2} stretches the index — only part of the ramp is reached",
                scene.color_contrast
            ),
        );
    }
    if scene.color_mode == ColorMode::Mix {
        s.text("mix mode has no colormap, so transform *combinations* are");
        s.text("distinguishable and color_contrast does not apply");
    }
    s
}

fn camera_section(
    subject: &Subject,
    effective: &crate::camera::OrbitCamera,
    layered: bool,
    notes: &mut Notes,
) -> Section {
    let scene = subject.scene;
    let chart = effective.chart();
    let mut s = Section::new("camera");
    s.pair("yaw", angle(chart.yaw.radians()));
    s.pair("pitch", angle(chart.pitch.radians()));
    s.pair("roll", angle(chart.roll.radians()));
    s.pair("distance", length(effective.distance));
    s.pair("focus", point(effective.focus));
    s.text(format!(
        "from {}",
        match (subject.view.is_some(), subject.camera.is_empty()) {
            (true, false) => "--view, then the camera flags",
            (true, true) => "--view",
            (false, false) => "the scene, then the camera flags",
            (false, true) => "the scene",
        }
    ));
    if layered {
        // Nothing is lost by asking about a view: the scene's own framing
        // stays underneath the one that would render.
        let own = scene.camera_orientation.yaw_pitch_roll();
        s.text(format!(
            "the scene's own: yaw {:.4} pitch {:.4} distance {:.3}",
            own.yaw.radians(),
            own.pitch.radians(),
            scene.camera_distance
        ));
        s.text(format!("                 focus {}", point(scene.camera_focus)));
    }
    if scene.zoom.is_some() {
        // Under infinite zoom a framing is only defined up to a zoom period,
        // and the renderer canonicalises it — so `--distance 6` can be
        // reported as 2.160 with the yaw turned by the twist. Say why, or it
        // reads as the flag having been ignored.
        s.text("under infinite zoom, reported in the canonical period as it renders");
        let _ = &mut *notes;
    }
    s
}

fn path_section(scene: &Scene) -> Section {
    let mut s = Section::new("path");
    let Some(p) = scene.camera_path.as_ref().filter(|p| p.playable()) else {
        s.text("none authored (the default full-turn turntable applies)");
        return s;
    };

    s.text(format!(
        "{} keypoint{}, {}, {:.1}s",
        p.keys.len(),
        if p.keys.len() == 1 { "" } else { "s" },
        match p.loops {
            crate::path::Loop::Zoom(z) => format!(
                "zoom loop, {} period{} down per loop",
                z.periods,
                if z.periods == 1 { "" } else { "s" }
            ),
            crate::path::Loop::PingPong => "ping-pong loop".to_string(),
            l => l.kind().label().to_string(),
        },
        p.duration()
    ));
    // The keys are the subject of path authoring and the report only ever
    // counted them. A count tells you a path exists; the keys tell you where
    // it goes, which is what you came to find out.
    s.text(format!("{:<5}{:>9}{:>9}{:>9}{:>11}   {}", "key", "yaw", "pitch", "roll", "distance", "route"));
    for (i, k) in p.keys.iter().enumerate() {
        let c = k.orientation.yaw_pitch_roll();
        let route = match p.routes.get(i) {
            Some(crate::path::Route::Turns(0)) | None => "short".to_string(),
            Some(crate::path::Route::Turns(n)) => format!("{:+} turn{}", n, if n.abs() == 1 { "" } else { "s" }),
            Some(crate::path::Route::Exact(t)) => format!("exact, {:.0}°", t.magnitude().to_degrees()),
        };
        let snap = |a: f32| if a.abs() < 5e-5 { 0.0 } else { a };
        s.text(format!(
            "[{}]{:>9.4}{:>9.4}{:>9.4}{:>11.3}   {}",
            i,
            snap(c.yaw.radians()),
            snap(c.pitch.radians()),
            snap(c.roll.radians()),
            k.distance,
            if i < p.segments() { route } else { String::new() }
        ));
    }
    // Focus is usually one point for the whole path, so it earns a column only
    // when it moves.
    let base = p.keys[0].focus;
    for (i, k) in p.keys.iter().enumerate() {
        if (k.focus - base).length() > 1e-4 || i == 0 {
            s.text(format!("[{}] focus {}", i, point(k.focus)));
        }
    }
    s.text("route is how each segment gets to the next key: the short way, or");
    s.text("that plus whole extra turns");
    s
}

fn zoom_section(scene: &Scene, notes: &mut Notes) -> Section {
    let mut s = Section::new("zoom");
    let Some(spec) = &scene.zoom else {
        return zoom_eligibility(scene, s);
    };

    let r = match Renorm::build(spec, &scene.transforms, scene.camera_distance) {
        Ok(r) => r,
        Err(e) => {
            s.text(format!("BROKEN — {}", e));
            notes.add("zoom", format!("the zoom will not build: {}", e));
            return s;
        }
    };

    // One 240-character prose sentence used to carry all of this, which hid
    // that `radius`, `levels`, `octave_falloff` and `edge_guard` are the four
    // things you would actually reach for. Renorm::summary() stays for the
    // GUI's status bar, where one line is the whole budget.
    s.text(format!("on   map [{}] {}", r.map, name_of(scene, r.map)));
    s.pair("fixed point", point(r.fixed_point));
    s.cells(vec![
        Cell::new("scale", length(r.scale)),
        Cell::note(format!(
            "{:.2} octaves/period, {:.0}° twist",
            r.log_scale / std::f32::consts::LN_2,
            r.twist_degrees()
        )),
    ]);
    s.cells(vec![
        Cell::new("radius", amount(spec.radius)),
        Cell::new("levels", amount(spec.levels)),
    ]);
    s.cells(vec![
        Cell::new("octave_falloff", amount(spec.octave_falloff)),
        Cell::new("edge_guard", amount(spec.edge_guard)),
    ]);
    match r.guard_span() {
        Some((start, end)) => {
            s.cells(vec![
                Cell::new(
                    "guard band",
                    format!("{:>8}", format!("{:.2}x-{:.2}x", start, end)),
                ),
                Cell::note(format!("of the eye distance, {:.1} octaves wide", r.guard_width)),
            ]);
            // The guard then dims material the frame wanted: a steady cost
            // rather than a wrap artifact, and it is the radius that wants
            // raising.
            if start < MIN_RADIUS {
                notes.add("zoom", "edge-guard ramp reaches into the view; raise zoom.radius");
            }
        }
        None => {
            s.cells(vec![Cell::new("guard band", word("none")), Cell::note("hard outer edge")]);
            notes.add("zoom", "edge_guard is 0 — the wrap will step at the band edge");
        }
    }
    s.cells(vec![
        Cell::new("rendered", format!("{:>8.0}", r.periods)),
        Cell::note(format!(
            "periods ({:.1} octaves)",
            r.periods * r.log_scale / std::f32::consts::LN_2
        )),
    ]);

    if !r.band_covers_the_view() {
        notes.add(
            "zoom",
            format!(
                "band too short (radius {:.2}x the eye distance, needs {:.2}x) — \
                 material blinks out at each wrap",
                r.radius / r.band,
                MIN_RADIUS
            ),
        );
    }
    if !r.similar {
        notes.add(
            "zoom",
            format!("map is not a similarity (defect {:.2}) — the wrap will show a seam", r.defect),
        );
    }
    s
}

/// Eligibility is four conditions spread over two files; checking it by eye is
/// exactly the sort of thing this command exists to stop. The best thing in
/// the report — every eligible map comes out as a command you can run — so it
/// keeps its shape, split across two lines only to stay inside the margin.
fn zoom_eligibility(scene: &Scene, mut s: Section) -> Section {
    s.text("off   maps that could carry it, and why the others cannot");
    let mut any = false;
    for i in 0..scene.transforms.len() {
        // The reference `--zoom` takes, not the name the report shows: an
        // unnamed map is `--zoom 3`, and `--zoom #3` would not resolve.
        let name = scene
            .transform_names
            .get(i)
            .cloned()
            .flatten()
            .unwrap_or_else(|| i.to_string());
        match Renorm::build(
            &ZoomSpec { map: i, ..Default::default() },
            &scene.transforms,
            scene.camera_distance,
        ) {
            Ok(r) => {
                any = true;
                s.text(format!(
                    "--zoom {:<14}{:.2} octaves/period, {:>3.0}° twist",
                    name,
                    r.log_scale / std::f32::consts::LN_2,
                    r.twist_degrees()
                ));
                s.text(format!("{:<21}fixed point {}", "", point(r.fixed_point)));
                let flaw = match (r.defect > 0.02, r.band_covers_the_view()) {
                    (true, _) => "not a similarity — the wrap will seam",
                    (_, false) => "band too short — see renorm::MIN_RADIUS",
                    _ => "",
                };
                if !flaw.is_empty() {
                    s.text(format!("{:<21}{}", "", flaw));
                }
            }
            Err(e) => {
                let head = format!("[{}] {:<14}no: ", i, name);
                let hang = head.chars().count();
                for line in wrap(&head, hang, &strip_prefix(&e)) {
                    s.text(line);
                }
            }
        }
    }
    if !any {
        s.text("(none — infinite zoom needs a pure affine map that contracts");
        s.text("on all three axes)");
    }
    s
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
        assert!(text.contains("maps     2"), "{}", text);
        // blank()'s two maps are plain half-scales: both eligible
        assert!(text.contains("--zoom 0"), "{}", text);
        assert!(text.contains("--zoom 1"), "{}", text);
        // and the measurement ran
        assert!(text.contains("centre"), "{}", text);
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
        assert!(!text.contains("view "), "{}", text);
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
        assert!(text.contains("point_size        0.0020   scene      0.0120"), "{}", text);
        assert!(text.contains("renderer           unset   default  points"), "{}", text);
    }

    /// A view is the framing that would render, so the camera line reports it
    /// — with the scene's own kept underneath rather than overwritten.
    #[test]
    fn the_camera_line_follows_the_view() {
        let scene = Scene::blank();
        let v = bare_view();
        let text = report(&Subject::new(&scene, "blank").with_view("views/v.toml", &v));
        assert!(text.contains("yaw         1.2500 rad"), "{}", text);
        assert!(text.contains("focus     ( 0.100,  0.200,  0.300)"), "{}", text);
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
        assert!(text.contains("distance     9.500"), "{}", text);
        assert!(text.contains("--view, then the camera flags"), "{}", text);
    }

    /// Angles carry both units and positions always have three components:
    /// the two shapes everything else in the report is written against. The
    /// sign has its own column, so a negative value doesn't shift its row.
    #[test]
    fn quantities_are_written_one_way() {
        assert_eq!(point(Vec3::new(1.0, -0.5, 0.0)), "( 1.000, -0.500,  0.000)");
        assert_eq!(point(Vec3::ZERO).len(), point(Vec3::splat(-1.0)).len());
        assert!(angle(std::f32::consts::PI).contains("rad"));
        assert!(angle(std::f32::consts::PI).contains("180.0°"));
    }

    /// Rule 5, checked rather than intended. A wrapped row is worse than no
    /// table, so the margin is the one layout rule worth a test.
    #[test]
    fn no_line_is_wider_than_the_margin() {
        for scene in [Scene::blank(), crate::randomize::random_flame(
            &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(7),
        )] {
            let text = report(&Subject::new(&scene, "test"));
            for line in text.lines() {
                assert!(
                    line.chars().count() <= WIDTH,
                    "{} columns: {:?}",
                    line.chars().count(),
                    line
                );
            }
        }
    }

    /// The default output is what 95% of callers get, and none of them can see
    /// an escape code. `--color` adds to that page; it never replaces part of
    /// it, so the coloured report is a strict superset.
    #[test]
    fn colour_is_opt_in_and_additive() {
        let scene = Scene::blank();
        let plain = report(&Subject::new(&scene, "blank"));
        assert!(!plain.contains('\x1b'), "escape codes in the default output:\n{}", plain);

        let painted = report(&Subject::new(&scene, "blank").with_color(true));
        assert!(painted.contains('\x1b'), "--color emitted nothing");
        // Every hex stop still reads as text with the paint stripped out
        for stop in plain.lines().filter(|l| l.contains('#')) {
            assert!(strip_ansi(&painted).contains(stop.trim_end()), "lost: {:?}", stop);
        }
    }

    // ---- golden files -----------------------------------------------------
    //
    // A layout rework is exactly when the fixed-schema convention needs
    // something checking it: diff stability is half of what this report is
    // for, and every assertion above tests one line at a time, which is what
    // let five of them go stale under a change none of them was about.
    //
    // The fixtures are written here rather than pointed at `scenes/`, so that
    // editing a scene never breaks a test about the report — and so that what
    // each golden covers is visible in the same file as the golden.
    //
    // Re-bless with:  FRACTURIZE_BLESS=1 cargo test golden

    /// Palette mode, infinite zoom on, a camera path, a mirrored map and an
    /// expanding one — every branch the report has, in one file.
    const FIXTURE: &str = r##"
[meta]
name = "Fixture"
author = "the test suite"
point_size = 0.0018
point_count = 4_000_000
color_speed = 0.5
color_falloff = 0.6
color_contrast = 1.0
haze = 0.5
background = [0.010, 0.008, 0.016]

[camera]
distance = 3.4
yaw = 0.0
pitch = 1.12
focus = [0.0, 0.0, 0.0]
path_loop = "closed"
path_seconds = 8.0
[[camera.path]]
yaw = 0.0
pitch = 0.4
[[camera.path]]
yaw = 1.5708
pitch = 0.4
turns = 1

[palette]
interpolate = "oklab"
cyclic = true
stops = [
  { at = 0.00, color = "#0d0a10" },
  { at = 0.45, color = "#c9788f" },
  { at = 0.86, color = "#fff2f5" },
]

[[transform]]
name = "bough"
translation = [0.0, 0.0, 0.0]
scale = 0.60
rotation = [0.0, 32.0, 0.0]
color = [0.16, 0.30, 0.66]
color_value = 0.12
weight = 3.0

[[transform]]
name = "mirror-fold"
translation = [-0.32, -0.30, -0.78]
scale = [-0.45, 0.45, 0.45]
rotation = [-24.0, 16.0, 0.0]
color = [0.96, 0.70, 0.32]
color_value = 0.60
weight = 0.95
variations = { absfold = 0.55, linear = 0.45 }

[[transform]]
name = "twig"
translation = [0.0, 0.30, 0.0]
scale = [0.05, 0.62, 0.05]
rotation = [0.0, 0.0, 0.0]
color = [1.0, 0.94, 0.72]
color_value = 0.86
weight = 0.09

[zoom]
map = "bough"
radius = 3.2
levels = 14
"##;

    /// Write the fixture somewhere `Scene::load_reporting` can read it —
    /// `--set` patches TOML *text*, so a scene built in memory cannot exercise
    /// the `set` block at all.
    fn fixture_at(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("fracturize-info-{}.toml", tag));
        std::fs::write(&path, FIXTURE).expect("write fixture");
        path
    }

    fn golden(name: &str, text: &str) {
        let path = std::path::Path::new("tests/golden").join(format!("{name}.txt"));
        if std::env::var_os("FRACTURIZE_BLESS").is_some() {
            std::fs::create_dir_all("tests/golden").expect("mkdir");
            std::fs::write(&path, text).expect("bless");
            return;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} — re-bless with FRACTURIZE_BLESS=1", path.display()));
        if want != text {
            let diff: Vec<String> = want
                .lines()
                .zip(text.lines())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| format!("{:>4}  - {:?}\n      + {:?}", i + 1, a, b))
                .collect();
            panic!(
                "{} drifted ({} vs {} lines):\n{}\n\nre-bless with FRACTURIZE_BLESS=1",
                path.display(),
                want.lines().count(),
                text.lines().count(),
                diff.join("\n")
            );
        }
    }

    /// Transforms mode, no zoom, no path, nothing authored: the floor.
    #[test]
    fn golden_blank() {
        let scene = Scene::blank();
        golden("blank", &report(&Subject::new(&scene, "--blank")));
    }

    /// Palette mode, zoom on, a camera path, and two maps worth a note.
    #[test]
    fn golden_palette_zoom() {
        let path = fixture_at("plain");
        let (scene, applied) = Scene::load_reporting(&path, &[]).expect("fixture loads");
        golden("palette-zoom", &report(&Subject::new(&scene, "fixture.toml").with_set(&applied)));
    }

    /// Everything layered at once: mix mode reached through `--set`, a `--view`
    /// over the top, and the camera flags over that. The case where "what did
    /// I actually change?" is hardest to answer and the report has to.
    #[test]
    fn golden_layered() {
        let path = fixture_at("layered");
        let sets = [
            "meta.color_mode=mix".to_string(),
            "transform.twig.weight=0.5".to_string(),
            "zoom.edge_guard=0".to_string(),
        ];
        let (scene, applied) = Scene::load_reporting(&path, &sets).expect("fixture loads");
        let view = bare_view();
        // A stable name, not the temp path the fixture happens to live at:
        // TMPDIR differs by machine and a golden must not.
        let text = report(
            &Subject::new(&scene, "fixture.toml")
                .with_set(&applied)
                .with_view("views/v.toml", &view)
                .with_camera(CameraOverride { distance: Some(2.5), ..Default::default() }),
        );
        golden("layered", &text);
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// A clean scene says so in one line an agent can branch on, and a broken
    /// one names what is wrong and where to look.
    #[test]
    fn notes_are_collected_into_one_block() {
        // Nothing raised is the cheapest possible "this scene is sound", and
        // it has to be one line rather than an absence: a block that vanishes
        // when empty can't be told apart from one you forgot to look at.
        assert!(notes_section(&Notes::default()).rows.len() == 1);
        let mut none = String::new();
        notes_section(&Notes::default()).render(&mut none);
        assert_eq!(none, "notes    none\n");

        // Every diagnostic carries the section it is about, so a reader knows
        // where to look without the note having to say.
        let mut broken = Scene::blank();
        broken.transforms[1].weight = 0.0;
        let text = report(&Subject::new(&broken, "blank"));
        assert!(text.contains("maps    [1] #1 has weight 0 and never fires"), "{}", text);
    }

    /// A map that reflects and a map that expands used to print exactly like a
    /// plain one, because the clamped contraction the colour system wants
    /// throws both facts away.
    #[test]
    fn contraction_shows_reflection_and_expansion() {
        let mut scene = Scene::blank();
        scene.transforms[0].matrix *= glam::Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        scene.transforms[1].matrix *= glam::Mat4::from_scale(Vec3::splat(4.0));
        let text = report(&Subject::new(&scene, "blank"));
        assert!(text.contains("contraction -0.500"), "{}", text);
        assert!(text.contains("expansion    2.000"), "{}", text);
        assert!(text.contains("expands (2.000)"), "{}", text);
    }

    /// Every map is three lines whatever it contains. Two reports that diff
    /// row for row is the whole reason the convention exists.
    #[test]
    fn a_map_is_always_four_lines() {
        let scene = Scene::blank();
        let text = report(&Subject::new(&scene, "blank"));
        let maps: Vec<&str> = text
            .lines()
            .skip_while(|l| !l.starts_with("maps"))
            .take_while(|l| !l.is_empty())
            .collect();
        // header + 2 maps x 4 lines + 3 footnote lines
        assert_eq!(maps.len(), 12, "{:#?}", maps);
        assert!(maps.iter().filter(|l| l.contains("linear")).count() == 2, "{:#?}", maps);
    }

    /// A suggestion you have to reassemble into a flag is a suggestion
    /// half-given, and the reader who has the flags memorised is the smallest
    /// slice of the mix.
    #[test]
    fn suggestions_are_runnable() {
        let scene = Scene::blank();
        let text = report(&Subject::new(&scene, "blank"));
        assert!(text.contains("-S camera.distance="), "{}", text);
        assert!(text.contains("-S meta.point_size="), "{}", text);
    }

    /// `--set` is applied before the report is built, so without this the
    /// report shows a value the file does not contain and says nothing.
    #[test]
    fn set_overrides_say_what_they_replaced() {
        let scene = Scene::blank();
        let applied = [Applied {
            path: "meta.haze".into(),
            from: Some("0.5".into()),
            to: "0.9".into(),
        }];
        let text = report(&Subject::new(&scene, "s.toml").with_set(&applied));
        assert!(text.contains("set      1 override applied"), "{}", text);
        assert!(text.contains("meta.haze"), "{}", text);
        assert!(text.contains("was  0.5"), "{}", text);
    }
}
