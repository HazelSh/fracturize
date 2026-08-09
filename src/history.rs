//! Unified undo/redo history: a single snapshot-based stack covering every
//! scene-mutating edit path (gizmo drags, weight/color/variation keys,
//! mutate, add/delete transform, ...). Pure logic — no egui/gpu deps — so it
//! can be unit-tested directly; `App` (src/app.rs) is the only caller.
//!
//! The choke point is `App::commit_edit(label, coalesce_key, before)`:
//! callers snapshot state with `App::edit_snapshot()` *before* mutating,
//! perform the mutation, then commit. `commit` clears the redo stack unless
//! the new commit coalesces into the top of the undo stack (same key, <1s
//! since the top entry's timestamp — held-key nudges and scroll bursts
//! collapse into one entry, keeping the *original* before-state).
//!
//! Both stacks store the "before" snapshot of each entry; `undo`/`redo` pop
//! one entry, push the *current* state onto the opposite stack (so the
//! round trip is exact), and return the popped entry's `before` to restore.
//! This makes `undo` and `redo` perfectly symmetric and lets a multi-step
//! `jump_undo`/`jump_redo` (clicking N deep into the history list) walk the
//! stacks one step at a time internally while only paying for one final
//! snapshot restore.

use std::time::{Duration, Instant};

use crate::scene::Scene;

/// How long a coalescing key stays "hot": a same-key commit landing within
/// this window of the previous one merges into it instead of creating a new
/// entry (held-key repeats, drag-scroll bursts).
const COALESCE_WINDOW: Duration = Duration::from_millis(1000);

/// Default entry-count cap (plan: 64-deep).
pub const DEFAULT_MAX_ENTRIES: usize = 64;
/// Default estimated-bytes cap (plan: ~128 MiB) — matters for 40k-transform
/// L-system scenes, where even a handful of full-scene snapshots add up.
pub const DEFAULT_MAX_BYTES: usize = 128 * 1024 * 1024;

/// Everything a scene-mutating edit can touch, captured whole so undo/redo
/// is a single assignment. `App::edit_snapshot()` builds one; `App::commit_edit`
/// is the only place they get consumed into history.
#[derive(Clone)]
pub struct EditSnapshot {
    pub scene: Scene,
    pub transform_enabled: Vec<bool>,
    pub point_size: f32,
    pub color_falloff: f32,
    pub color_contrast: f32,
    pub haze_amount: f32,
    /// Splat exposure. Lives on `App` as well as on `Scene` — the same
    /// live-value/scene-value pairing as `point_size` and `haze_amount`.
    pub exposure: f32,
}

impl EditSnapshot {
    /// Rough memory footprint, for the byte-cap eviction. Dominated by
    /// `Scene::transforms` (`TransformSpec` is a handful of Mat4/f32 fields,
    /// ~176 bytes); the flat 4096 covers the colormap array and other
    /// scene-level bookkeeping.
    pub fn estimated_bytes(&self) -> usize {
        self.scene.transforms.len() * 176 + 4096
    }
}

/// One undoable/redoable action.
struct HistoryEntry {
    label: String,
    /// Coalescing key (see module docs); `None` never coalesces.
    key: Option<String>,
    at: Instant,
    before: EditSnapshot,
    /// Identifies the document state this entry *produced*, for the dirty bit
    /// (see [`History::top_serial`]). Unique, never reused, and it travels
    /// with the entry across the undo/redo boundary so a round trip comes back
    /// to the same state by the same name.
    serial: u64,
}

/// Unified undo/redo stacks. Newest entry is the *last* element of `undo`
/// (so `undo_display()` — newest-first for the history list — iterates it
/// in reverse).
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    max_entries: usize,
    max_bytes: usize,
    /// How many undo entries have been evicted since the last `clear`.
    ///
    /// Kept so the Explore window can *say so*. Silent eviction is the worst
    /// kind: on an L-system scene one whole-scene snapshot runs to megabytes,
    /// so the byte cap binds long before the entry cap and the stack can be
    /// ground down to its floor with nothing on screen admitting it. A person
    /// who thinks they have 60 steps of undo and has 10 finds out at the worst
    /// possible moment.
    dropped: usize,
    /// Source of [`HistoryEntry::serial`]. Monotonic for the life of the
    /// `History`, and deliberately not reset by `clear` — a serial handed out
    /// before a scene was opened must never be able to match one handed out
    /// after it.
    next_serial: u64,
}

/// Entries the byte cap may never take, however large the snapshots are.
///
/// The cap exists to stop history eating the machine; it is not a licence to
/// leave a person with one step of undo. Ten is enough to get out of a mistake
/// that took a few edits to make, which is what undo is *for*.
pub const MIN_ENTRIES: usize = 10;

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self::with_caps(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }

    pub fn with_caps(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            max_entries,
            max_bytes,
            dropped: 0,
            next_serial: 0,
        }
    }

    /// Names the document state the stacks are currently at: the serial of the
    /// newest applied edit, or `None` for "nothing has been done".
    ///
    /// This is the whole of the dirty bit. `App` remembers what this returned
    /// when the scene was last written and compares; equal means the file on
    /// disk and the scene in memory are the same document, whether you got
    /// back here by not editing, by undoing every edit, or by redoing forward
    /// to where you saved. A boolean flag can't say any of that — it only ever
    /// went one way, so undoing back past your last save left a scene that was
    /// byte-for-byte the saved one still claiming to be modified.
    ///
    /// Fails safe under the caps: an entry evicted from the bottom of the
    /// stack takes its serial with it, so a save point that has been evicted
    /// simply never compares equal again and the scene stays dirty.
    pub fn top_serial(&self) -> Option<u64> {
        self.undo.last().map(|e| e.serial)
    }

    fn take_serial(&mut self) -> u64 {
        let s = self.next_serial;
        self.next_serial += 1;
        s
    }

    /// Undo entries evicted since the last `clear`, for the history list's
    /// "… N older edits dropped" footer.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Record a commit. Clears the redo stack, unless this commit coalesces
    /// into the top of the undo stack (same `coalesce_key`, landing within
    /// `COALESCE_WINDOW` of that entry's timestamp) — in which case the
    /// *original* `before` snapshot is kept and only the timestamp refreshes.
    pub fn commit(
        &mut self,
        label: impl Into<String>,
        coalesce_key: Option<&str>,
        before: EditSnapshot,
        now: Instant,
    ) {
        self.redo.clear();

        if let Some(key) = coalesce_key {
            // A coalesced commit still *changes the document*, so it still
            // takes a fresh serial even though it adds no entry. Without that,
            // saving mid-drag and then dragging further within the coalescing
            // window would leave the file reading as clean while the scene
            // moved out from under it.
            let serial = self.take_serial();
            if let Some(top) = self.undo.last_mut() {
                if top.key.as_deref() == Some(key) && now.saturating_duration_since(top.at) < COALESCE_WINDOW {
                    top.at = now;
                    top.serial = serial;
                    return;
                }
            }
        }

        let serial = self.take_serial();
        self.undo.push(HistoryEntry {
            label: label.into(),
            key: coalesce_key.map(str::to_string),
            at: now,
            before,
            serial,
        });
        self.dropped += Self::evict(&mut self.undo, self.max_entries, self.max_bytes);
    }

    /// Pop the most recent undo entry, push `current` onto redo, and return
    /// (label, snapshot-to-restore). `None` when there's nothing to undo.
    pub fn undo(&mut self, current: EditSnapshot) -> Option<(String, EditSnapshot)> {
        let entry = self.undo.pop()?;
        let restore = entry.before;
        self.redo.push(HistoryEntry {
            label: entry.label.clone(),
            key: None,
            at: Instant::now(),
            before: current,
            // The serial names the state this edit produced, so it belongs to
            // the edit wherever the edit currently sits. Redoing pushes it
            // back onto the undo stack and `top_serial` reads the same value
            // it did before the undo.
            serial: entry.serial,
        });
        // Redo-side eviction isn't counted: the footer speaks about the undo
        // list, and a redo entry lost to the caps is already unreachable by the
        // time it matters.
        let _ = Self::evict(&mut self.redo, self.max_entries, self.max_bytes);
        Some((entry.label, restore))
    }

    /// Symmetric opposite of `undo`.
    pub fn redo(&mut self, current: EditSnapshot) -> Option<(String, EditSnapshot)> {
        let entry = self.redo.pop()?;
        let restore = entry.before;
        self.undo.push(HistoryEntry {
            label: entry.label.clone(),
            key: None,
            at: Instant::now(),
            before: current,
            serial: entry.serial,
        });
        self.dropped += Self::evict(&mut self.undo, self.max_entries, self.max_bytes);
        Some((entry.label, restore))
    }

    /// Undo `steps` entries in one go (clicking N deep into the history
    /// list). `None` if fewer than `steps` undo entries exist — the caller
    /// makes no state change in that case.
    pub fn jump_undo(&mut self, steps: usize, mut current: EditSnapshot) -> Option<EditSnapshot> {
        if steps == 0 || steps > self.undo.len() {
            return None;
        }
        for _ in 0..steps {
            let (_, restored) = self.undo(current)?;
            current = restored;
        }
        Some(current)
    }

    /// Symmetric opposite of `jump_undo`.
    pub fn jump_redo(&mut self, steps: usize, mut current: EditSnapshot) -> Option<EditSnapshot> {
        if steps == 0 || steps > self.redo.len() {
            return None;
        }
        for _ in 0..steps {
            let (_, restored) = self.redo(current)?;
            current = restored;
        }
        Some(current)
    }

    /// Drop all history (scene load: a fresh scene has no undoable past).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.dropped = 0;
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Undo entries, newest-first (index 0 = what Ctrl+Z undoes next) — the
    /// order the Explore window's history list displays them in.
    pub fn undo_display(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().rev().map(|e| e.label.as_str())
    }

    /// Redo entries in display order: index 0 is the *furthest* redo, the
    /// last item is the very next thing Ctrl+Shift+Z would redo (i.e. this
    /// is the natural stack order — nearest-to-current sits closest to the
    /// list's "current position" marker when redo entries are drawn above
    /// it, undo entries below).
    pub fn redo_display(&self) -> impl Iterator<Item = &str> {
        self.redo.iter().map(|e| e.label.as_str())
    }

    /// Evict oldest-first until both caps are satisfied, returning how many
    /// entries went.
    ///
    /// The byte cap stops at [`MIN_ENTRIES`] rather than at one: the cap is
    /// there to stop history eating the machine, and grinding a person down to
    /// a single step of undo because their scene has 40k transforms is not what
    /// it was for. The entry cap has no such floor — it is a count, so it can
    /// never be in tension with one.
    fn evict(stack: &mut Vec<HistoryEntry>, max_entries: usize, max_bytes: usize) -> usize {
        let was = stack.len();
        while stack.len() > max_entries {
            stack.remove(0);
        }
        while stack.len() > MIN_ENTRIES.min(max_entries) && Self::total_bytes(stack) > max_bytes {
            stack.remove(0);
        }
        was - stack.len()
    }

    fn total_bytes(stack: &[HistoryEntry]) -> usize {
        stack.iter().map(|e| e.before.estimated_bytes()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3};
    use crate::scene::TransformSpec;

    /// Minimal scene with one transform whose weight we vary, so snapshots
    /// are cheaply distinguishable.
    fn dummy_scene(weight: f32) -> Scene {
        Scene {
            name: "Test".to_string(),
            author: "Test".to_string(),
            point_size: 0.002,
            points_per_frame: 1000,
            point_count: 1000,
            point_count_defaulted: false,
            decay: 0.8,
            color_speed: 0.5,
            color_falloff: 0.0,
            color_contrast: 1.0,
            haze: 0.0,
            exposure: 1.0,
            transforms: vec![TransformSpec {
                matrix: Mat4::IDENTITY,
                post_affine: Mat4::IDENTITY,
                color_value: 0.0,
                weight,
                color_speed: 0.5,
                explicit_color_speed: None,
                symmetry: None,
                variations: TransformSpec::linear_variations(),
            }],
            transform_names: vec![None],
            colors: vec![Vec3::ONE],
            palette: None,
            color_mode: crate::scene::ColorMode::Transforms,
            colormap: [[0.0; 4]; 256],
            camera_focus: Vec3::ZERO,
            camera_distance: 3.0,
            camera_orientation: crate::rot::Orientation::IDENTITY,
            background: crate::scene::DEFAULT_BACKGROUND,
            camera_path: None,
            zoom: None,
        }
    }

    fn snap(weight: f32) -> EditSnapshot {
        EditSnapshot {
            scene: dummy_scene(weight),
            transform_enabled: vec![true],
            point_size: 0.002,
            color_falloff: 0.0,
            color_contrast: 1.0,
            haze_amount: 0.0,
            exposure: 1.0,
        }
    }

    fn weight_of(snap: &EditSnapshot) -> f32 {
        snap.scene.transforms[0].weight
    }

    #[test]
    fn coalescing_same_key_within_window_keeps_original_before() {
        let mut h = History::new();
        let t0 = Instant::now();
        // Three rapid commits under the same key, as a held "." key would
        // produce: each commit's `before` is the state right before that
        // particular nudge, but the entry should remember only the first.
        h.commit("Weight", Some("weight:T0"), snap(1.0), t0);
        h.commit("Weight", Some("weight:T0"), snap(1.15), t0 + Duration::from_millis(200));
        h.commit("Weight", Some("weight:T0"), snap(1.32), t0 + Duration::from_millis(400));

        assert_eq!(h.undo_len(), 1, "same-key commits within the window should coalesce into one entry");

        let (label, restored) = h.undo(snap(1.52)).expect("one entry to undo");
        assert_eq!(label, "Weight");
        assert_eq!(weight_of(&restored), 1.0, "coalesced entry must keep the ORIGINAL before-state");
    }

    #[test]
    fn different_coalesce_key_does_not_coalesce() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("Weight", Some("weight:T0"), snap(1.0), t0);
        h.commit("Hue", Some("color:0:T0"), snap(1.15), t0 + Duration::from_millis(50));
        assert_eq!(h.undo_len(), 2, "a different coalesce key must start a new entry");
    }

    #[test]
    fn elapsed_past_window_does_not_coalesce() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("Weight", Some("weight:T0"), snap(1.0), t0);
        h.commit("Weight", Some("weight:T0"), snap(1.15), t0 + Duration::from_millis(1500));
        assert_eq!(h.undo_len(), 2, "a commit more than 1s after the last must start a new entry");
    }

    #[test]
    fn no_coalesce_key_never_coalesces() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("Add transform", None, snap(1.0), t0);
        h.commit("Add transform", None, snap(1.0), t0 + Duration::from_millis(10));
        assert_eq!(h.undo_len(), 2, "commits with no coalesce key are always distinct entries");
    }

    #[test]
    fn commit_clears_redo() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("A", None, snap(1.0), t0);
        let (_, restored) = h.undo(snap(1.15)).expect("undo A");
        assert_eq!(h.redo_len(), 1, "undo should populate redo");

        h.commit("B", None, restored, t0 + Duration::from_millis(10));
        assert_eq!(h.redo_len(), 0, "a fresh commit must clear redo");
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut h = History::new();
        let t0 = Instant::now();
        // Edit A: weight 1.0 -> 2.0
        h.commit("A", None, snap(1.0), t0);
        // Edit B: weight 2.0 -> 3.0
        h.commit("B", None, snap(2.0), t0 + Duration::from_secs(2));

        let (label, restored) = h.undo(snap(3.0)).expect("undo B");
        assert_eq!(label, "B");
        assert_eq!(weight_of(&restored), 2.0);

        let (label, restored) = h.undo(restored).expect("undo A");
        assert_eq!(label, "A");
        assert_eq!(weight_of(&restored), 1.0);
        assert_eq!(h.undo_len(), 0);
        assert_eq!(h.redo_len(), 2);

        let (label, restored) = h.redo(restored).expect("redo A");
        assert_eq!(label, "A");
        assert_eq!(weight_of(&restored), 2.0);

        let (label, restored) = h.redo(restored).expect("redo B");
        assert_eq!(label, "B");
        assert_eq!(weight_of(&restored), 3.0);
        assert_eq!(h.redo_len(), 0);
        assert_eq!(h.undo_len(), 2);
    }

    #[test]
    fn jump_undo_and_redo_multi_step() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("A", None, snap(1.0), t0);
        h.commit("B", None, snap(2.0), t0 + Duration::from_secs(2));
        h.commit("C", None, snap(3.0), t0 + Duration::from_secs(4));

        let restored = h.jump_undo(3, snap(4.0)).expect("jump back 3 steps");
        assert_eq!(weight_of(&restored), 1.0);
        assert_eq!(h.undo_len(), 0);
        assert_eq!(h.redo_len(), 3);

        // Asking for more steps than are available must not partially apply
        assert!(h.jump_undo(1, snap(1.0)).is_none());

        let restored = h.jump_redo(3, restored).expect("jump forward 3 steps");
        assert_eq!(weight_of(&restored), 4.0);
        assert_eq!(h.redo_len(), 0);
        assert_eq!(h.undo_len(), 3);
    }

    #[test]
    fn entry_cap_evicts_oldest() {
        let mut h = History::with_caps(3, usize::MAX);
        let t0 = Instant::now();
        for i in 0..5 {
            h.commit(format!("E{}", i), None, snap(i as f32), t0 + Duration::from_secs(i as u64 * 2));
        }
        assert_eq!(h.undo_len(), 3);
        let labels: Vec<&str> = h.undo_display().collect();
        assert_eq!(labels, vec!["E4", "E3", "E2"], "entry cap should evict the oldest entries");
    }

    #[test]
    fn byte_cap_evicts_oldest_but_stops_at_the_floor() {
        // Each 1-transform snapshot estimates to 1*176 + 4096 = 4272 bytes, so
        // a 5000-byte cap is exceeded by the second entry and never satisfied
        // again. It used to grind the stack down to one; now it stops at
        // MIN_ENTRIES, because a person with a 40k-transform scene still
        // deserves an undo stack.
        let mut h = History::with_caps(64, 5000);
        let t0 = Instant::now();
        for i in 0..15 {
            h.commit(format!("E{}", i), None, snap(i as f32), t0 + Duration::from_secs(2 * i));
        }
        assert_eq!(h.undo_len(), MIN_ENTRIES, "the byte cap must not evict past the floor");
        assert_eq!(h.undo_display().next(), Some("E14"), "newest is kept");
        assert_eq!(
            h.undo_display().last(),
            Some("E5"),
            "and the oldest survivor is the one the floor saved",
        );
    }

    #[test]
    fn eviction_is_counted_so_the_ui_can_say_so() {
        let mut h = History::with_caps(3, usize::MAX);
        let t0 = Instant::now();
        for i in 0..7 {
            h.commit(format!("E{}", i), None, snap(i as f32), t0 + Duration::from_secs(2 * i));
        }
        assert_eq!(h.undo_len(), 3);
        assert_eq!(h.dropped(), 4, "four entries went, and the list should be able to say so");
        h.clear();
        assert_eq!(h.dropped(), 0, "a fresh document starts with a clean count");
    }

    #[test]
    fn a_tiny_entry_cap_still_wins_over_the_floor() {
        // The floor guards the *byte* cap. An explicitly-asked-for entry cap
        // below it is a count, and a count can't be in tension with itself.
        let mut h = History::with_caps(2, 1);
        let t0 = Instant::now();
        for i in 0..5 {
            h.commit(format!("E{}", i), None, snap(i as f32), t0 + Duration::from_secs(2 * i));
        }
        assert_eq!(h.undo_len(), 2);
    }

    // === The save point (App::is_dirty compares `top_serial` against it) ===

    #[test]
    fn undoing_back_to_the_save_point_reads_as_saved() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("A", None, snap(1.0), t0);
        h.commit("B", None, snap(2.0), t0 + Duration::from_secs(2));
        // ...saved here...
        let saved = h.top_serial();
        h.commit("C", None, snap(3.0), t0 + Duration::from_secs(4));
        h.commit("D", None, snap(4.0), t0 + Duration::from_secs(6));
        assert_ne!(h.top_serial(), saved, "two edits past the save point");

        h.undo(snap(5.0)).unwrap();
        assert_ne!(h.top_serial(), saved, "one edit past it is still not it");
        h.undo(snap(4.0)).unwrap();
        assert_eq!(h.top_serial(), saved, "back at the state that was written");

        // ...and forward again, which the old boolean could never report.
        h.redo(snap(3.0)).unwrap();
        assert_ne!(h.top_serial(), saved, "redoing past the save point is dirty again");
        h.undo(snap(4.0)).unwrap();
        assert_eq!(h.top_serial(), saved, "and undoing back to it is clean again");
    }

    #[test]
    fn undoing_every_edit_reaches_the_freshly_opened_state() {
        // The literal todo.txt item: "full undo should mark file undirty".
        let mut h = History::new();
        let t0 = Instant::now();
        let opened = h.top_serial();
        assert_eq!(opened, None);
        h.commit("A", None, snap(1.0), t0);
        h.commit("B", None, snap(2.0), t0 + Duration::from_secs(2));
        h.undo(snap(3.0)).unwrap();
        h.undo(snap(2.0)).unwrap();
        assert_eq!(h.top_serial(), opened);
    }

    #[test]
    fn a_coalesced_commit_still_moves_off_the_save_point() {
        // Save mid-drag, then keep dragging: the commit coalesces into the
        // entry that was on top when you saved, so the *entry* is the same one
        // — but the scene has moved and the file has not.
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("Weight", Some("weight:T0"), snap(1.0), t0);
        let saved = h.top_serial();
        h.commit("Weight", Some("weight:T0"), snap(1.15), t0 + Duration::from_millis(200));
        assert_eq!(h.undo_len(), 1, "still one entry — it coalesced");
        assert_ne!(h.top_serial(), saved, "but the document is not the saved one");
    }

    #[test]
    fn an_evicted_save_point_never_compares_equal_again() {
        // Fails safe: if the caps take the entry you saved at, the scene stays
        // dirty rather than claiming to be a file it can no longer get back to.
        let mut h = History::with_caps(3, usize::MAX);
        let t0 = Instant::now();
        h.commit("A", None, snap(1.0), t0);
        let saved = h.top_serial();
        for i in 1..6 {
            h.commit(format!("E{}", i), None, snap(i as f32), t0 + Duration::from_secs(2 * i));
        }
        while h.undo_len() > 0 {
            h.undo(snap(9.0)).unwrap();
            assert_ne!(h.top_serial(), saved, "the evicted save point is unreachable");
        }
        assert_eq!(h.top_serial(), None);
        assert_ne!(h.top_serial(), saved);
    }

    #[test]
    fn clear_empties_both_stacks() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.commit("A", None, snap(1.0), t0);
        let (_, restored) = h.undo(snap(2.0)).unwrap();
        assert!(h.undo_len() == 0 && h.redo_len() == 1);
        h.clear();
        assert_eq!(h.undo_len(), 0);
        assert_eq!(h.redo_len(), 0);
        drop(restored);
    }
}
