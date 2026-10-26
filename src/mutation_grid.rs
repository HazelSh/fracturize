//! The mutation grid: the scene in the middle, its mutations around it, and
//! a click makes one of them the scene. Apophysis's look-and-choose, in place
//! of roll, judge, undo.
//!
//! Two things live here: the **layout**, which the Mutate window and
//! `--mutations 8|24` share so an agent's sheet and the window are the same
//! picture, and the **model** the window draws — the tiles, made on the
//! worker threads the Random flames list uses (`random_list::spawn_workers`),
//! through the same `dice::mutate_seeded` that U and `--mutations` call.
//!
//! **Tile `k` is seed `base + k`**, exactly as on a `--mutations` sheet, so
//! `--mutations 8 --seed <base>` renders the window's grid. A 3×3's ring is a
//! 5×5's inner ring, tile for tile; the 5×5's outer ring is made at twice the
//! strength, so one grid shows near and far.
//!
//! **A parent keeps its grid.** The base seed is remembered per parent (by a
//! fingerprint of the scene, camera left out), so after picking a tile,
//! Ctrl+Z brings back the old parent *and the same eight around it*, and a
//! sibling can be picked instead. Nothing about this touches the history: the
//! grid follows whatever scene is on screen, and a scene it has seen before
//! gets the seed it had.
//!
//! **Every tile is framed by the live camera**, not the scene's saved one:
//! the comparison is of form, seen the way you were looking at it. Picking a
//! tile keeps the scene's saved framing as it was (`App::adopt_mutation`).

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::camera::OrbitCamera;
use crate::dice::{self, RandomSettings, STRENGTH_LIMITS};
use crate::random_list::{spawn_workers, Done, Job, Outcome};
use crate::scene::Scene;

/// The two grid sizes: 3×3 (eight mutations) and 5×5 (twenty-four).
pub const SIZES: [u32; 2] = [3, 5];

/// Mutations in an `n`×`n` grid.
pub fn children(n: u32) -> u32 {
    n * n - 1
}

/// The grid size whose ring holds exactly `count` mutations, if one does:
/// `--mutations 8` and `--mutations 24` lay out as the window does.
pub fn size_for(count: u32) -> Option<u32> {
    SIZES.into_iter().find(|&n| children(n) == count)
}

/// Which ring a cell is on, counting the parent's cell as ring 0.
fn ring_of(n: u32, col: u32, row: u32) -> u32 {
    let c = (n / 2) as i32;
    (col as i32 - c).unsigned_abs().max((row as i32 - c).unsigned_abs())
}

/// Where tile `k` (1-based; 0 is the parent) sits, as `(col, row)`. Inner
/// ring first in reading order, then the outer ring in reading order — so a
/// 3×3's tiles are a 5×5's first eight, in the same places around the parent.
pub fn cell(n: u32, k: u32) -> (u32, u32) {
    if k == 0 {
        return (n / 2, n / 2);
    }
    let mut seen = 0;
    for ring in 1..=n / 2 {
        for row in 0..n {
            for col in 0..n {
                if ring_of(n, col, row) == ring {
                    seen += 1;
                    if seen == k {
                        return (col, row);
                    }
                }
            }
        }
    }
    panic!("tile {k} is outside a {n}x{n} grid")
}

/// Tile `k`'s ring: 1 next to the parent, 2 on a 5×5's edge.
pub fn ring(n: u32, k: u32) -> u32 {
    let (col, row) = cell(n, k);
    ring_of(n, col, row)
}

/// The strength tile `k` is made at: the profile's next to the parent, twice
/// it (within the limits) on the outer ring.
pub fn ring_strength(strength: f32, ring: u32) -> f32 {
    if ring <= 1 { strength } else { (strength * 2.0).min(STRENGTH_LIMITS[1]) }
}

/// The settings tile `k` is mutated under.
pub fn settings_for_ring(settings: &RandomSettings, ring: u32) -> RandomSettings {
    let mut s = settings.clone();
    s.mutation.strength = ring_strength(settings.mutation.strength, ring);
    s
}

/// A mutation on show.
pub struct Child {
    /// Unique for the life of the app: the thumbnail cache's key.
    pub id: u64,
    pub seed: u64,
    pub scene: Scene,
    /// What changed, one line per change.
    pub log: Vec<String>,
    /// The strength it was made at, after any back-off.
    pub strength: f32,
    /// The strength its ring asked for.
    pub asked: f32,
}

pub enum Tile {
    Pending,
    Ready(Child),
    /// Nothing passed the gate in the budget: why.
    Failed(String),
}

/// How long the scene or the settings have to sit still before the grid
/// starts over — so a drag doesn't restart it every frame.
const SETTLE: Duration = Duration::from_millis(400);

/// Parents remembered with their seeds. Each is two integers; this is far
/// more history than anyone walks back through.
const REMEMBERED: usize = 256;

pub struct MutationGrid {
    /// 3 or 5.
    size: u32,
    /// The scene the tiles are mutations of, framed by the camera it was
    /// taken at.
    parent: Option<Arc<Scene>>,
    pub parent_id: u64,
    parent_print: u64,
    pub base_seed: u64,
    /// Tile `k` is `tiles[k - 1]`.
    pub tiles: Vec<Tile>,
    settings: RandomSettings,
    /// The last (history position, scene loads) the grid was told about, and
    /// since when — which says when to look at the scene again.
    seen: Option<(Option<u64>, u64)>,
    changed_since: Option<Instant>,
    /// Start over without waiting; see [`Self::expire`].
    immediate: bool,
    /// The id the next parent takes; see [`Self::expire`].
    picked: Option<u64>,
    /// Parents and the seeds their grids were made from, newest last.
    seeds: VecDeque<(u64, u64)>,
    generation: Arc<AtomicU64>,
    next_id: u64,
    workers: Option<(Sender<Job>, Receiver<Done>)>,
}

impl Default for MutationGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationGrid {
    pub fn new() -> Self {
        Self {
            size: 3,
            parent: None,
            parent_id: 0,
            parent_print: 0,
            base_seed: 0,
            tiles: Vec::new(),
            settings: RandomSettings::default(),
            seen: None,
            changed_since: None,
            immediate: false,
            picked: None,
            seeds: VecDeque::new(),
            generation: Arc::new(AtomicU64::new(0)),
            next_id: 0,
            workers: None,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// The parent as the tiles were made from it.
    pub fn parent(&self) -> Option<&Scene> {
        self.parent.as_deref()
    }

    /// Whether tiles are still being made, or the grid is about to start over.
    pub fn working(&self) -> bool {
        self.changed_since.is_some() || self.tiles.iter().any(|t| matches!(t, Tile::Pending))
    }

    /// Start over on the next poll without waiting for things to settle: tile
    /// `picked` (its id) was just made the scene, and the new parent wants
    /// its ring at once. The new parent takes the picked tile's id, so its
    /// picture moves to the middle instead of being drawn again.
    pub fn expire(&mut self, picked: Option<u64>) {
        self.immediate = true;
        self.picked = picked;
        self.changed_since = Some(Instant::now() - SETTLE);
    }

    /// Bring the grid up to date with what's on screen, and collect finished
    /// tiles. Called once a frame while the grid is shown.
    ///
    /// `serial` is the history's position and `loads` counts scenes opened:
    /// between them they change whenever the scene might have, which is when
    /// the (comparatively dear) fingerprint is taken — once things settle.
    pub fn poll(
        &mut self,
        settings: &RandomSettings,
        scene: &Scene,
        camera: &OrbitCamera,
        serial: Option<u64>,
        loads: u64,
        size: u32,
    ) {
        let size = if SIZES.contains(&size) { size } else { 3 };
        let marker = (serial, loads);
        let moved = self.seen != Some(marker);
        if moved || (self.changed_since.is_none() && (*settings != self.settings || size != self.size)) {
            self.seen = Some(marker);
            // Each change pushes the restart back, so it lands once a drag
            // ends rather than on every frame of it — unless there is no grid
            // yet, or a tile was just picked and its ring is wanted now.
            let now = self.parent.is_none() || self.immediate;
            self.changed_since = Some(if now { Instant::now() - SETTLE } else { Instant::now() });
        }
        if self.changed_since.is_some_and(|t| t.elapsed() >= SETTLE) {
            self.changed_since = None;
            self.immediate = false;
            let print = fingerprint(scene);
            if self.parent.is_none() || print != self.parent_print || *settings != self.settings || size != self.size {
                self.settings = settings.clone();
                self.size = size;
                let seed = self.remembered(print).unwrap_or_else(crate::random_list::fresh_seed);
                self.start(scene, camera, print, seed);
            }
        }
        self.collect();
    }

    /// New mutations of the same parent: a fresh seed, remembered for it.
    pub fn reroll(&mut self, settings: &RandomSettings, scene: &Scene, camera: &OrbitCamera, size: u32) {
        self.settings = settings.clone();
        if SIZES.contains(&size) {
            self.size = size;
        }
        let print = fingerprint(scene);
        self.start(scene, camera, print, crate::random_list::fresh_seed());
    }

    fn remembered(&self, print: u64) -> Option<u64> {
        self.seeds.iter().rev().find(|(p, _)| *p == print).map(|&(_, s)| s)
    }

    fn start(&mut self, scene: &Scene, camera: &OrbitCamera, print: u64, seed: u64) {
        self.seeds.retain(|(p, _)| *p != print);
        self.seeds.push_back((print, seed));
        while self.seeds.len() > REMEMBERED {
            self.seeds.pop_front();
        }
        let mut parent = scene.clone();
        parent.set_camera(camera);
        let parent = Arc::new(parent);
        self.parent = Some(parent.clone());
        self.parent_print = print;
        self.base_seed = seed;
        self.next_id += 1;
        self.parent_id = self.picked.take().unwrap_or(self.next_id);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let n = children(self.size);
        self.tiles = (0..n).map(|_| Tile::Pending).collect();
        let inner = Arc::new(settings_for_ring(&self.settings, 1));
        let outer = Arc::new(settings_for_ring(&self.settings, 2));
        let (tx, _) = self.workers.get_or_insert_with(|| spawn_workers("mutation-grid", self.generation.clone()));
        for k in 1..=n {
            let settings = if ring(self.size, k) <= 1 { inner.clone() } else { outer.clone() };
            let job = Job {
                generation,
                seed: seed.wrapping_add(k as u64),
                settings,
                base: parent.clone(),
                action: dice::Action::Mutate,
                tag: k as usize,
            };
            if tx.send(job).is_err() {
                self.tiles[k as usize - 1] = Tile::Failed("the mutation workers aren't running".to_string());
            }
        }
    }

    fn collect(&mut self) {
        let Some((_, rx)) = &self.workers else { return };
        let generation = self.generation.load(Ordering::Relaxed);
        let done: Vec<Done> = rx.try_iter().collect();
        for d in done {
            if d.generation != generation || d.tag == 0 || d.tag > self.tiles.len() {
                continue;
            }
            let asked = ring_strength(self.settings.mutation.strength, ring(self.size, d.tag as u32));
            self.tiles[d.tag - 1] = match d.outcome {
                Outcome::Rolled(r) => {
                    self.next_id += 1;
                    Tile::Ready(Child {
                        id: self.next_id,
                        seed: d.seed,
                        strength: r.strength.unwrap_or(asked),
                        asked,
                        log: r.log,
                        scene: r.scene,
                    })
                }
                Outcome::Failed { message, .. } => Tile::Failed(message),
            };
        }
    }

    /// Tile `k`'s mutation, if it has one.
    pub fn child(&self, k: u32) -> Option<&Child> {
        match self.tiles.get((k as usize).checked_sub(1)?) {
            Some(Tile::Ready(c)) => Some(c),
            _ => None,
        }
    }
}

/// The scene, less what an orbit or a save moves: two scenes with the same
/// fingerprint mutate the same way from the same seed.
fn fingerprint(scene: &Scene) -> u64 {
    let mut s = scene.clone();
    s.set_camera(&Scene::blank().camera());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.to_toml_string().unwrap_or_default().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_is_in_the_middle_and_a_3x3_is_the_5x5s_inner_ring() {
        assert_eq!(cell(3, 0), (1, 1));
        assert_eq!(cell(5, 0), (2, 2));
        let three: Vec<_> = (1..=8).map(|k| cell(3, k)).collect();
        assert_eq!(three, [(0, 0), (1, 0), (2, 0), (0, 1), (2, 1), (0, 2), (1, 2), (2, 2)]);
        for k in 1..=8 {
            let (c, r) = cell(3, k);
            assert_eq!(cell(5, k), (c + 1, r + 1), "tile {k}");
            assert_eq!(ring(5, k), 1);
        }
        let mut all: Vec<_> = (0..=24).map(|k| cell(5, k)).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 25, "every cell once");
        assert!((9..=24).all(|k| ring(5, k) == 2));
        assert_eq!(size_for(8), Some(3));
        assert_eq!(size_for(24), Some(5));
        assert_eq!(size_for(6), None);
    }

    #[test]
    fn the_outer_ring_is_twice_as_strong_within_the_limit() {
        assert_eq!(ring_strength(0.7, 1), 0.7);
        assert_eq!(ring_strength(0.7, 2), 1.4);
        assert_eq!(ring_strength(2.5, 2), STRENGTH_LIMITS[1]);
    }

    #[test]
    fn the_camera_is_not_part_of_the_fingerprint() {
        let a = Scene::blank();
        let mut b = a.clone();
        b.camera_distance *= 3.0;
        assert_eq!(fingerprint(&a), fingerprint(&b));
        let mut c = a.clone();
        c.transforms[0].weight = 0.25;
        assert_ne!(fingerprint(&a), fingerprint(&c));
    }

    fn fill(grid: &mut MutationGrid, settings: &RandomSettings, scene: &Scene, serial: Option<u64>, size: u32) {
        let start = Instant::now();
        loop {
            grid.poll(settings, scene, &scene.camera(), serial, 0, size);
            if (!grid.working() && grid.parent().is_some()) || start.elapsed() > Duration::from_secs(30) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn seeds(grid: &MutationGrid) -> Vec<Option<u64>> {
        (1..=children(grid.size())).map(|k| grid.child(k).map(|c| c.seed)).collect()
    }

    /// The window's grid is the CLI's sheet: tile k is `mutate_seeded` with
    /// seed base+k, and going back to a parent brings back its grid.
    #[test]
    fn tiles_are_the_cli_seeds_and_a_parent_keeps_its_grid() {
        let settings = RandomSettings::default();
        let parent = dice::roll_seeded(&settings, &Scene::blank(), 7).ok().expect("a parent").scene;
        let mut grid = MutationGrid::new();
        fill(&mut grid, &settings, &parent, Some(1), 3);
        assert_eq!(grid.tiles.len(), 8);
        let ready = (1..=8).filter(|&k| grid.child(k).is_some()).count();
        assert!(ready >= 6, "only {ready} of 8 tiles passed");
        for k in 1..=8 {
            if let Some(c) = grid.child(k) {
                assert_eq!(c.seed, grid.base_seed + k as u64);
                let again = dice::mutate_seeded(&settings, grid.parent().unwrap(), c.seed).ok().expect("reproduces");
                assert_eq!(again.log, c.log, "tile {k}");
            }
        }
        let first = (grid.base_seed, seeds(&grid));

        // Pick tile 1: a new parent, a new grid.
        let picked = grid.child(1).or(grid.child(2)).map(|c| c.scene.clone()).expect("a tile");
        grid.expire(None);
        fill(&mut grid, &settings, &picked, Some(2), 3);
        assert_ne!(grid.base_seed, first.0);

        // Undo: the old parent, the same seeds.
        grid.expire(None);
        fill(&mut grid, &settings, &parent, Some(1), 3);
        assert_eq!((grid.base_seed, seeds(&grid)), first);

        // A 5x5 of the same parent keeps those eight as its inner ring.
        fill(&mut grid, &settings, &parent, Some(1), 5);
        assert_eq!(grid.tiles.len(), 24);
        assert_eq!(grid.base_seed, first.0);
        assert_eq!(seeds(&grid)[..8], first.1[..]);
        let outer = (9..=24).find_map(|k| grid.child(k)).expect("an outer tile");
        assert_eq!(outer.asked, ring_strength(settings.mutation.strength, 2));
    }
}
