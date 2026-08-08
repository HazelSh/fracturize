//! Finite subgroups of SO(3), as a property a transform can carry.
//!
//! For a finite group `G` and maps `{fᵢ}`, the IFS `{g ∘ fᵢ : g ∈ G}` has an
//! attractor that is exactly `G`-symmetric. The proof is one line —
//! `A = ⋃_{g,i} g(fᵢ(A))`, so for any `h ∈ G`, `h(A) = ⋃ hg fᵢ(A) = A`, because
//! `hG = G` — and the consequence is the reason this module exists: **two
//! authored maps under the icosahedral group are an effective map set of 120**,
//! from six lines of TOML.
//!
//! Note the composition order. `g` is applied *after* `f`, so it needs a slot
//! outside the variation blend; that is [`TransformSpec::post_affine`], and this
//! module is what fills it with more than one matrix.
//!
//! # The group stays live
//!
//! The orbit is never expanded into the transform list. A scene under `I` holds
//! two transforms and a group, not 120 transforms — so the panel shows two rows,
//! `pick.rs` has two things to hit, `--info` prints two maps, and a mutation
//! operator has two maps to perturb. The complexity sits here, in the one place
//! that wants it, instead of in the twelve places that don't.
//!
//! The chaos game is unchanged by this in the maths: picking `fᵢ` with weight
//! `wᵢ` and then `g` uniformly from `G` is *exactly* sampling the `|G|·N` map set
//! `{g ∘ fᵢ}` with weights `wᵢ/|G|`. No approximation, and no convergence
//! penalty — one extra RNG draw and one matrix multiply per iteration.
//!
//! # Why the elements are generated, not written down
//!
//! Sixty consistent rotation matrices are exactly the kind of arithmetic that is
//! tedious to get right and silent when wrong — a single mistyped sign gives a
//! set that is *nearly* a group, and a near-group produces an attractor that is
//! nearly symmetric, which reads as a smear rather than as an error. So the
//! elements come from **closure over two generators**: multiply until the set
//! stops growing. The generators are short enough to check by eye, and
//! [`Symmetry::elements`] is verified against the known orders in the tests.

use glam::{Mat3, Mat4, Vec3};

/// Hard ceiling on a generated group's order.
///
/// Closure over a bad pair of generators does not terminate — an axis pair that
/// generates a *dense* subgroup of SO(3) grows without bound, which is §1.1(a)
/// of the brainstorm arrived at from the wrong end. Every group this module can
/// name is finite by construction, so hitting the cap is a bug rather than a
/// user error; it is still checked, because the alternative to a cap is a hang.
pub const MAX_ORDER: usize = 256;

/// Largest fold count accepted for `C_n` / `D_n`.
///
/// `D_60` is already 120 maps from one motif. The limit exists so a typo in a
/// scene file (`group = "C1000"`) is a load error rather than 2000 matrices.
pub const MAX_FOLD: u32 = 60;

/// Two matrices are the same group element when every entry agrees to this.
/// Generated elements are exact to within a few multiplies of rounding, so the
/// tolerance only has to be loose enough to absorb that and tight enough to
/// keep a 60-element group from collapsing (its closest pair of distinct
/// elements are ~0.3 apart in this norm).
const SAME: f32 = 1e-4;

/// A finite repeat: `count` copies stepped by a similarity.
///
/// The step is the general orientation-preserving similarity of space — turn
/// about an axis, slide along a vector, and shrink — which is the smallest
/// parameter set that covers the whole family at once:
///
/// | translate | turn | scale | what you get           |
/// |-----------|------|-------|------------------------|
/// | yes       | 0    | 1     | a row                  |
/// | yes       | yes  | 1     | a helix                |
/// | 0         | yes  | <1    | a logarithmic spiral   |
/// | yes       | yes  | <1    | a cone / phyllotaxis   |
///
/// `scale` is capped at 1. `Sᵏ ∘ f` has linear part `S_linᵏ · f_lin`, so a step
/// that grows makes the far copies expansive and the walk diverges — and nothing
/// is lost by the cap, because a growing repeat of `N` copies from motif `m` is
/// the same picture as a shrinking one from `S^(N-1) m`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Repeat {
    pub count: u32,
    /// Metres per copy, along the axis or across it — this is a free vector, not
    /// constrained to the axis, so a repeat can shear as well as stack.
    pub translate: Vec3,
    /// Degrees per copy about the axis. 137.5 is the golden angle, which is why
    /// phyllotaxis is one field away rather than a special case.
    pub turn: f32,
    /// Multiplier per copy, `0 < scale <= 1`.
    pub scale: f32,
}

impl Default for Repeat {
    fn default() -> Self {
        // A visible helix rather than a degenerate stack: a repeat whose step is
        // the identity is `count` copies of one map on top of each other, which
        // looks like nothing happened and reads as a broken feature.
        Self { count: 8, translate: Vec3::new(0.0, 0.28, 0.0), turn: 45.0, scale: 0.92 }
    }
}

/// Largest `count` for a repeat. Unlike a group's order this is not forced by
/// any classification — it is a budget, matching the spirit of [`MAX_FOLD`].
pub const MAX_REPEAT: u32 = 64;

/// What a transform's post-composition set is.
///
/// Five of these are **groups**, and one is not. The distinction is not
/// pedantry, and `--info` reports it: for a group `G`, `hG = G` for every
/// `h ∈ G`, so the attractor of `{g ∘ fᵢ}` is *exactly* `G`-invariant. A
/// [`Repeat`] is a truncated progression `{S⁰ … S^(N-1)}`, which is not closed
/// — `S · S^(N-1)` is not in the set — so its attractor is a perfectly good
/// fractal that is **not** `S`-invariant. It repeats; it is not symmetric.
///
/// Both are the same thing to everything downstream, though, which is why they
/// share a type: a finite list of matrices post-composed onto a map. The
/// renderer, the GPU table and the ghost gizmos never needed to know which.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OrbitKind {
    /// `C_n`: `n` rotations about one axis. The mandala case.
    Cyclic(u32),
    /// `D_n`: `C_n` plus `n` half-turns about axes perpendicular to it — the
    /// flip that turns a rosette into something with a front and a back.
    Dihedral(u32),
    /// `T`, order 12: the rotations of a tetrahedron.
    Tetrahedral,
    /// `O`, order 24: the rotations of a cube / octahedron.
    Octahedral,
    /// `I`, order 60: the rotations of an icosahedron / dodecahedron. The one
    /// with no hand-authored equivalent anywhere in `scenes/`.
    Icosahedral,
    /// Not a group — see the type docs. `count` copies stepped by a similarity.
    Repeat(Repeat),
}

impl OrbitKind {
    /// `|G|` before any mirror extension, or a repeat's `count`.
    pub fn order(self) -> usize {
        match self {
            OrbitKind::Cyclic(n) => n as usize,
            OrbitKind::Dihedral(n) => 2 * n as usize,
            OrbitKind::Tetrahedral => 12,
            OrbitKind::Octahedral => 24,
            OrbitKind::Icosahedral => 60,
            OrbitKind::Repeat(r) => r.count as usize,
        }
    }

    /// Whether this really is a group — whether the attractor it makes is
    /// exactly invariant, or merely repetitive. See the type docs.
    pub fn is_group(self) -> bool {
        !matches!(self, OrbitKind::Repeat(_))
    }

    /// The repeat's step, for the callers that need to edit it.
    pub fn repeat(self) -> Option<Repeat> {
        match self {
            OrbitKind::Repeat(r) => Some(r),
            _ => None,
        }
    }

    /// The short name used in scene files, `--info` and the panel: `Cyc5`,
    /// `Dih3`, `Tetra`, `Octa`, `Icosa`.
    ///
    /// Abbreviated rather than the mathematician's bare `C5`/`D3`/`T`/`O`/`I`,
    /// which is correct notation and unreadable to anyone who doesn't already
    /// know it — a scene file and a panel badge both have to survive being read
    /// by someone meeting the feature for the first time. The bare letters
    /// still parse, so notation-first authors lose nothing.
    pub fn label(self) -> String {
        match self {
            OrbitKind::Cyclic(n) => format!("Cyc{}", n),
            OrbitKind::Dihedral(n) => format!("Dih{}", n),
            OrbitKind::Tetrahedral => "Tetra".to_string(),
            OrbitKind::Octahedral => "Octa".to_string(),
            OrbitKind::Icosahedral => "Icosa".to_string(),
            OrbitKind::Repeat(r) => format!("Repeat{}", r.count),
        }
    }

    /// Long name, for tooltips and the `--info` line. Says what the group *is*
    /// rather than restating the label — `Icosa (icosahedral)` would be a row
    /// that tells the reader nothing they couldn't already see.
    pub fn description(self) -> String {
        match self {
            OrbitKind::Cyclic(n) => format!("{}-fold rotation about an axis", n),
            OrbitKind::Dihedral(n) => format!("{}-fold rotation plus a half-turn flip", n),
            OrbitKind::Tetrahedral => "rotations of a tetrahedron".to_string(),
            OrbitKind::Octahedral => "rotations of a cube".to_string(),
            OrbitKind::Icosahedral => "rotations of an icosahedron".to_string(),
            // Named for what it does to the picture, since unlike the groups
            // there is no standard name to borrow.
            OrbitKind::Repeat(r) => {
                let turning = r.turn.abs() > 1e-3;
                let sliding = r.translate.length() > 1e-6;
                let shrinking = (r.scale - 1.0).abs() > 1e-4;
                match (sliding, turning, shrinking) {
                    (true, true, true) => "copies down a tapering helix".to_string(),
                    (true, true, false) => "copies along a helix".to_string(),
                    (true, false, true) => "copies along a shrinking line".to_string(),
                    (true, false, false) => "copies along a line".to_string(),
                    (false, true, true) => "copies around a shrinking spiral".to_string(),
                    (false, true, false) => "copies around a ring".to_string(),
                    (false, false, true) => "copies shrinking in place".to_string(),
                    (false, false, false) => "copies with no step — every one on top of the last"
                        .to_string(),
                }
            }
        }
    }

    /// Whether the `axis` field means anything. The polyhedral groups have no
    /// single axis — their symmetry axes come in threes, fours and fives at
    /// fixed angles to each other — so they are generated in a canonical
    /// orientation and `axis` is ignored.
    pub fn uses_axis(self) -> bool {
        matches!(
            self,
            OrbitKind::Cyclic(_) | OrbitKind::Dihedral(_) | OrbitKind::Repeat(_)
        )
    }

    /// Parse `Cyc5`, `Dih3`, `Tetra`, `Octa`, `Icosa`, the bare mathematical
    /// `C5`/`D3`/`T`/`O`/`I`, or the full words. Case-insensitive, and `C_5` is
    /// accepted for the author who writes the subscript out.
    ///
    /// Every spelling is permanent. The bare letters are the notation an author
    /// coming from the literature will reach for first, and a scene that failed
    /// to load over an abbreviation would be a gratuitous thing to do to them.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "t" | "tetra" | "tetrahedral" => return Ok(OrbitKind::Tetrahedral),
            "o" | "octa" | "octahedral" => return Ok(OrbitKind::Octahedral),
            "i" | "icosa" | "icosahedral" => return Ok(OrbitKind::Icosahedral),
            _ => {}
        }
        // Longest prefix first, so `cyc5` isn't read as C followed by `yc5`.
        let split = ["cyclic", "cyc", "c", "dihedral", "dih", "d"]
            .into_iter()
            .find_map(|p| lower.strip_prefix(p).map(|rest| (p.as_bytes()[0], rest)));
        if let Some((head, rest)) = split {
            let rest = rest.trim_start_matches(['_', ' ', '-']);
            if let Ok(n) = rest.parse::<u32>() {
                let min = if head == b'c' { 1 } else { 2 };
                if n < min {
                    return Err(format!(
                        "group '{}' needs a fold count of at least {}",
                        s, min
                    ));
                }
                if n > MAX_FOLD {
                    return Err(format!(
                        "group '{}' exceeds the {}-fold limit",
                        s, MAX_FOLD
                    ));
                }
                return Ok(if head == b'c' {
                    OrbitKind::Cyclic(n)
                } else {
                    OrbitKind::Dihedral(n)
                });
            }
        }
        Err(format!(
            "unknown symmetry group '{}'. Use Cyc<n> (n-fold about an axis), \
             Dih<n> (that plus a flip), or Tetra / Octa / Icosa for the \
             polyhedral groups. The bare C<n>, D<n>, T, O and I are accepted too",
            s
        ))
    }

    /// Fold count, for the kinds that have one.
    pub fn fold(self) -> Option<u32> {
        match self {
            OrbitKind::Cyclic(n) | OrbitKind::Dihedral(n) => Some(n),
            _ => None,
        }
    }

    /// The smallest rotation in the group, in degrees — what the gizmo's snap
    /// increment becomes while this symmetry is selected (`app.rs` otherwise
    /// snaps to 15°, on the argument that IFS aesthetics live on clean
    /// rotational symmetry; with a group active the group's own step *is* the
    /// clean one).
    pub fn snap_degrees(self) -> f32 {
        match self {
            OrbitKind::Cyclic(n) | OrbitKind::Dihedral(n) => 360.0 / n.max(1) as f32,
            OrbitKind::Tetrahedral => 120.0,
            OrbitKind::Octahedral => 90.0,
            OrbitKind::Icosahedral => 72.0,
            // A repeat's own turn is the step that keeps it coherent: nudging
            // the motif by exactly one copy's worth lands it on its neighbour.
            OrbitKind::Repeat(r) if r.turn.abs() > 1e-3 => r.turn.abs(),
            OrbitKind::Repeat(_) => 15.0,
        }
    }
}

/// How the orbit is coloured.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OrbitColor {
    /// Every copy takes the motif's own colour. A monochrome mandala, and the
    /// right default: the copies *are* the same map, and colouring them apart
    /// says they aren't.
    #[default]
    Shared,
    /// The colormap index is offset by which group element was drawn.
    ///
    /// Read the name carefully, because with the group live in the walk this
    /// does not mean what the Design-A version of it would. `g` is drawn afresh
    /// every iteration, so a walker crosses between copies constantly, and the
    /// offset tracks the **most recent** group element rather than "which copy
    /// this point is in". It reads as an interference pattern across the whole
    /// form, not as `|G|` solid petals. That is a legitimate and rather good
    /// look, but it is not the one the name suggests, so it is off by default.
    Orbit,
}

impl OrbitColor {
    pub fn name(self) -> &'static str {
        match self {
            OrbitColor::Shared => "shared",
            OrbitColor::Orbit => "orbit",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "shared" => Ok(OrbitColor::Shared),
            "orbit" => Ok(OrbitColor::Orbit),
            other => Err(format!(
                "unknown symmetry color '{}'. Use \"shared\" (every copy the \
                 motif's colour) or \"orbit\" (index offset by group element)",
                other
            )),
        }
    }
}

/// A symmetry group, resolved to its elements.
///
/// Constructed only through [`Symmetry::new`], so `elements` can never
/// disagree with the fields that describe it. Editing means building a new one,
/// which is cheap — the closure for `I` is 60 elements and a few thousand
/// multiplies, microseconds, and it happens on an edit rather than per frame.
#[derive(Clone, Debug)]
pub struct Symmetry {
    kind: OrbitKind,
    /// The rotation axis, for `C_n` / `D_n`. Normalized on construction, and
    /// carried unnormalized-but-remembered nowhere: what you set is what you
    /// get back, pointing the same way.
    axis: Vec3,
    /// Extend the group by the central inversion `−I`, doubling its order.
    ///
    /// `−I` commutes with everything, so `G × {±I}` is a group for *any* `G`
    /// — which is what makes this one flag rather than a per-group table of
    /// which mirror extensions are legal. Geometrically it puts a second copy
    /// of every petal through the origin, and since the elements stay
    /// orthogonal it changes no contraction and so moves no dimension.
    mirror: bool,
    color: OrbitColor,
    /// The group itself. Element 0 is always the identity.
    elements: Vec<Mat4>,
}

impl PartialEq for Symmetry {
    /// Compares what was *authored*. The elements are a pure function of these
    /// four fields, so comparing them as well would only be slower.
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.mirror == other.mirror
            && self.color == other.color
            && (self.axis - other.axis).length_squared() < 1e-12
    }
}

impl Symmetry {
    /// Build a group or a repeat. `axis` is ignored for the polyhedral kinds.
    ///
    /// Fails only on inputs that cannot name one: a zero axis, a repeat whose
    /// count or scale is out of range, or a closure that runs past
    /// [`MAX_ORDER`] (which no group `OrbitKind` can actually do, and which is
    /// checked because a hang would be worse than an error).
    pub fn new(
        kind: OrbitKind,
        axis: Vec3,
        mirror: bool,
        color: OrbitColor,
    ) -> Result<Self, String> {
        let axis = if kind.uses_axis() {
            let n = axis.normalize_or_zero();
            if n == Vec3::ZERO {
                return Err(format!(
                    "symmetry group {} needs a non-zero axis",
                    kind.label()
                ));
            }
            n
        } else {
            // Kept as authored for round-tripping, but not used to generate.
            axis.normalize_or(Vec3::Y)
        };

        // A repeat is not generated by closure — it *has* no closure, being a
        // truncated progression rather than a group — so it takes its own path
        // and never reaches the group machinery below.
        if let OrbitKind::Repeat(r) = kind {
            return Self::from_repeat(r, axis, color);
        }

        let mut generators = match kind {
            OrbitKind::Cyclic(n) => vec![Mat3::from_axis_angle(axis, tau_over(n))],
            OrbitKind::Dihedral(n) => {
                let perp = perpendicular_to(axis);
                vec![
                    Mat3::from_axis_angle(axis, tau_over(n)),
                    Mat3::from_axis_angle(perp, std::f32::consts::PI),
                ]
            }
            // The tetrahedral and octahedral groups share their 3-fold
            // generator — the body diagonal of the cube — and differ only in
            // whether the coordinate axes carry a half-turn or a quarter-turn.
            OrbitKind::Tetrahedral => vec![
                Mat3::from_axis_angle(Vec3::Z, std::f32::consts::PI),
                Mat3::from_axis_angle(Vec3::ONE.normalize(), tau_over(3)),
            ],
            OrbitKind::Octahedral => vec![
                Mat3::from_axis_angle(Vec3::Z, std::f32::consts::FRAC_PI_2),
                Mat3::from_axis_angle(Vec3::ONE.normalize(), tau_over(3)),
            ],
            // In the standard orientation the icosahedron's vertices are the
            // cyclic permutations of (0, ±1, ±φ), which puts a 5-fold axis
            // through (0, 1, φ) and 2-fold axes along the coordinate axes.
            // Those two generate all 60: a subgroup holding an order-5 element
            // is `D₅` or the whole group, and this half-turn is not
            // perpendicular to that 5-fold axis, so it is not in that `D₅`.
            // Returned above; listed so that adding a kind is a build error
            // here rather than a silent fallthrough.
            OrbitKind::Repeat(_) => unreachable!("repeats take from_repeat"),
            OrbitKind::Icosahedral => {
                let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
                vec![
                    Mat3::from_axis_angle(Vec3::Z, std::f32::consts::PI),
                    Mat3::from_axis_angle(Vec3::new(0.0, 1.0, phi).normalize(), tau_over(5)),
                ]
            }
        };

        if mirror {
            generators.push(Mat3::from_diagonal(Vec3::splat(-1.0)));
        }

        let elements = closure(&generators)?;
        let expected = kind.order() * if mirror { 2 } else { 1 };
        if elements.len() != expected {
            // Not reachable from any input this module accepts; it fires only
            // if a generator above is edited into something wrong, which is
            // exactly the mistake that would otherwise ship as a smear.
            return Err(format!(
                "symmetry group {} generated {} elements, expected {}",
                kind.label(),
                elements.len(),
                expected
            ));
        }

        Ok(Self {
            kind,
            axis,
            mirror,
            color,
            elements: elements.into_iter().map(mat3_to_mat4).collect(),
        })
    }

    /// Build the powers `S⁰ … S^(N-1)` of a repeat's step.
    ///
    /// Accumulated by multiplication rather than by rebuilding each power from
    /// `k · turn` and `scaleᵏ`, so that the copies chain exactly: any drift is
    /// shared with its neighbours instead of each copy drifting independently
    /// from an ideal it alone knows about.
    fn from_repeat(r: Repeat, axis: Vec3, color: OrbitColor) -> Result<Self, String> {
        if r.count == 0 || r.count > MAX_REPEAT {
            return Err(format!(
                "a repeat needs between 1 and {} copies, not {}",
                MAX_REPEAT, r.count
            ));
        }
        if !(r.scale > 0.0) || r.scale > 1.0 {
            // See `Repeat`: a growing step makes the far copies expansive and
            // the walk unbounded. The cap costs no pictures, only a re-anchoring.
            return Err(format!(
                "a repeat's scale must be greater than 0 and at most 1, not {}. \
                 A growing repeat is the same picture as a shrinking one started \
                 from its far end",
                r.scale
            ));
        }
        if !r.translate.is_finite() || !r.turn.is_finite() {
            return Err("a repeat's step must be a finite translation and turn".to_string());
        }

        let step = Mat4::from_scale_rotation_translation(
            Vec3::splat(r.scale),
            glam::Quat::from_axis_angle(axis, r.turn.to_radians()),
            r.translate,
        );
        let mut elements = Vec::with_capacity(r.count as usize);
        let mut current = Mat4::IDENTITY;
        for _ in 0..r.count {
            elements.push(current);
            current *= step;
        }

        Ok(Self { kind: OrbitKind::Repeat(r), axis, mirror: false, color, elements })
    }

    pub fn kind(&self) -> OrbitKind {
        self.kind
    }
    pub fn axis(&self) -> Vec3 {
        self.axis
    }
    pub fn mirror(&self) -> bool {
        self.mirror
    }
    pub fn color(&self) -> OrbitColor {
        self.color
    }

    /// The group's elements. Element 0 is the identity, so the *first* copy of
    /// a motif is always the motif itself and a gizmo drawn at the identity
    /// coincides with the one already there.
    pub fn elements(&self) -> &[Mat4] {
        &self.elements
    }

    /// `|G|` — how many copies of each motif this makes.
    pub fn order(&self) -> usize {
        self.elements.len()
    }

    /// Whether this is a group rather than a repeat. See [`OrbitKind`].
    pub fn is_group(&self) -> bool {
        self.kind.is_group()
    }

    /// The linear scale factor each element contributes, in element order.
    ///
    /// Every group element is orthogonal, so for a group this is all ones and
    /// the copies of a motif all contract alike. A repeat's `k`-th copy carries
    /// `scaleᵏ`, so they do not — which the similarity dimension has to know
    /// about, or a tapering helix reads as `count` copies of the widest one.
    pub fn element_scales(&self) -> Vec<f32> {
        match self.kind {
            OrbitKind::Repeat(r) => (0..r.count).map(|k| r.scale.powi(k as i32)).collect(),
            _ => vec![1.0; self.elements.len()],
        }
    }

    /// `Cyc5`, `Dih3 + mirror`, `Icosa`, … — the name with a mirror marker.
    ///
    /// Display only: the mirror is its own `mirror = true` field in a scene
    /// file, so this never has to round-trip through `OrbitKind::parse`.
    pub fn label(&self) -> String {
        if self.mirror {
            format!("{} + mirror", self.kind.label())
        } else {
            self.kind.label()
        }
    }

    /// A one-line summary for `--info` and the panel badge:
    /// `Cyc5 about [0.000 1.000 0.000]`, or `Icosa (rotations of an
    /// icosahedron)` where there is no axis.
    pub fn summary(&self) -> String {
        if let OrbitKind::Repeat(_) = self.kind {
            // The axis matters less than the shape here — a helix and a ring
            // differ in the step, not in what they are aimed along.
            return format!("{} ({})", self.label(), self.kind.description());
        }
        if self.kind.uses_axis() {
            format!(
                "{} about [{:.3} {:.3} {:.3}]",
                self.label(),
                self.axis.x,
                self.axis.y,
                self.axis.z
            )
        } else {
            format!("{} ({})", self.label(), self.kind.description())
        }
    }

    /// The images of `m` under every group element — the motif and its
    /// `|G| − 1` ghosts, in element order, starting with `m` itself.
    ///
    /// Left-multiplied, because the group composes *after* the map.
    pub fn orbit(&self, m: Mat4) -> impl Iterator<Item = Mat4> + '_ {
        self.elements.iter().map(move |g| *g * m)
    }

}

fn tau_over(n: u32) -> f32 {
    std::f32::consts::TAU / n.max(1) as f32
}

/// Any unit vector perpendicular to `axis` — the second `D_n` generator's axis.
/// Which one it is only rotates the whole group about `axis`, which is a phase
/// the author can't distinguish and the motif's own rotation absorbs.
fn perpendicular_to(axis: Vec3) -> Vec3 {
    let seed = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    axis.cross(seed).normalize_or(Vec3::X)
}

fn mat3_to_mat4(m: Mat3) -> Mat4 {
    Mat4::from_mat3(m)
}

/// Multiply the generators together until the set stops growing.
///
/// Breadth-first over products, which for a group of order `n` from `k`
/// generators costs `n·k` multiplies and an `O(n²)` membership test — 60 × 2
/// multiplies and ~3600 comparisons for the icosahedral group, i.e. nothing.
fn closure(generators: &[Mat3]) -> Result<Vec<Mat3>, String> {
    let mut elements = vec![Mat3::IDENTITY];
    let mut frontier = vec![Mat3::IDENTITY];

    while let Some(current) = frontier.pop() {
        for g in generators {
            let next = *g * current;
            if !elements.iter().any(|e| same(e, &next)) {
                if elements.len() >= MAX_ORDER {
                    return Err(format!(
                        "symmetry generators do not close: passed {} elements. \
                         The rotations generate a dense subgroup of SO(3) rather \
                         than a finite group",
                        MAX_ORDER
                    ));
                }
                elements.push(next);
                frontier.push(next);
            }
        }
    }

    Ok(elements)
}

fn same(a: &Mat3, b: &Mat3) -> bool {
    a.to_cols_array()
        .iter()
        .zip(b.to_cols_array().iter())
        .all(|(x, y)| (x - y).abs() < SAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(kind: OrbitKind) -> Symmetry {
        Symmetry::new(kind, Vec3::Y, false, OrbitColor::Shared).unwrap()
    }

    /// The whole point of generating rather than tabulating: if a generator is
    /// wrong the order is wrong, and the order is a number every reference
    /// agrees on.
    #[test]
    fn generated_groups_have_the_known_orders() {
        for (kind, order) in [
            (OrbitKind::Cyclic(1), 1),
            (OrbitKind::Cyclic(2), 2),
            (OrbitKind::Cyclic(5), 5),
            (OrbitKind::Cyclic(17), 17),
            (OrbitKind::Dihedral(2), 4),
            (OrbitKind::Dihedral(3), 6),
            (OrbitKind::Dihedral(6), 12),
            (OrbitKind::Tetrahedral, 12),
            (OrbitKind::Octahedral, 24),
            (OrbitKind::Icosahedral, 60),
        ] {
            assert_eq!(sym(kind).order(), order, "{:?}", kind);
        }
    }

    #[test]
    fn a_mirror_doubles_every_order() {
        for kind in [
            OrbitKind::Cyclic(3),
            OrbitKind::Dihedral(4),
            OrbitKind::Tetrahedral,
            OrbitKind::Octahedral,
            OrbitKind::Icosahedral,
        ] {
            let plain = sym(kind);
            let mirrored = Symmetry::new(kind, Vec3::Y, true, OrbitColor::Shared).unwrap();
            assert_eq!(mirrored.order(), plain.order() * 2, "{:?}", kind);
        }
    }

    /// The group axioms, checked numerically on the generated set. This is what
    /// makes the attractor exactly rather than nearly symmetric: closure is
    /// what the theorem needs (`hG = G`), and without it the orbit smears.
    #[test]
    fn generated_sets_are_actually_groups() {
        for kind in [
            OrbitKind::Cyclic(5),
            OrbitKind::Dihedral(3),
            OrbitKind::Tetrahedral,
            OrbitKind::Octahedral,
            OrbitKind::Icosahedral,
        ] {
            for mirror in [false, true] {
                let g = Symmetry::new(kind, Vec3::new(0.3, 1.0, -0.2), mirror, OrbitColor::Shared)
                    .unwrap();
                let els: Vec<Mat3> = g.elements().iter().map(|m| Mat3::from_mat4(*m)).collect();

                assert!(same(&els[0], &Mat3::IDENTITY), "element 0 must be the identity");

                for a in &els {
                    // Orthogonal: a symmetry may not stretch anything, or it
                    // would change the contraction and move the dimension.
                    let should_be_i = a.transpose() * *a;
                    assert!(same(&should_be_i, &Mat3::IDENTITY), "{:?} not orthogonal", kind);

                    // Closed under multiplication, and every element has its
                    // inverse in the set.
                    assert!(
                        els.iter().any(|e| same(e, &a.transpose())),
                        "{:?}: an element's inverse is missing",
                        kind
                    );
                    for b in &els {
                        let product = *a * *b;
                        assert!(
                            els.iter().any(|e| same(e, &product)),
                            "{:?}: not closed under multiplication",
                            kind
                        );
                    }
                }

                // Distinct: a duplicate would mean one copy drawn twice as
                // often as the rest, which is a weighting bug wearing a
                // symmetry's clothes.
                for (i, a) in els.iter().enumerate() {
                    for (j, b) in els.iter().enumerate() {
                        assert!(i == j || !same(a, b), "{:?}: duplicate elements", kind);
                    }
                }
            }
        }
    }

    /// `C_n` about an arbitrary axis has to be `n`-fold about *that* axis, not
    /// about whatever the generator happened to be written in terms of.
    #[test]
    fn a_cyclic_group_fixes_its_own_axis() {
        let axis = Vec3::new(-0.4, 0.7, 0.6).normalize();
        let g = Symmetry::new(OrbitKind::Cyclic(7), axis, false, OrbitColor::Shared).unwrap();
        for e in g.elements() {
            let moved = e.transform_vector3(axis);
            assert!((moved - axis).length() < 1e-4, "the axis must be fixed");
        }
    }

    /// The orbit is what the ghosts draw and what the walk composes; both want
    /// the identity first so the motif itself is copy 0.
    #[test]
    fn the_orbit_starts_with_the_map_itself() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.5),
            glam::Quat::from_rotation_x(0.3),
            Vec3::new(1.0, 0.0, 0.0),
        );
        let g = sym(OrbitKind::Cyclic(4));
        let orbit: Vec<Mat4> = g.orbit(m).collect();
        assert_eq!(orbit.len(), 4);
        assert!((orbit[0].to_cols_array()[12] - m.to_cols_array()[12]).abs() < 1e-6);
        // And every image is a genuinely different placement.
        assert!((orbit[1].w_axis - orbit[0].w_axis).length() > 1e-3);
    }

    /// The powers chain: copy `k` is the step applied `k` times, so a repeat of
    /// pure translation lays its copies out evenly along the step vector.
    #[test]
    fn a_repeat_walks_its_step_once_per_copy() {
        let r = Repeat { count: 5, translate: Vec3::new(0.0, 0.5, 0.0), turn: 0.0, scale: 1.0 };
        let s = Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).unwrap();
        assert_eq!(s.order(), 5);
        for (k, e) in s.elements().iter().enumerate() {
            let origin = e.transform_point3(Vec3::ZERO);
            assert!(
                (origin.y - 0.5 * k as f32).abs() < 1e-5,
                "copy {} sits at {} not {}",
                k,
                origin.y,
                0.5 * k as f32
            );
        }
    }

    /// A turning repeat whose turn divides 360° revisits its starting angle, and
    /// the accumulated matrix has to land back on it rather than drift there.
    #[test]
    fn a_quarter_turn_repeat_closes_on_the_fourth_copy() {
        let r = Repeat { count: 5, translate: Vec3::ZERO, turn: 90.0, scale: 1.0 };
        let s = Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).unwrap();
        let p = Vec3::new(1.0, 0.0, 0.0);
        let fourth = s.elements()[4].transform_point3(p);
        assert!((fourth - p).length() < 1e-4, "S^4 should be the identity, got {:?}", fourth);
    }

    /// The taper is what the similarity dimension needs, so it has to be exact
    /// rather than merely monotone.
    #[test]
    fn element_scales_are_the_powers_of_the_shrink() {
        let r = Repeat { count: 4, translate: Vec3::Y, turn: 10.0, scale: 0.5 };
        let s = Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).unwrap();
        assert_eq!(s.element_scales(), vec![1.0, 0.5, 0.25, 0.125]);
        // And a group's copies are all the same size, orthogonality being the
        // whole reason `|G|·sᵈ` works for one and not for the other.
        assert_eq!(sym(OrbitKind::Icosahedral).element_scales(), vec![1.0; 60]);
    }

    /// The distinction the type docs turn on, asserted rather than asserted-in-a-
    /// comment: a repeat's element set is not closed under composition.
    #[test]
    fn a_repeat_is_not_a_group() {
        let r = Repeat { count: 4, translate: Vec3::new(0.3, 0.0, 0.0), turn: 0.0, scale: 1.0 };
        let s = Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).unwrap();
        assert!(!s.is_group());

        let last = s.elements()[3];
        let step = s.elements()[1];
        let past_the_end = step * last;
        assert!(
            !s.elements().iter().any(|e| {
                e.to_cols_array()
                    .iter()
                    .zip(past_the_end.to_cols_array().iter())
                    .all(|(a, b)| (a - b).abs() < SAME)
            }),
            "S^4 must fall outside a 4-copy repeat — if it did not, this would be a group"
        );

        // Every named group, by contrast, does contain all its products.
        assert!(sym(OrbitKind::Tetrahedral).is_group());
    }

    /// A step that grows makes the far copies expansive and the chaos game
    /// unbounded, so it is refused at construction rather than rendered.
    #[test]
    fn a_growing_repeat_is_refused() {
        for scale in [1.0001, 2.0, 0.0, -0.5] {
            let r = Repeat { count: 4, translate: Vec3::Y, turn: 0.0, scale };
            assert!(
                Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).is_err(),
                "scale {} should be refused",
                scale
            );
        }
        // The boundary itself is fine: a step that neither grows nor shrinks is
        // a plain row of copies.
        let r = Repeat { count: 4, translate: Vec3::Y, turn: 0.0, scale: 1.0 };
        assert!(Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).is_ok());
    }

    #[test]
    fn a_repeat_count_stays_inside_its_budget() {
        for count in [0, MAX_REPEAT + 1, 10_000] {
            let r = Repeat { count, ..Repeat::default() };
            assert!(
                Symmetry::new(OrbitKind::Repeat(r), Vec3::Y, false, OrbitColor::Shared).is_err(),
                "count {} should be refused",
                count
            );
        }
    }

    #[test]
    fn group_names_round_trip() {
        for text in [
            "Cyc5", "cyc5", "C5", "c5", "C_5", "cyclic5", "Dih3", "dih_3", "D3", "d_3", "Tetra",
            "T", "octa", "o", "Icosa", "icosahedral", "I",
        ] {
            let kind = OrbitKind::parse(text).unwrap_or_else(|e| panic!("{}: {}", text, e));
            // The label always re-parses to the same kind.
            assert_eq!(OrbitKind::parse(&kind.label()).unwrap(), kind, "{}", text);
        }
    }

    /// The abbreviations and the bare mathematical letters name the same groups.
    /// Scene files written before the names were expanded must keep loading, and
    /// an author who types `I` must not get a different picture from `Icosa`.
    #[test]
    fn the_short_and_long_spellings_agree() {
        for (short, long) in [
            ("C5", "Cyc5"),
            ("D2", "Dih2"),
            ("T", "Tetra"),
            ("O", "Octa"),
            ("I", "Icosa"),
        ] {
            assert_eq!(
                OrbitKind::parse(short).unwrap(),
                OrbitKind::parse(long).unwrap(),
                "{} vs {}",
                short,
                long
            );
        }
    }

    #[test]
    fn bad_group_names_say_what_is_allowed() {
        for text in [
            "", "C", "C0", "D1", "C1000", "hexagonal", "5", "cyc", "cyclic", "dih", "tetr",
        ] {
            let err = OrbitKind::parse(text).unwrap_err();
            assert!(!err.is_empty(), "{} should be rejected", text);
        }
    }

    #[test]
    fn a_zero_axis_is_an_error_not_a_nan() {
        assert!(Symmetry::new(OrbitKind::Cyclic(3), Vec3::ZERO, false, OrbitColor::Shared).is_err());
        // ...but a polyhedral group doesn't use the axis, so it doesn't care.
        assert!(Symmetry::new(OrbitKind::Octahedral, Vec3::ZERO, false, OrbitColor::Shared).is_ok());
    }

    #[test]
    fn equality_ignores_the_generated_elements() {
        let a = Symmetry::new(OrbitKind::Cyclic(5), Vec3::Y, false, OrbitColor::Shared).unwrap();
        let b = Symmetry::new(OrbitKind::Cyclic(5), Vec3::Y * 3.0, false, OrbitColor::Shared)
            .unwrap();
        assert_eq!(a, b, "the axis is normalized, so these are the same group");
        let recoloured =
            Symmetry::new(OrbitKind::Cyclic(5), Vec3::Y, false, OrbitColor::Orbit).unwrap();
        assert_ne!(a, recoloured, "colour is part of what a symmetry is");
    }
}
