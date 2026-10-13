//! Xaos: which map may follow which.
//!
//! An ordinary chaos game draws every step's map from the same distribution:
//! map `j` with probability `wⱼ / Σw`, whatever happened last. Xaos (Apophysis's
//! name, after flam3's `chaos` attribute) makes the draw depend on the map that
//! ran *last*. Each map `i` carries a row of multipliers `xᵢⱼ`, and the step
//! after `i` picks `j` with probability
//!
//! ```text
//!     P(j | i) = wⱼ·xᵢⱼ / Σₖ wₖ·xᵢₖ          (w = effective weight, 0 if disabled)
//! ```
//!
//! so `1` is "as usual", `0` is "never straight after me", `3` is "three times
//! as likely as usual". The walk becomes a Markov chain over the maps, and the
//! scene a **graph-directed IFS**. `XAOS-PLAN.md` is the design; the four facts
//! that drive it:
//!
//! 1. **Only zeros change the shape.** Which map *sequences* are allowed is the
//!    zero pattern of `x`; a positive multiplier only moves density, like a
//!    weight. A cut is a structural edit, a scale is a density edit.
//! 2. **Weights stop meaning "how often".** How often a map fires is the chain's
//!    stationary distribution — the *walk share* ([`Chain::shares`]) — and a
//!    weight-1 map can be starved to nothing.
//! 3. **Dimension changes formula**: Mauldin–Williams ([`Chain::dimension`]),
//!    which again depends only on the zero pattern.
//! 4. **Disconnected parts are layers.** Xaos stays inside a layer; a subscene's
//!    walkers never draw another layer's maps.
//!
//! ## A row is a rest value plus exceptions
//!
//! [`Xaos`] stores `rest` — the multiplier for every target not listed, `"*"`
//! in the file, 1 by default — and the targets whose multiplier differs from
//! it. Nothing is ever N², so a 40k-map L-system port can carry "no self-loops"
//! (40k one-entry rows) and "cycle" (40k `rest = 0` rows of one entry) at the
//! cost of the exceptions alone. A default row is no row at all, and an
//! ordinary scene renders byte for byte as it did before xaos existed.
//!
//! Targets are positions **within the map's layer**. Adding a map appends a
//! position every row covers with its `rest`; removing or inserting one has to
//! renumber the exceptions ([`remove_column`], [`insert_column`]), and
//! duplicating one copies its row and its column ([`copy_column`]).
//!
//! ## Sampling, exactly, in two binary searches
//!
//! The base CDF is the layer's normalized cumulative weights — the array the
//! ordinary walk already searches. With `bⱼ` the base mass of `j`, `E` the
//! exceptions' base mass and `X = Σ_exc bⱼ·xᵢⱼ`, a row's total mass is
//! `c·(1 − E) + X`. Draw `r` in `[0, total)`:
//!
//! * `r < X`: search the exceptions' own CDF;
//! * otherwise `s = (r − X)/c` is a position in the base CDF with the
//!   exceptions' intervals squeezed out. Each exception's gap sits at a known
//!   point `g` of that squeezed axis; adding back the mass of the gaps at or
//!   before `s` gives a base coordinate, and the ordinary `select_transform`
//!   finishes the job.
//!
//! With no exceptions and `c = 1` every step of that is exact in f32 — the
//! total is `1`, `s = r`, nothing is added — so a scene whose rows are all
//! `{ "*" = 1 }` runs the whole new path and still renders byte-identically.
//! `FRACTURIZE_XAOS_FORCE=1` forces that path on for every layer, which is how
//! the claim is tested against real scenes.
//!
//! The table is built once on the CPU ([`build_rows`]) and read by both the GPU
//! (`chaos.wgsl`'s `select_xaos`) and the CPU walkers ([`sample`]), which is a
//! line-for-line transcription of the shader. One builder, two readers: they
//! cannot disagree about the distribution.
//!
//! ## Edges of the model
//!
//! * The first step, and the step after a re-seed, have no previous map and
//!   draw from the plain weights.
//! * **A dead end** — a row whose mass is zero — restarts from the plain
//!   weights, and is *reported* (`--info`, the Xaos block), never refused: random
//!   generation and live edits both reach one constantly, and a walk that froze
//!   would look like a renderer bug.
//! * **Symmetry is untouched.** Xaos is over motifs; the group element is still
//!   drawn independently after the map, so exact symmetry survives.
//! * **Infinite zoom needs `f_z(S) ⊆ S`**, which holds only if every map that
//!   fires may be followed by the zoom map ([`zoom_conflict`]).

use crate::scene::TransformSpec;

/// `GpuTransform::xaos_start` for a map with no row: select by plain weight.
pub const NO_ROW: u32 = u32::MAX;

/// The largest f32 below 1. A uniform draw can round up to exactly 1.0, and
/// the exceptions search must not run off its row.
const BELOW_ONE: f32 = 0.99999994;

/// One map's outgoing row: `rest` for every target not listed, and the targets
/// whose multiplier differs from it, sorted by target with none equal to `rest`.
#[derive(Clone, Debug, PartialEq)]
pub struct Xaos {
    pub rest: f32,
    pub exceptions: Vec<(u32, f32)>,
}

impl Default for Xaos {
    fn default() -> Self {
        Self { rest: 1.0, exceptions: Vec::new() }
    }
}

impl Xaos {
    /// Whether this row says nothing: every target at 1.
    pub fn is_default(&self) -> bool {
        self.rest == 1.0 && self.exceptions.is_empty()
    }

    /// The multiplier toward the map at layer position `j`.
    pub fn get(&self, j: usize) -> f32 {
        match self.exceptions.binary_search_by_key(&(j as u32), |e| e.0) {
            Ok(k) => self.exceptions[k].1,
            Err(_) => self.rest,
        }
    }

    /// Set one edge, keeping the invariants (sorted, nothing equal to rest).
    pub fn set(&mut self, j: usize, v: f32) {
        let j = j as u32;
        match self.exceptions.binary_search_by_key(&j, |e| e.0) {
            Ok(k) if v == self.rest => {
                self.exceptions.remove(k);
            }
            Ok(k) => self.exceptions[k].1 = v,
            Err(_) if v == self.rest => {}
            Err(k) => self.exceptions.insert(k, (j, v)),
        }
    }

    /// Change the rest of the row. Listed targets keep their values (one that
    /// now equals the rest stops being an exception); unlisted ones follow.
    pub fn set_rest(&mut self, c: f32) {
        self.rest = c;
        self.exceptions.retain(|e| e.1 != c);
    }

    /// Every target from `0..n` with its multiplier — for display.
    pub fn dense(&self, n: usize) -> Vec<f32> {
        let mut out = vec![self.rest; n];
        for &(j, v) in &self.exceptions {
            if let Some(slot) = out.get_mut(j as usize) {
                *slot = v;
            }
        }
        out
    }

    /// How many of `0..n` this row cuts.
    pub fn cut_count(&self, n: usize) -> usize {
        let cut_exc = self.exceptions.iter().filter(|e| e.1 == 0.0 && (e.0 as usize) < n).count();
        if self.rest == 0.0 {
            n - self.exceptions.iter().filter(|e| e.1 != 0.0 && (e.0 as usize) < n).count()
        } else {
            cut_exc
        }
    }
}

/// Whether any map of this layer weights its transitions.
pub fn layer_has_xaos(maps: &[TransformSpec]) -> bool {
    maps.iter().any(|t| !t.xaos.is_default())
}

/// `FRACTURIZE_XAOS_FORCE=1`: run every layer through the xaos path even with
/// no rows authored. A test lever (see the module doc), read once.
pub fn forced() -> bool {
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| std::env::var("FRACTURIZE_XAOS_FORCE").is_ok_and(|v| v == "1"))
}

// ---------------------------------------------------------------------------
// Structural edits: keep every row pointing at the same maps.
// ---------------------------------------------------------------------------

/// Renumber one row for a map arriving at layer position `pos`.
pub fn shift_row(row: &mut Xaos, pos: usize) {
    for e in &mut row.exceptions {
        if e.0 >= pos as u32 {
            e.0 += 1;
        }
    }
}

/// The layer lost its map at position `pos`: drop edges into it and renumber
/// the ones past it.
pub fn remove_column(transforms: &mut [TransformSpec], layer: u16, pos: usize) {
    let pos = pos as u32;
    for t in transforms.iter_mut().filter(|t| t.layer == layer) {
        t.xaos.exceptions.retain(|e| e.0 != pos);
        for e in &mut t.xaos.exceptions {
            if e.0 > pos {
                e.0 -= 1;
            }
        }
    }
}

/// A map is about to occupy layer position `pos`, pushing the rest along.
/// Every row then covers it with its `rest`, as for an appended map.
pub fn insert_column(transforms: &mut [TransformSpec], layer: u16, pos: usize) {
    for t in transforms.iter_mut().filter(|t| t.layer == layer) {
        shift_row(&mut t.xaos, pos);
    }
}

/// Map `dst` (already in place, with `src`'s row cloned onto it) is a copy of
/// `src`: everything that could go to `src` may go to the copy, at the same
/// odds. With the row already cloned this also gives the copy `src`'s
/// self-transition toward both itself and the original — a duplicate behaves
/// as a second copy of the same map.
pub fn copy_column(transforms: &mut [TransformSpec], layer: u16, src: usize, dst: usize) {
    for t in transforms.iter_mut().filter(|t| t.layer == layer) {
        let v = t.xaos.get(src);
        t.xaos.set(dst, v);
    }
}

// ---------------------------------------------------------------------------
// Presets: operations that write rows (a pasted row is just a row).
// ---------------------------------------------------------------------------

/// The one-click patterns. Each *writes* rows; none is a live rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Preset {
    /// Clear every row back to "anything may follow".
    Reset,
    /// No map may follow itself.
    NoSelfLoops,
    /// Each map may only be followed by the next one, in list order, wrapping.
    Cycle,
    /// `n` contiguous groups in list order, each closed to the others.
    Blocks(usize),
}

impl Preset {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "reset" => Ok(Self::Reset),
            "no-self-loops" => Ok(Self::NoSelfLoops),
            "cycle" => Ok(Self::Cycle),
            other => match other.strip_prefix("blocks:") {
                Some(n) => match n.parse::<usize>() {
                    Ok(n) if n >= 2 => Ok(Self::Blocks(n)),
                    _ => Err(format!("'{other}': blocks:N needs N of 2 or more")),
                },
                None => Err(format!(
                    "unknown xaos preset '{other}' (expected reset, no-self-loops, cycle or blocks:N)"
                )),
            },
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::Reset => "reset".into(),
            Self::NoSelfLoops => "no-self-loops".into(),
            Self::Cycle => "cycle".into(),
            Self::Blocks(n) => format!("blocks:{n}"),
        }
    }

    /// The rows this preset gives a layer of `n` maps.
    pub fn rows(self, n: usize) -> Vec<Xaos> {
        (0..n)
            .map(|i| {
                let mut x = Xaos::default();
                match self {
                    Self::Reset => {}
                    Self::NoSelfLoops => x.set(i, 0.0),
                    Self::Cycle => {
                        x.set_rest(0.0);
                        x.set((i + 1) % n, 1.0);
                    }
                    Self::Blocks(k) => {
                        let k = k.min(n).max(1);
                        let block = |m: usize| m * k / n;
                        x.set_rest(0.0);
                        for j in (0..n).filter(|&j| block(j) == block(i)) {
                            x.set(j, 1.0);
                        }
                    }
                }
                x
            })
            .collect()
    }
}

/// Apply a preset to every layer of a transform list.
pub fn apply_preset(transforms: &mut [TransformSpec], preset: Preset) {
    let layers: std::collections::BTreeSet<u16> = transforms.iter().map(|t| t.layer).collect();
    for layer in layers {
        let idx: Vec<usize> = (0..transforms.len()).filter(|&i| transforms[i].layer == layer).collect();
        for (row, &i) in preset.rows(idx.len()).into_iter().zip(&idx) {
            transforms[i].xaos = row;
        }
    }
}

// ---------------------------------------------------------------------------
// The chain, for analysis: walk shares, dead ends, dimension.
// ---------------------------------------------------------------------------

/// Effective plain weights: the authored weight, or 0 for a disabled map.
fn weights(maps: &[TransformSpec], enabled: &[bool]) -> Vec<f64> {
    maps.iter()
        .enumerate()
        .map(|(i, t)| {
            if enabled.get(i).copied().unwrap_or(true) { t.weight.max(0.0) as f64 } else { 0.0 }
        })
        .collect()
}

struct ChainRow {
    rest: f64,
    exceptions: Vec<(usize, f64)>,
    /// `Σⱼ bⱼ·xᵢⱼ`; zero for a dead row.
    total: f64,
}

/// The walk as a Markov chain over one layer's maps, held sparse: one rest
/// value and a list of exceptions per row, never an N×N matrix.
pub struct Chain {
    /// Plain weights, normalized: where a walker with no history starts.
    pub start: Vec<f64>,
    rows: Vec<ChainRow>,
    /// Maps that can fire but whose row leaves nowhere to go. They restart
    /// from `start`.
    pub dead: Vec<usize>,
}

impl Chain {
    /// `None` when no map has any weight — the same `None` the CPU walker
    /// gives for an unwalkable scene.
    pub fn build(maps: &[TransformSpec], enabled: &[bool]) -> Option<Self> {
        let w = weights(maps, enabled);
        let total: f64 = w.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let start: Vec<f64> = w.iter().map(|v| v / total).collect();
        let n = maps.len();
        let mut dead = Vec::new();
        let rows = maps
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let exceptions: Vec<(usize, f64)> = t
                    .xaos
                    .exceptions
                    .iter()
                    .filter(|e| (e.0 as usize) < n)
                    .map(|e| (e.0 as usize, e.1.max(0.0) as f64))
                    .collect();
                let rest = t.xaos.rest.max(0.0) as f64;
                let exc_base: f64 = exceptions.iter().map(|&(j, _)| start[j]).sum();
                let exc_mass: f64 = exceptions.iter().map(|&(j, x)| start[j] * x).sum();
                let row_total = rest * (1.0 - exc_base).max(0.0) + exc_mass;
                let row_total = if row_total > 1e-15 { row_total } else { 0.0 };
                if row_total == 0.0 && start[i] > 0.0 {
                    dead.push(i);
                }
                ChainRow { rest, exceptions, total: row_total }
            })
            .collect();
        Some(Self { start, rows, dead })
    }

    pub fn len(&self) -> usize {
        self.start.len()
    }

    /// `P(j | i)`.
    pub fn prob(&self, i: usize, j: usize) -> f64 {
        let row = &self.rows[i];
        if row.total == 0.0 {
            return self.start[j];
        }
        let x = match row.exceptions.binary_search_by_key(&j, |e| e.0) {
            Ok(k) => row.exceptions[k].1,
            Err(_) => row.rest,
        };
        self.start[j] * x / row.total
    }

    /// Whether `j` may follow `i` at all (non-zero probability).
    pub fn allowed(&self, i: usize, j: usize) -> bool {
        self.prob(i, j) > 0.0
    }

    /// One step of the lazy chain `π ← (π + πP)/2`, in O(N + exceptions).
    fn lazy_step(&self, pi: &[f64], next: &mut [f64]) {
        // Mass every row sends "at the rest rate", per unit of base mass.
        let mut spread = 0.0;
        for (i, row) in self.rows.iter().enumerate() {
            if pi[i] == 0.0 {
                continue;
            }
            spread += if row.total == 0.0 { pi[i] } else { pi[i] * row.rest / row.total };
        }
        for j in 0..pi.len() {
            next[j] = 0.5 * pi[j] + 0.5 * spread * self.start[j];
        }
        for (i, row) in self.rows.iter().enumerate() {
            if pi[i] == 0.0 || row.total == 0.0 {
                continue;
            }
            let k = 0.5 * pi[i] / row.total;
            for &(j, x) in &row.exceptions {
                next[j] += k * (x - row.rest) * self.start[j];
            }
        }
    }

    /// The share of points each map places in the long run, from a walker
    /// that starts with no history — what the picture is made of — and whether
    /// the iteration settled.
    ///
    /// For a scene without xaos this is exactly the normalized weights. Under
    /// xaos it can be anything: a map behind a cut can be starved entirely, and
    /// a chain with several closed classes splits its walkers between them in
    /// proportion to how often a fresh walker falls into each. Iterated on the
    /// *lazy* chain `(P + I)/2`, which has the same stationary distributions
    /// and absorption probabilities but no periodicity, so plain iteration
    /// converges instead of oscillating around a cycle like `a → b → a`.
    pub fn shares_and_settled(&self) -> (Vec<f64>, bool) {
        let n = self.len();
        let mut pi = self.start.clone();
        let mut next = vec![0.0; n];
        for _ in 0..20_000 {
            self.lazy_step(&pi, &mut next);
            let change: f64 = pi.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut pi, &mut next);
            if change < 1e-12 {
                return (pi, true);
            }
        }
        (pi, false)
    }

    pub fn shares(&self) -> Vec<f64> {
        self.shares_and_settled().0
    }

    /// Mauldin–Williams dimension: the `d` at which the spectral radius of
    /// `M(d)ᵢⱼ = [i → j allowed] · Σₑ (sⱼ·e)ᵈ` is 1, over the maps the walk
    /// visits in the long run (a transient map draws a handful of points and no
    /// structure). `contractions[j]` is map `j`'s per-copy contractions — one
    /// per symmetry element — and empty for a map that doesn't contract, which
    /// then adds nothing, as in the ordinary sum.
    ///
    /// Only *which* transitions are allowed enters, never their odds: a
    /// positive multiplier deals density and cannot add or remove material.
    /// With every transition allowed `M` has rank one and radius `Σⱼ cⱼ(d)`, so
    /// this reduces to the ordinary `Σ sᵈ = 1`.
    pub fn dimension(&self, contractions: &[Vec<f32>]) -> Option<f32> {
        let shares = self.shares();
        let n = self.len();
        let live: Vec<bool> = (0..n).map(|i| shares[i] > 1e-7).collect();
        if (0..n).all(|j| !live[j] || contractions[j].is_empty()) {
            return None;
        }

        // y = M v, sparse: a row with rest > 0 reaches every live target but
        // its zero exceptions; a row with rest 0 reaches only its positive ones.
        let matvec = |k: &[f64], v: &[f64], y: &mut [f64]| {
            let z: Vec<f64> = (0..n)
                .map(|j| if live[j] && self.start[j] > 0.0 { k[j] * v[j] } else { 0.0 })
                .collect();
            let all: f64 = z.iter().sum();
            for i in 0..n {
                if !live[i] {
                    y[i] = 0.0;
                    continue;
                }
                let row = &self.rows[i];
                y[i] = if row.total == 0.0 {
                    all
                } else if row.rest > 0.0 {
                    all - row.exceptions.iter().filter(|e| e.1 == 0.0).map(|e| z[e.0]).sum::<f64>()
                } else {
                    row.exceptions.iter().filter(|e| e.1 > 0.0).map(|e| z[e.0]).sum()
                };
            }
        };

        let mut x = vec![1.0f64; n];
        let mut radius = |d: f32| -> f64 {
            let k: Vec<f64> = contractions
                .iter()
                .map(|cs| cs.iter().map(|&s| (s as f64).powf(d as f64)).sum())
                .collect();
            spectral_radius(n, |v, y| matvec(&k, v, y), &mut x)
        };

        let (mut lo, mut hi) = (0.0f32, 10.0f32);
        if radius(lo) < 1.0 {
            return Some(0.0);
        }
        while radius(hi) > 1.0 && hi < 100.0 {
            hi *= 2.0;
        }
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if radius(mid) > 1.0 { lo = mid } else { hi = mid }
        }
        Some(0.5 * (lo + hi))
    }
}

/// Perron root of a non-negative operator, by power iteration on `M + I` (same
/// Perron vector, root shifted by one, no periodicity to stall on). `x` is a
/// warm start and is left holding the eigenvector, which is what makes a
/// bisection over `d` cheap: neighbouring `d`s have nearly the same one.
fn spectral_radius(n: usize, m: impl Fn(&[f64], &mut [f64]), x: &mut Vec<f64>) -> f64 {
    let mut y = vec![0.0; n];
    let mut estimate = 0.0;
    for it in 0..400 {
        m(x, &mut y);
        for i in 0..n {
            y[i] += x[i];
        }
        let norm: f64 = y.iter().sum();
        let prev: f64 = x.iter().sum();
        if norm <= 0.0 || prev <= 0.0 {
            return 0.0;
        }
        let next = norm / prev - 1.0;
        for i in 0..n {
            x[i] = y[i] / norm;
        }
        if it > 8 && (next - estimate).abs() < 1e-10 * next.abs().max(1.0) {
            return next;
        }
        estimate = next;
    }
    estimate
}

/// Similarity dimension of one layer under xaos, cached: the status bar asks
/// every frame, and a bisection over a power iteration is not free.
pub fn dimension_cached(maps: &[TransformSpec], weights: Option<&[f32]>) -> Option<f32> {
    use std::hash::{Hash, Hasher};
    let enabled: Vec<bool> = match weights {
        Some(ws) => (0..maps.len()).map(|i| ws.get(i).copied().unwrap_or(0.0) > 0.0).collect(),
        None => Vec::new(),
    };
    let contractions: Vec<Vec<f32>> = maps.iter().map(crate::scene::copy_contractions).collect();

    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (i, t) in maps.iter().enumerate() {
        t.weight.to_bits().hash(&mut h);
        enabled.get(i).hash(&mut h);
        t.xaos.rest.to_bits().hash(&mut h);
        for e in &t.xaos.exceptions {
            (e.0, e.1.to_bits()).hash(&mut h);
        }
        for c in &contractions[i] {
            c.to_bits().hash(&mut h);
        }
    }
    let key = h.finish();
    thread_local! {
        static LAST: std::cell::Cell<Option<(u64, Option<f32>)>> = const { std::cell::Cell::new(None) };
    }
    if let Some((k, d)) = LAST.with(|l| l.get()) {
        if k == key {
            return d;
        }
    }
    let d = Chain::build(maps, &enabled).and_then(|c| c.dimension(&contractions));
    LAST.with(|l| l.set(Some((key, d))));
    d
}

/// Each of a layer's maps as scene indices, and the layer's chain — `None`
/// when the layer has no xaos (its weights already say everything) or nothing
/// to walk.
pub fn layer_chain(scene: &crate::scene::Scene, k: usize) -> Option<(Vec<usize>, Chain)> {
    let idx: Vec<usize> =
        (0..scene.transforms.len()).filter(|&i| scene.transforms[i].layer as usize == k).collect();
    let maps: Vec<TransformSpec> = idx.iter().map(|&i| scene.transforms[i].clone()).collect();
    if !layer_has_xaos(&maps) {
        return None;
    }
    Chain::build(&maps, &[]).map(|c| (idx, c))
}

/// Per scene transform, its share of its own layer's walk under xaos — the
/// chain's stationary distribution — or `None` in a layer without xaos, where
/// the weights already say it.
pub fn scene_walk_shares(scene: &crate::scene::Scene) -> Vec<Option<f64>> {
    let mut out = vec![None; scene.transforms.len()];
    for k in 0..scene.layer_count() {
        if let Some((idx, chain)) = layer_chain(scene, k) {
            for (n, share) in chain.shares().into_iter().enumerate() {
                out[idx[n]] = Some(share);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The sampling table: built once, read by the GPU and the CPU walkers.
// ---------------------------------------------------------------------------

/// One map's row as the GPU holds it, in `vec4`s.
///
/// `[0]` is the header `(c, X, total, last positive exception as bits)`; then
/// one entry per exception, sorted by target:
/// `(target as bits, cumulative x-mass, gap position, cumulative base mass)`.
pub type Row = Vec<[f32; 4]>;

/// Rows for one layer, from its maps and the normalized cumulative base
/// weights the GPU uploads for it (`cum[j]` covers maps `0..=j`). `None` for a
/// map that selects by plain weight: no row authored (unless `force`), or a
/// dead end, which restarts from the plain weights.
pub fn build_rows(maps: &[TransformSpec], cum: &[f32], force: bool) -> Vec<Option<Row>> {
    let n = maps.len();
    let base = |j: usize| -> f64 {
        let hi = cum[j] as f64;
        let lo = if j == 0 { 0.0 } else { cum[j - 1] as f64 };
        (hi - lo).max(0.0)
    };
    let before = |j: usize| -> f64 { if j == 0 { 0.0 } else { cum[j - 1] as f64 } };
    maps.iter()
        .map(|t| {
            if t.xaos.is_default() && !force {
                return None;
            }
            let exceptions: Vec<(usize, f64)> = t
                .xaos
                .exceptions
                .iter()
                .filter(|e| (e.0 as usize) < n)
                .map(|e| (e.0 as usize, e.1.max(0.0) as f64))
                .collect();
            let exc_base: f64 = exceptions.iter().map(|&(j, _)| base(j)).sum();
            // Literally 1 - E, not cum[last] - E: with no exceptions the total
            // then comes out as exactly 1.0 and the draw is used unscaled,
            // which is the byte-identity the module doc promises.
            let rest_mass = 1.0 - exc_base;
            let mut c = t.xaos.rest.max(0.0) as f64;
            if rest_mass <= 1e-12 {
                c = 0.0;
            }
            let mut entries = Vec::with_capacity(exceptions.len());
            let (mut cum_x, mut cum_b, mut last_pos) = (0.0f64, 0.0f64, 0u32);
            for (k, &(j, x)) in exceptions.iter().enumerate() {
                let m = base(j) * x;
                let gap = before(j) - cum_b;
                cum_x += m;
                cum_b += base(j);
                if m > 0.0 {
                    last_pos = k as u32;
                }
                entries.push([f32::from_bits(j as u32), cum_x as f32, gap as f32, cum_b as f32]);
            }
            let x_total = cum_x;
            let total = c * rest_mass + x_total;
            if !(total > 1e-15) {
                return None;
            }
            // Take X from the stored cumulative itself, so `r < X` and the
            // search that follows it agree to the last bit.
            let x_f32 = entries.last().map_or(0.0, |e| e[1]);
            let mut row = Vec::with_capacity(entries.len() + 1);
            row.push([c as f32, x_f32, total as f32, f32::from_bits(last_pos)]);
            row.extend(entries);
            Some(row)
        })
        .collect()
}

/// First `i` with `u < cum[i]`, the last index if none — `select_transform`.
pub fn select_base(cum: &[f32], u: f32) -> usize {
    cum.iter().position(|&c| u < c).unwrap_or(cum.len().saturating_sub(1))
}

/// Draw the next map from a row, given a uniform `rand` in `[0, 1]`. A
/// line-for-line transcription of `select_xaos` in `chaos.wgsl`, in the same
/// f32 arithmetic: keep the two in step.
pub fn sample(row: &[[f32; 4]], cum: &[f32], rand: f32) -> usize {
    let h = row[0];
    let (c, x_total, total) = (h[0], h[1], h[2]);
    let entries = &row[1..];
    let len = entries.len();
    let r = rand.min(BELOW_ONE) * total;
    if r < x_total || c <= 0.0 {
        let (mut lo, mut hi) = (0usize, len - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if r < entries[mid][1] { hi = mid } else { lo = mid + 1 }
        }
        if !(r < entries[lo][1]) {
            lo = h[3].to_bits() as usize;
        }
        return entries[lo][0].to_bits() as usize;
    }
    let s = (r - x_total) / c;
    // How many gaps sit at or before s.
    let (mut lo, mut hi) = (0usize, len);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if s < entries[mid][2] { hi = mid } else { lo = mid + 1 }
    }
    let mut u = s;
    if lo > 0 {
        u = s + entries[lo - 1][3];
    }
    let mut idx = select_base(cum, u);
    // Rounding can leave u a hair inside a squeezed-out interval. Step past it
    // rather than draw an exception at the rest's odds — a cut edge is never
    // drawn, not almost never.
    for _ in 0..len {
        if u >= 1.0 || !is_exception(entries, idx as u32) {
            break;
        }
        u = cum[idx];
        idx = select_base(cum, u);
    }
    idx
}

fn is_exception(entries: &[[f32; 4]], target: u32) -> bool {
    entries.binary_search_by_key(&target, |e| e[0].to_bits()).is_ok()
}

// ---------------------------------------------------------------------------
// Checks and solving.
// ---------------------------------------------------------------------------

/// Values must be finite and non-negative: they are relative odds.
pub fn check_value(v: f64) -> Result<(), String> {
    if v.is_finite() && v >= 0.0 {
        Ok(())
    } else {
        Err(format!("xaos multiplier {v} is not allowed: they are relative odds, 0 (never) or more"))
    }
}

/// Infinite zoom about map `z` needs `f_z(S) ⊆ S`, which holds only when every
/// map that fires may be followed by `z`. The first edge that breaks it, as a
/// sentence naming both maps.
pub fn zoom_conflict(maps: &[TransformSpec], names: &[Option<String>], z: usize) -> Option<String> {
    if !layer_has_xaos(maps) || z >= maps.len() {
        return None;
    }
    let chain = Chain::build(maps, &[])?;
    let shares = chain.shares();
    let label = |i: usize| match names.get(i).and_then(|n| n.as_deref()) {
        Some(n) => format!("'{n}'"),
        None => format!("#{i}"),
    };
    (0..maps.len()).find(|&i| shares[i] > 1e-7 && !chain.allowed(i, z)).map(|i| {
        format!(
            "infinite zoom about {} needs every map that fires to allow {} next, \
             and {} cuts it (xaos). Restore that edge, or zoom about another map",
            label(z),
            label(z),
            label(i)
        )
    })
}

/// The multiplier that makes `P(to | from) = p` with the rest of `from`'s row
/// held: `x = p·Σ_{k≠to} wₖxₖ / ((1−p)·w_to)`. `None` when no multiplier can:
/// `to` has no weight, or every other exit is already cut and `p < 1`.
/// `p = 1` is "only this exit" and is returned as `f32::INFINITY` for the
/// caller to write as a `rest = 0` row instead.
pub fn multiplier_for(maps: &[TransformSpec], from: usize, to: usize, p: f64) -> Option<f32> {
    let w_to = maps.get(to)?.weight.max(0.0) as f64;
    if !(0.0..=1.0).contains(&p) || w_to <= 0.0 {
        return None;
    }
    if p >= 1.0 {
        return Some(f32::INFINITY);
    }
    let row = &maps[from].xaos;
    let others: f64 = maps
        .iter()
        .enumerate()
        .filter(|&(k, _)| k != to)
        .map(|(k, m)| m.weight.max(0.0) as f64 * row.get(k).max(0.0) as f64)
        .sum();
    if p > 0.0 && others <= 0.0 {
        return None;
    }
    Some((p * others / ((1.0 - p) * w_to)) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3};
    use rand::{Rng, SeedableRng};

    fn map(scale: f32, at: Vec3) -> TransformSpec {
        TransformSpec {
            matrix: Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(scale)),
            post_affine: Mat4::IDENTITY,
            color_value: 0.0,
            weight: 1.0,
            color_speed: 0.5,
            explicit_color_speed: None,
            variations: TransformSpec::linear_variations(),
            symmetry: None,
            layer: 0,
            xaos: Xaos::default(),
        }
    }

    fn maps(n: usize) -> Vec<TransformSpec> {
        (0..n).map(|k| map(0.5, Vec3::X * k as f32)).collect()
    }

    fn cum_of(maps: &[TransformSpec]) -> Vec<f32> {
        let total: f32 = maps.iter().map(|m| m.weight).sum();
        let mut acc = 0.0f32;
        maps.iter().map(|m| { acc += m.weight / total; acc }).collect()
    }

    fn contractions(maps: &[TransformSpec]) -> Vec<Vec<f32>> {
        maps.iter().map(crate::scene::copy_contractions).collect()
    }

    /// The empirical P(j|i) over many draws matches the formula, for rows that
    /// exercise both branches and every edge of the gap arithmetic.
    #[test]
    fn the_sampler_draws_the_formula() {
        let mut ms = maps(6);
        ms[1].weight = 0.0; // a zero-mass target sitting among exceptions
        ms[3].weight = 2.5;
        ms[5].weight = 0.7;
        let rows: Vec<Xaos> = vec![
            Xaos { rest: 1.0, exceptions: vec![(0, 0.0)] },           // cut at the start
            Xaos { rest: 1.0, exceptions: vec![(5, 0.0)] },           // cut at the end
            Xaos { rest: 1.0, exceptions: vec![(2, 0.0), (3, 0.0)] }, // adjacent cuts
            Xaos { rest: 0.0, exceptions: vec![(2, 1.0), (4, 3.0)] }, // c = 0
            Xaos { rest: 2.0, exceptions: vec![(0, 0.5), (1, 9.0), (4, 0.0)] }, // c != 1
            Xaos { rest: 0.3, exceptions: vec![] },                   // c != 1, nothing listed
        ];
        for (i, r) in rows.into_iter().enumerate() {
            ms[i].xaos = r;
        }
        let cum = cum_of(&ms);
        let table = build_rows(&ms, &cum, false);
        let chain = Chain::build(&ms, &[]).unwrap();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        const N: usize = 400_000;
        for i in 0..ms.len() {
            let row = table[i].as_ref().expect("every row is authored");
            let mut counts = [0usize; 6];
            for _ in 0..N {
                counts[sample(row, &cum, rng.r#gen())] += 1;
            }
            for j in 0..6 {
                let want = chain.prob(i, j);
                let got = counts[j] as f64 / N as f64;
                if want == 0.0 {
                    assert_eq!(counts[j], 0, "row {i}: forbidden {j} drawn");
                } else {
                    assert!((got - want).abs() < 0.005, "row {i} -> {j}: {got} vs {want}");
                }
            }
        }
    }

    #[test]
    fn the_extreme_draws_stay_on_allowed_maps() {
        let mut ms = maps(4);
        ms[0].xaos = Xaos { rest: 1.0, exceptions: vec![(3, 0.0)] };
        ms[1].xaos = Xaos { rest: 0.0, exceptions: vec![(0, 1.0), (3, 0.0)] };
        let cum = cum_of(&ms);
        let t = build_rows(&ms, &cum, false);
        for rand in [0.0f32, 1e-9, 0.5, BELOW_ONE, 1.0] {
            assert_ne!(sample(t[0].as_ref().unwrap(), &cum, rand), 3);
            assert_eq!(sample(t[1].as_ref().unwrap(), &cum, rand), 0);
        }
    }

    /// The byte-identity of `{ "*" = 1 }`, at the level of single draws: the
    /// forced path returns exactly what the plain search does.
    #[test]
    fn a_forced_default_row_draws_exactly_the_plain_map() {
        let mut ms = maps(5);
        ms[2].weight = 3.0;
        let cum = cum_of(&ms);
        let t = build_rows(&ms, &cum, true);
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        for _ in 0..100_000 {
            let r: f32 = rng.r#gen();
            assert_eq!(sample(t[0].as_ref().unwrap(), &cum, r), select_base(&cum, r));
        }
        assert_eq!(t[0].as_ref().unwrap()[0], [1.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn default_rows_are_no_rows() {
        let ms = maps(3);
        assert!(build_rows(&ms, &cum_of(&ms), false).iter().all(Option::is_none));
    }

    #[test]
    fn an_ordinary_chain_is_the_plain_weights() {
        let mut ms = maps(2);
        ms[1].weight = 3.0;
        let c = Chain::build(&ms, &[]).unwrap();
        assert_eq!((c.prob(0, 1), c.prob(1, 0)), (0.75, 0.25));
        let s = c.shares();
        assert!((s[0] - 0.25).abs() < 1e-9 && (s[1] - 0.75).abs() < 1e-9);
    }

    #[test]
    fn shares_follow_the_chain_not_the_weights() {
        // a -> b always, b -> a or b evenly: stationary (1/3, 2/3).
        let mut ms = maps(2);
        ms[0].xaos.set(0, 0.0);
        let s = Chain::build(&ms, &[]).unwrap().shares();
        assert!((s[0] - 1.0 / 3.0).abs() < 1e-6, "{s:?}");
    }

    #[test]
    fn a_periodic_chain_still_settles() {
        let mut ms = maps(2);
        apply_preset(&mut ms, Preset::Cycle);
        let (s, settled) = Chain::build(&ms, &[]).unwrap().shares_and_settled();
        assert!(settled && (s[0] - 0.5).abs() < 1e-6, "{s:?}");
    }

    #[test]
    fn an_absorbing_class_drains_the_rest() {
        // 0 -> anything; 1 and 2 only reach each other.
        let mut ms = maps(3);
        for i in [1, 2] {
            ms[i].xaos.set(0, 0.0);
        }
        let s = Chain::build(&ms, &[]).unwrap().shares();
        assert!(s[0] < 1e-9, "{s:?}");
    }

    #[test]
    fn a_dead_row_falls_back_and_is_reported() {
        let mut ms = maps(2);
        ms[0].xaos.set_rest(0.0);
        let c = Chain::build(&ms, &[]).unwrap();
        assert_eq!(c.dead, vec![0]);
        assert_eq!(c.prob(0, 1), c.start[1]);
        assert!(build_rows(&ms, &cum_of(&ms), false)[0].is_none());
    }

    #[test]
    fn disabling_the_only_target_kills_the_row() {
        let mut ms = maps(2);
        ms[0].xaos.set(0, 0.0);
        let c = Chain::build(&ms, &[true, false]).unwrap();
        assert_eq!(c.dead, vec![0]);
    }

    #[test]
    fn with_every_transition_allowed_the_dimension_is_the_ordinary_one() {
        let mut ms = maps(4);
        ms[0].xaos.set(1, 3.0); // odds only
        let c = Chain::build(&ms, &[]).unwrap();
        let d = c.dimension(&contractions(&ms)).unwrap();
        assert!((d - 2.0).abs() < 1e-3, "{d}");
    }

    #[test]
    fn a_cut_lowers_the_dimension_as_the_golden_mean_shift_does() {
        // Two maps at 1/2, b -> b forbidden: admissible words are the golden
        // mean shift, growth rate φ, so d = ln φ / ln 2.
        let mut ms = maps(2);
        ms[1].xaos.set(1, 0.0);
        let d = Chain::build(&ms, &[]).unwrap().dimension(&contractions(&ms)).unwrap();
        let phi = (1.0 + 5f32.sqrt()) / 2.0;
        assert!((d - phi.ln() / 2f32.ln()).abs() < 1e-3, "{d}");
    }

    #[test]
    fn a_cycle_is_a_finite_set() {
        let mut ms = maps(3);
        apply_preset(&mut ms, Preset::Cycle);
        let d = Chain::build(&ms, &[]).unwrap().dimension(&contractions(&ms)).unwrap();
        assert!(d < 1e-3, "{d}");
    }

    #[test]
    fn edits_keep_rows_pointing_at_the_same_maps() {
        let mut ms = maps(4);
        ms[0].xaos = Xaos { rest: 1.0, exceptions: vec![(1, 0.0), (3, 2.0)] };
        remove_column(&mut ms, 0, 1);
        ms.remove(1);
        assert_eq!(ms[0].xaos.exceptions, vec![(2, 2.0)]);
        // Insert a copy of map 2 at position 1.
        insert_column(&mut ms, 0, 1);
        let copy = ms[2].clone();
        ms.insert(1, copy);
        assert_eq!(ms[0].xaos.exceptions, vec![(3, 2.0)]);
        copy_column(&mut ms, 0, 3, 1);
        assert_eq!(ms[0].xaos.exceptions, vec![(1, 2.0), (3, 2.0)]);
    }

    #[test]
    fn rows_keep_their_invariants() {
        let mut x = Xaos::default();
        x.set(3, 0.0);
        x.set(1, 2.0);
        x.set(3, 1.0);
        assert_eq!(x.exceptions, vec![(1, 2.0)]);
        x.set_rest(2.0);
        assert!(x.exceptions.is_empty() && x.rest == 2.0);
        assert_eq!(x.dense(3), vec![2.0; 3]);
    }

    #[test]
    fn presets_say_what_they_mean() {
        assert_eq!(Preset::parse("blocks:2"), Ok(Preset::Blocks(2)));
        assert!(Preset::parse("blocks:1").is_err());
        let rows = Preset::Blocks(2).rows(4);
        assert_eq!(rows[0].dense(4), vec![1.0, 1.0, 0.0, 0.0]);
        assert_eq!(rows[3].dense(4), vec![0.0, 0.0, 1.0, 1.0]);
        assert_eq!(Preset::NoSelfLoops.rows(3)[2].dense(3), vec![1.0, 1.0, 0.0]);
    }

    #[test]
    fn a_probability_solves_for_its_multiplier() {
        let mut ms = maps(3);
        ms[2].weight = 2.0;
        let x = multiplier_for(&ms, 0, 1, 0.4).unwrap();
        ms[0].xaos.set(1, x);
        let p = Chain::build(&ms, &[]).unwrap().prob(0, 1);
        assert!((p - 0.4).abs() < 1e-6, "{p}");
        assert_eq!(multiplier_for(&ms, 0, 1, 1.0), Some(f32::INFINITY));
    }

    #[test]
    fn a_zoom_map_must_be_allowed_after_everything_that_fires() {
        let mut ms = maps(2);
        assert!(zoom_conflict(&ms, &[None, None], 0).is_none());
        ms[1].xaos.set(0, 0.0);
        assert!(zoom_conflict(&ms, &[None, Some("b".into())], 0).unwrap().contains("'b'"));
    }
}
