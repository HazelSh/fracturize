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
}

/// Unified undo/redo stacks. Newest entry is the *last* element of `undo`
/// (so `undo_display()` — newest-first for the history list — iterates it
/// in reverse).
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    max_entries: usize,
    max_bytes: usize,
}

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
        }
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
            if let Some(top) = self.undo.last_mut() {
                if top.key.as_deref() == Some(key) && now.saturating_duration_since(top.at) < COALESCE_WINDOW {
                    top.at = now;
                    return;
                }
            }
        }

        self.undo.push(HistoryEntry {
            label: label.into(),
            key: coalesce_key.map(str::to_string),
            at: now,
            before,
        });
        Self::evict(&mut self.undo, self.max_entries, self.max_bytes);
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
        });
        Self::evict(&mut self.redo, self.max_entries, self.max_bytes);
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
        });
        Self::evict(&mut self.undo, self.max_entries, self.max_bytes);
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

    /// Evict oldest-first until both caps are satisfied. Never evicts the
    /// last remaining entry purely for the byte cap — a single
    /// oversized snapshot still leaves one step of undo available.
    fn evict(stack: &mut Vec<HistoryEntry>, max_entries: usize, max_bytes: usize) {
        while stack.len() > max_entries {
            stack.remove(0);
        }
        while stack.len() > 1 && Self::total_bytes(stack) > max_bytes {
            stack.remove(0);
        }
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
            decay: 0.8,
            color_speed: 0.5,
            color_falloff: 0.0,
            color_contrast: 1.0,
            haze: 0.0,
            transforms: vec![TransformSpec {
                matrix: Mat4::IDENTITY,
                color_value: 0.0,
                weight,
                color_speed: 0.5,
                explicit_color_speed: None,
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
    fn byte_cap_evicts_oldest() {
        // Each 1-transform snapshot estimates to 1*176 + 4096 = 4272 bytes;
        // a 5000-byte cap allows only one such entry at a time.
        let mut h = History::with_caps(64, 5000);
        let t0 = Instant::now();
        h.commit("E0", None, snap(0.0), t0);
        h.commit("E1", None, snap(1.0), t0 + Duration::from_secs(2));
        assert_eq!(h.undo_len(), 1, "byte cap should evict the oldest entry once exceeded");
        assert_eq!(h.undo_display().next(), Some("E1"));
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
