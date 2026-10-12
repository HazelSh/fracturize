//! The Random flames list's model: rolls made on background threads, in the
//! order they land, growing as the list is scrolled.
//!
//! Rolling is off the UI thread because a bound can make it slow. One roll is
//! ~1 ms at the default settings (`randomize::random_flame_study`), but a
//! lacunarity bound is rejection-sampled, and a tight one can spend its whole
//! budget of candidates — a hitch per row if it ran in the frame.
//!
//! **Every roll is reproducible from its seed.** A row's seed seeds a
//! `StdRng` that is handed to `randomize::roll` with the settings the list was
//! rolled under — exactly what `--random --seed N --random-config FILE` does —
//! so a row someone liked can be had again from the command line.
//!
//! Changing the settings starts the list over, after a short pause so a drag
//! across a slider doesn't restart it on every frame. Rolls already on their
//! way under the old settings are dropped rather than shown: a list that
//! claims to be "what these settings roll" must not mix in what the last ones
//! rolled.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::randomize::{self, Rejections, RandomSettings};
use crate::scene::{Scene, VARIATION_NAMES};

/// One roll in the list.
pub struct RandomEntry {
    /// Unique for the life of the app — the thumbnail cache's key.
    pub id: u64,
    pub seed: u64,
    pub scene: Scene,
    pub dimension: Option<f32>,
    pub lacunarity: Option<f32>,
    /// Candidates it took, this one included.
    pub attempts: u32,
    /// The variations it carries, by name, in slot order.
    pub kinds: Vec<&'static str>,
}

struct Job {
    generation: u64,
    seed: u64,
    settings: Arc<RandomSettings>,
}

enum Outcome {
    Rolled(Box<randomize::Roll>),
    Failed { attempts: u32, rejected: Rejections },
}

struct Done {
    generation: u64,
    seed: u64,
    outcome: Outcome,
}

/// Rolls that came back empty in a row before the list stops asking. Past
/// this the settings are almost certainly asking for something the dice
/// can't make, and rolling on would only heat the CPU; the window says so and
/// offers to keep going.
const STALL_AFTER: u32 = 8;

/// How long settings have to sit still before the list starts over.
const SETTLE: Duration = Duration::from_millis(400);

pub struct RandomList {
    pub entries: Vec<RandomEntry>,
    /// Under these settings: rolls that found nothing, and candidates tried
    /// in all (the acceptance rate the settings window shows).
    pub failures: u32,
    pub candidates: u64,
    pub rejected: Rejections,
    /// Consecutive empty rolls; see [`STALL_AFTER`].
    empty_run: u32,
    /// Asked to keep rolling past a stall.
    persist: bool,
    settings: Arc<RandomSettings>,
    changed_since: Option<Instant>,
    generation: Arc<AtomicU64>,
    base_seed: u64,
    next_index: u64,
    next_id: u64,
    in_flight: usize,
    workers: Option<(Sender<Job>, Receiver<Done>)>,
}

impl RandomList {
    pub fn new(settings: RandomSettings) -> Self {
        Self {
            entries: Vec::new(),
            failures: 0,
            candidates: 0,
            rejected: Rejections::default(),
            empty_run: 0,
            persist: false,
            settings: Arc::new(settings),
            changed_since: None,
            generation: Arc::new(AtomicU64::new(0)),
            base_seed: fresh_seed(),
            next_index: 0,
            next_id: 0,
            in_flight: 0,
            workers: None,
        }
    }

    /// The settings the rows on show were rolled under.
    pub fn settings(&self) -> &RandomSettings {
        &self.settings
    }

    /// The settings changed; start over once they settle.
    pub fn settings_changed(&mut self, settings: &RandomSettings) {
        if *self.settings != *settings {
            self.changed_since = Some(Instant::now());
        }
    }

    /// Throw the list away and roll afresh: new seeds, same settings.
    pub fn reroll(&mut self, settings: &RandomSettings) {
        self.settings = Arc::new(settings.clone());
        self.changed_since = None;
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.entries.clear();
        self.failures = 0;
        self.candidates = 0;
        self.rejected = Rejections::default();
        self.empty_run = 0;
        self.persist = false;
        self.base_seed = fresh_seed();
        self.next_index = 0;
        self.in_flight = 0;
    }

    /// Whether rolling has stopped because nothing is coming back.
    pub fn stalled(&self) -> bool {
        self.empty_run >= STALL_AFTER && !self.persist
    }

    /// Rolls that have come back empty in a row.
    pub fn empty_run(&self) -> u32 {
        self.empty_run
    }

    /// Keep rolling past a stall.
    pub fn persist(&mut self) {
        self.persist = true;
    }

    /// Whether more rows are on their way.
    pub fn rolling(&self) -> bool {
        self.in_flight > 0 || self.changed_since.is_some()
    }

    /// Mean candidates per roll that found something, if any did.
    pub fn tries_per_flame(&self) -> Option<f32> {
        (!self.entries.is_empty()).then(|| {
            self.entries.iter().map(|e| e.attempts as f32).sum::<f32>() / self.entries.len() as f32
        })
    }

    /// Collect what the workers finished, restart if the settings have
    /// settled, and keep `want` rows rolled or on their way. Called once a
    /// frame while the list is shown.
    pub fn poll(&mut self, settings: &RandomSettings, want: usize) {
        if self.changed_since.is_some_and(|t| t.elapsed() >= SETTLE) {
            if *self.settings != *settings {
                self.reroll(settings);
            } else {
                // Changed and changed back: the rows on show still stand.
                self.changed_since = None;
            }
        }
        if let Some((_, rx)) = &self.workers {
            let generation = self.generation.load(Ordering::Relaxed);
            let done: Vec<Done> = rx.try_iter().collect();
            for d in done {
                if d.generation != generation {
                    continue;
                }
                self.in_flight = self.in_flight.saturating_sub(1);
                self.accept(d);
            }
        }
        if self.changed_since.is_some() || self.stalled() {
            return;
        }
        while self.entries.len() + self.in_flight < want {
            self.submit();
        }
    }

    fn accept(&mut self, d: Done) {
        match d.outcome {
            Outcome::Rolled(roll) => {
                self.candidates += roll.attempts as u64;
                self.add_rejections(roll.rejected);
                self.empty_run = 0;
                let kinds = (1..VARIATION_NAMES.len())
                    .filter(|&s| roll.scene.transforms.iter().any(|t| t.variations[s] != 0.0))
                    .map(|s| VARIATION_NAMES[s])
                    .collect();
                self.next_id += 1;
                self.entries.push(RandomEntry {
                    id: self.next_id,
                    seed: d.seed,
                    scene: roll.scene,
                    dimension: roll.dimension,
                    lacunarity: roll.lacunarity,
                    attempts: roll.attempts,
                    kinds,
                });
            }
            Outcome::Failed { attempts, rejected } => {
                self.candidates += attempts as u64;
                self.add_rejections(rejected);
                self.failures += 1;
                self.empty_run += 1;
            }
        }
    }

    fn add_rejections(&mut self, r: Rejections) {
        self.rejected.shape += r.shape;
        self.rejected.dimension += r.dimension;
        self.rejected.lacunarity += r.lacunarity;
    }

    fn submit(&mut self) {
        let generation = self.generation.load(Ordering::Relaxed);
        let seed = self.base_seed.wrapping_add(self.next_index);
        self.next_index += 1;
        let job = Job { generation, seed, settings: self.settings.clone() };
        let (tx, _) = self.workers.get_or_insert_with(|| spawn_workers(self.generation.clone()));
        if tx.send(job).is_ok() {
            self.in_flight += 1;
        }
    }
}

/// A seed short enough to read: rolls are named after their seeds, and a
/// 20-digit name in the toolbar helps nobody.
pub fn fresh_seed() -> u64 {
    rand::random::<u32>() as u64
}

/// Threads rolling flames: a couple, leaving the rest of the machine to the
/// renderer and the person. They live as long as the app.
fn spawn_workers(current: Arc<AtomicU64>) -> (Sender<Job>, Receiver<Done>) {
    let (job_tx, job_rx) = channel::<Job>();
    let (done_tx, done_rx) = channel::<Done>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2)).clamp(1, 3);
    for i in 0..threads {
        let job_rx = job_rx.clone();
        let done_tx = done_tx.clone();
        let current = current.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("random-flames-{i}"))
            .spawn(move || {
                loop {
                    let Ok(job) = job_rx.lock().map(|rx| rx.recv()) else { return };
                    let Ok(job) = job else { return };
                    // Settings moved on while this sat in the queue.
                    if job.generation != current.load(Ordering::Relaxed) {
                        continue;
                    }
                    let outcome = match randomize::roll_seeded(&job.settings, job.seed) {
                        Ok(r) => Outcome::Rolled(Box::new(r)),
                        Err(f) => Outcome::Failed { attempts: f.attempts, rejected: f.rejected },
                    };
                    if done_tx.send(Done { generation: job.generation, seed: job.seed, outcome }).is_err() {
                        return;
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("Random flames: couldn't start a worker: {e}");
        }
    }
    (job_tx, done_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(list: &mut RandomList, settings: &RandomSettings, want: usize) {
        let start = Instant::now();
        while list.entries.len() < want && !list.stalled() && start.elapsed() < Duration::from_secs(20) {
            list.poll(settings, want);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_list_fills_and_rows_reproduce_from_their_seeds() {
        let settings = RandomSettings::default();
        let mut list = RandomList::new(settings.clone());
        fill(&mut list, &settings, 6);
        assert!(list.entries.len() >= 6, "only {} rows", list.entries.len());
        let ids: std::collections::HashSet<u64> = list.entries.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), list.entries.len(), "ids are unique");
        for e in &list.entries {
            let again = randomize::roll_seeded(&settings, e.seed).ok().expect("a seed that rolled rolls again");
            assert_eq!(again.scene.background, e.scene.background, "seed {}", e.seed);
            assert_eq!(again.scene.transforms.len(), e.scene.transforms.len());
        }
    }

    #[test]
    fn impossible_settings_stall_instead_of_spinning() {
        let settings = RandomSettings {
            lacunarity: randomize::Bound { on: true, range: [900.0, 1000.0] },
            attempts: 3,
            ..Default::default()
        };
        let mut list = RandomList::new(settings.clone());
        fill(&mut list, &settings, 4);
        assert!(list.stalled(), "{} failures", list.failures);
        assert!(list.entries.is_empty());
        assert!(list.rejected.lacunarity > 0);
    }

    #[test]
    fn rolls_from_old_settings_are_dropped() {
        let settings = RandomSettings::default();
        let mut list = RandomList::new(settings.clone());
        list.poll(&settings, 8);
        let other = RandomSettings { transforms: [7, 7], ..Default::default() };
        list.reroll(&other);
        fill(&mut list, &other, 4);
        for e in &list.entries {
            assert_eq!(e.scene.transforms.len(), 7, "a row from the old settings survived");
        }
    }
}
