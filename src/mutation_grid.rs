//! The mutation grid's model: the scene in the centre, a ring of its
//! mutations around it — 8 in a 3×3, 24 in a 5×5 — made on background
//! threads. Click one and it becomes the scene, and a new ring grows round it.
//!
//! This is the Random flames list with a parent instead of nothing: the same
//! dice (`dice::mutate_seeded`, judged against the parent by the gate), the
//! same worker pool (`random_list::spawn_pool`), the same rule that work
//! begun under old settings is dropped rather than shown.
//!
//! **Tile numbering is the CLI's.** Tiles are numbered 1..=N row by row,
//! skipping the centre, and tile `k` is `mutate_seeded(seed + k)` — so
//! `--mutations 8|24 --seed G` draws exactly the grid the window shows, with
//! the parent in the middle. In a 5×5 the outer ring mutates at twice the
//! profile's strength ([`ring_settings`]), so one grid shows near and far.
//!
//! **A grid belongs to a point in the history.** Grids are kept by
//! [`GridKey`] — which document, and the history's `top_serial` — so Ctrl+Z
//! after accepting a tile brings back the previous parent *and the ring it
//! was picked from*, and a sibling can be picked instead. Serials are never
//! reused, so a key can't name two different scenes. Nothing about the grid
//! is in the history itself: it is a view of the scene, and a regenerated
//! ring is the same ring, because every tile is its seed.
//!
//! When the scene changes some other way (an edit, `U`, a load) the grid
//! waits for it to settle and starts over around it with a fresh seed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dice::{self, RandomSettings};
use crate::random_list::{fresh_seed, spawn_pool};
use crate::scene::Scene;

/// 3×3 or 5×5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GridSize {
    #[default]
    Three,
    Five,
}

impl GridSize {
    /// Tiles along a side.
    pub fn side(self) -> usize {
        match self {
            Self::Three => 3,
            Self::Five => 5,
        }
    }

    /// Mutations in the ring: every tile but the centre.
    pub fn children(self) -> usize {
        self.side() * self.side() - 1
    }

    /// The grid a `--mutations` count lays out as, if it is one.
    pub fn for_children(n: usize) -> Option<Self> {
        [Self::Three, Self::Five].into_iter().find(|s| s.children() == n)
    }
}

/// One cell of the grid, in row-major order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    /// 0 for the centre (the parent), else 1..=N, numbered row by row.
    pub tile: usize,
    /// Rings out from the centre: 0, 1 or 2.
    pub ring: u32,
}

/// Every cell of a grid, row by row.
pub fn cells(size: GridSize) -> Vec<Cell> {
    let side = size.side();
    let mid = side / 2;
    let mut tile = 0;
    (0..side * side)
        .map(|i| {
            let (row, col) = (i / side, i % side);
            let ring = row.abs_diff(mid).max(col.abs_diff(mid)) as u32;
            let n = if ring == 0 {
                0
            } else {
                tile += 1;
                tile
            };
            Cell { row, col, tile: n, ring }
        })
        .collect()
}

/// The profile a ring mutates under: the profile's strength times the ring,
/// so a 5×5's outer ring takes twice the step its inner ring does.
pub fn ring_settings(settings: &RandomSettings, ring: u32) -> RandomSettings {
    let mut s = settings.clone();
    s.mutation.strength *= ring.max(1) as f32;
    s
}

/// Which grid: the document (bumped on every load, which clears the history)
/// and where its history is (`History::top_serial`).
pub type GridKey = (u64, Option<u64>);

/// A mutation that passed the gate.
pub struct Child {
    /// The thumbnail cache's key (`ThumbKey::Grid`); never reused.
    pub id: u64,
    pub scene: Scene,
    /// The operators it applied, as the History window will name them.
    pub log: Vec<String>,
    /// "strength backed off to …", if it had to.
    pub note: Option<String>,
}

pub enum TileState {
    Pending,
    Ready(Box<Child>),
    /// Nothing passed in the profile's budget; why.
    Empty(String),
}

pub struct Tile {
    pub cell: Cell,
    pub seed: u64,
    pub state: TileState,
}

struct Grid {
    key: GridKey,
    parent: Arc<Scene>,
    /// The centre's thumbnail key. An accepted tile keeps its own, so the
    /// picture that was just clicked doesn't flicker as it moves to the middle.
    parent_id: u64,
    seed: u64,
    size: GridSize,
    settings: Arc<RandomSettings>,
    /// Ring 1 first: the near ones land before the far ones.
    tiles: Vec<Tile>,
}

struct Job {
    generation: u64,
    tile: usize,
    seed: u64,
    settings: Arc<RandomSettings>,
    parent: Arc<Scene>,
}

struct Done {
    generation: u64,
    tile: usize,
    /// The child and its backoff note, or why there's none.
    outcome: Result<(dice::Roll, Option<String>), String>,
}

/// How long the scene or settings have to sit still before the ring starts
/// over, so a drag doesn't restart it every frame.
const SETTLE: Duration = Duration::from_millis(400);

/// Grids kept for Ctrl+Z. Each holds at most 25 scenes.
const MEMORY: usize = 32;

/// Unpolled for this long and the window was closed: a reopened grid starts
/// at once rather than settling.
const AWAY: Duration = Duration::from_millis(500);

pub struct MutationGrid {
    current: Option<Grid>,
    /// Grids for other points in the history, oldest first.
    memory: Vec<Grid>,
    /// What the window shows next: 3×3 or 5×5.
    pub size: GridSize,
    changed_since: Option<Instant>,
    /// A tile was accepted: the grid for this key grows round that tile at
    /// once, keeping its thumbnail as the centre's.
    accepted: Option<(GridKey, u64)>,
    last_poll: Option<Instant>,
    generation: Arc<AtomicU64>,
    next_id: u64,
    workers: Option<(Sender<Job>, Receiver<Done>)>,
}

impl MutationGrid {
    pub fn new(size: GridSize) -> Self {
        Self {
            current: None,
            memory: Vec::new(),
            size,
            changed_since: None,
            accepted: None,
            last_poll: None,
            generation: Arc::new(AtomicU64::new(0)),
            next_id: 0,
            workers: None,
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The tiles, in row-major order with the centre left out, if a grid has
    /// been started.
    pub fn tiles(&self) -> &[Tile] {
        self.current.as_ref().map_or(&[], |g| &g.tiles)
    }

    /// The tile numbered `n`.
    pub fn tile(&self, n: usize) -> Option<&Tile> {
        self.tiles().iter().find(|t| t.cell.tile == n)
    }

    /// The centre: the scene the ring was made from, and its thumbnail key.
    pub fn parent(&self) -> Option<(&Scene, u64)> {
        self.current.as_ref().map(|g| (g.parent.as_ref(), g.parent_id))
    }

    /// The seed tile `k` is `seed + k` from, and the grid's size.
    pub fn seed(&self) -> Option<(u64, GridSize)> {
        self.current.as_ref().map(|g| (g.seed, g.size))
    }

    /// The settings the ring on show was made under.
    pub fn settings(&self) -> Option<&RandomSettings> {
        self.current.as_ref().map(|g| g.settings.as_ref())
    }

    /// Whether the ring on show is out of date and about to start over.
    pub fn settling(&self) -> bool {
        self.changed_since.is_some()
    }

    /// Mutations still being made.
    pub fn pending(&self) -> usize {
        self.tiles().iter().filter(|t| matches!(t.state, TileState::Pending)).count()
    }

    /// Tile `id` was accepted and the scene is now it, at history position
    /// `key`. The next poll grows the ring round it without waiting.
    pub fn accepted(&mut self, key: GridKey, id: u64) {
        self.accepted = Some((key, id));
    }

    /// A new ring round the same parent: the same scene, a fresh seed.
    pub fn reroll(&mut self) {
        if let Some(g) = self.current.take() {
            let (key, parent, settings) = (g.key, g.parent.clone(), g.settings.clone());
            self.start(key, parent, settings, fresh_seed(), None);
        }
    }

    /// Collect finished tiles, and start over (or bring back a remembered
    /// grid) if the scene, the settings or the size moved. Called once a
    /// frame while the grid is shown. `scene` is only cloned when a grid
    /// starts.
    pub fn poll(&mut self, key: GridKey, scene: &Scene, settings: &RandomSettings) {
        let away = self.last_poll.is_none_or(|t| t.elapsed() > AWAY);
        self.last_poll = Some(Instant::now());
        self.collect();

        let (same_scene, resized, stale) = match &self.current {
            None => (false, false, true),
            Some(g) => (
                g.key == key,
                g.size != self.size,
                g.key != key || *g.settings != *settings || g.size != self.size,
            ),
        };
        if !stale {
            self.changed_since = None;
            return;
        }
        // Back to a point in the history this grid has been: Ctrl+Z.
        if let Some(i) = self
            .memory
            .iter()
            .position(|g| g.key == key && *g.settings == *settings && g.size == self.size)
        {
            let g = self.memory.remove(i);
            self.changed_since = None;
            self.replace(g);
            self.submit_pending();
            return;
        }
        let accepted = self.accepted.take_if(|(k, _)| *k == key).map(|(_, id)| id);
        // A size is picked once, not dragged through, so it needn't settle.
        let now = accepted.is_some() || away || resized || self.current.is_none();
        if !now {
            let since = *self.changed_since.get_or_insert_with(Instant::now);
            if since.elapsed() < SETTLE {
                return;
            }
        }
        self.changed_since = None;
        // Only the settings or the size moved: the same seed, so a tile that
        // didn't change under the new settings is the same tile.
        let seed = match &self.current {
            Some(g) if same_scene => g.seed,
            _ => fresh_seed(),
        };
        self.start(key, Arc::new(scene.clone()), Arc::new(settings.clone()), seed, accepted);
    }

    fn start(&mut self, key: GridKey, parent: Arc<Scene>, settings: Arc<RandomSettings>, seed: u64, parent_id: Option<u64>) {
        let parent_id = parent_id.unwrap_or_else(|| self.id());
        let mut tiles: Vec<Tile> = cells(self.size)
            .into_iter()
            .filter(|c| c.tile != 0)
            .map(|cell| Tile { cell, seed: seed.wrapping_add(cell.tile as u64), state: TileState::Pending })
            .collect();
        tiles.sort_by_key(|t| (t.cell.ring, t.cell.tile));
        let size = self.size;
        self.replace(Grid { key, parent, parent_id, seed, size, settings, tiles });
        self.submit_pending();
    }

    /// Make `g` the grid on show, remembering the one it replaces under its
    /// own key (unless `g` is for that same key: a reroll, or new settings).
    fn replace(&mut self, g: Grid) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        if let Some(old) = self.current.take()
            && old.key != g.key
        {
            self.memory.retain(|m| m.key != old.key);
            self.memory.push(old);
            if self.memory.len() > MEMORY {
                self.memory.remove(0);
            }
        }
        self.current = Some(g);
    }

    fn submit_pending(&mut self) {
        let generation = self.generation.load(Ordering::Relaxed);
        let current = self.generation.clone();
        let Some(g) = &self.current else { return };
        let (tx, _) = self.workers.get_or_insert_with(|| spawn_workers(current));
        for t in g.tiles.iter().filter(|t| matches!(t.state, TileState::Pending)) {
            let settings = Arc::new(ring_settings(&g.settings, t.cell.ring));
            let job = Job { generation, tile: t.cell.tile, seed: t.seed, settings, parent: g.parent.clone() };
            if tx.send(job).is_err() {
                return;
            }
        }
    }

    fn collect(&mut self) {
        let Some((_, rx)) = &self.workers else { return };
        let generation = self.generation.load(Ordering::Relaxed);
        let done: Vec<Done> = rx.try_iter().filter(|d| d.generation == generation).collect();
        for d in done {
            let state = match d.outcome {
                Ok((r, note)) => {
                    TileState::Ready(Box::new(Child { id: self.id(), scene: r.scene, log: r.log, note }))
                }
                Err(why) => TileState::Empty(why),
            };
            if let Some(t) = self.current.as_mut().and_then(|g| g.tiles.iter_mut().find(|t| t.cell.tile == d.tile)) {
                t.state = state;
            }
        }
    }
}

fn spawn_workers(current: Arc<AtomicU64>) -> (Sender<Job>, Receiver<Done>) {
    spawn_pool("mutation-grid", move |job: Job| {
        // The grid moved on while this sat in the queue.
        if job.generation != current.load(Ordering::Relaxed) {
            return None;
        }
        let outcome = dice::mutate_seeded(&job.settings, &job.parent, job.seed)
            .map(|r| {
                let note = r.backoff_note(&job.settings);
                (r, note)
            })
            .map_err(|f| f.message());
        Some(Done { generation: job.generation, tile: job.tile, outcome })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent() -> Scene {
        Scene::load("scenes/koru.toml").expect("koru loads")
    }

    fn fill(grid: &mut MutationGrid, key: GridKey, scene: &Scene, settings: &RandomSettings) {
        let start = Instant::now();
        loop {
            grid.poll(key, scene, settings);
            if (grid.current.is_some() && grid.pending() == 0) || start.elapsed() > Duration::from_secs(30) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn cells_number_the_ring_row_by_row_round_the_centre() {
        let c = cells(GridSize::Three);
        assert_eq!(c.iter().map(|c| c.tile).collect::<Vec<_>>(), [1, 2, 3, 4, 0, 5, 6, 7, 8]);
        let c = cells(GridSize::Five);
        assert_eq!(c.len(), 25);
        assert_eq!(c[12], Cell { row: 2, col: 2, tile: 0, ring: 0 });
        assert_eq!(c.iter().filter(|c| c.ring == 1).count(), 8);
        assert_eq!(c.iter().filter(|c| c.ring == 2).count(), 16);
        assert_eq!(c.iter().map(|c| c.tile).max(), Some(24));
        assert_eq!(GridSize::for_children(8), Some(GridSize::Three));
        assert_eq!(GridSize::for_children(24), Some(GridSize::Five));
        assert_eq!(GridSize::for_children(9), None);
    }

    #[test]
    fn a_tile_is_its_seed_as_the_cli_draws_it() {
        let settings = RandomSettings::default();
        let scene = parent();
        let mut grid = MutationGrid::new(GridSize::Five);
        fill(&mut grid, (0, None), &scene, &settings);
        let (seed, _) = grid.seed().unwrap();
        let mut ready = 0;
        for t in grid.tiles() {
            assert_eq!(t.seed, seed.wrapping_add(t.cell.tile as u64));
            if let TileState::Ready(c) = &t.state {
                ready += 1;
                let again = dice::mutate_seeded(&ring_settings(&settings, t.cell.ring), &scene, t.seed).ok().unwrap();
                assert_eq!(again.log, c.log, "tile {}", t.cell.tile);
            }
        }
        assert!(ready >= 20, "only {ready} of 24 tiles passed");
    }

    #[test]
    fn going_back_in_the_history_brings_back_its_grid() {
        let settings = RandomSettings::default();
        let scene = parent();
        let mut grid = MutationGrid::new(GridSize::Three);
        fill(&mut grid, (0, Some(1)), &scene, &settings);
        let first = grid.seed().unwrap().0;
        let pick = grid.tiles().iter().find_map(|t| match &t.state {
            TileState::Ready(c) => Some((c.id, c.scene.clone())),
            _ => None,
        });
        let (id, child) = pick.expect("a tile passed");
        grid.accepted((0, Some(2)), id);
        grid.poll((0, Some(2)), &child, &settings);
        assert_eq!(grid.parent().unwrap().1, id, "the accepted tile's thumbnail is the new centre");
        assert_ne!(grid.seed().unwrap().0, first);
        fill(&mut grid, (0, Some(2)), &child, &settings);
        // Ctrl+Z: the history is back at serial 1, and so is its grid.
        grid.poll((0, Some(1)), &scene, &settings);
        assert_eq!(grid.seed().unwrap().0, first);
        assert_eq!(grid.pending(), 0, "remembered tiles come back made");
    }

    #[test]
    fn work_for_an_old_grid_is_dropped() {
        let settings = RandomSettings::default();
        let scene = parent();
        let mut grid = MutationGrid::new(GridSize::Three);
        grid.poll((0, None), &scene, &settings);
        grid.reroll();
        let seed = grid.seed().unwrap().0;
        fill(&mut grid, (0, None), &scene, &settings);
        for t in grid.tiles() {
            assert_eq!(t.seed, seed.wrapping_add(t.cell.tile as u64));
        }
    }
}
