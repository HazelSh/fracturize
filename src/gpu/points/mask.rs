//! Mute and solo: a *view* of which maps placed which points (FEATURE-PLANS
//! item 14).
//!
//! Not enable/disable. Disabling a map takes it out of the IFS and the
//! attractor changes; muting one leaves the walk exactly as it was and only
//! stops drawing the points it placed — `--isolate`'s partition (a point
//! belongs to the map that moved it last, and the parts add up to the
//! picture), generalised from one map to any set, and live.
//!
//! `--isolate` drops points as they are written, so a change would take the
//! ring's thirteen seconds to show. Here the chaos pass instead records, per
//! point, which map placed it (`point_maps`, parallel to the point ring, four
//! bytes a point), and the vertex stages clip against a small mask. So changing
//! the mask shows on the very next frame. The record is allocated only once
//! something is first muted or soloed, and kept, the way emphasis keeps its
//! sensitivity buffer; until it has filled in, an unrecorded point counts as
//! hidden, so the first solo densifies in rather than showing everything.
//!
//! Exposure is still normalised by every point, as `--isolate`'s is, so a
//! soloed part is at the brightness it has in the full picture.
//!
//! The mask is one `u32` of "on", then a bit per GPU slot, set to hide. Every
//! renderer binds it; all but the live window's bind stand-ins with "on" at 0,
//! which is why offline renders, thumbnails and render jobs never see it.

use wgpu::{Buffer, Device};

/// The two read-only storage entries a renderer adds to its group 0, at
/// `first` and `first + 1`: the per-point map record, then the mask.
pub fn layout_entries(first: u32) -> [wgpu::BindGroupLayoutEntry; 2] {
    let entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    [entry(first), entry(first + 1)]
}

/// Sixteen zeroed bytes: as a mask, "off"; as a map record, never read.
pub fn stand_in(device: &Device, label: &str) -> Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

/// A renderer's map-record and mask buffers, and which pair they are, so a
/// renderer only rebuilds its bind group when the pair changes.
#[derive(Clone)]
pub struct Binding {
    pub maps: Buffer,
    pub mask: Buffer,
    pub generation: u64,
}

impl Binding {
    pub fn stand_in(device: &Device) -> Self {
        Self { maps: stand_in(device, "point_maps_stand_in"), mask: stand_in(device, "map_mask_stand_in"), generation: 0 }
    }
}

/// A fresh generation number, unique in the process, for a newly allocated
/// pair. Zero is the stand-ins'.
pub fn next_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Mute/solo for every map, by scene index — session state, never scene data.
/// Mixer rules, as on a mixing desk: if anything is soloed, only the soloed
/// maps show; otherwise everything but the muted ones does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MuteSolo {
    pub mute: Vec<bool>,
    pub solo: Vec<bool>,
}

impl MuteSolo {
    /// Whether anything is muted or soloed — whether the mask is on at all.
    pub fn active(&self) -> bool {
        self.mute.iter().chain(&self.solo).any(|&b| b)
    }

    pub fn is_muted(&self, i: usize) -> bool {
        self.mute.get(i).copied().unwrap_or(false)
    }

    pub fn is_soloed(&self, i: usize) -> bool {
        self.solo.get(i).copied().unwrap_or(false)
    }

    /// Whether map `i`'s points are hidden by the mute/solo rules.
    pub fn hides(&self, i: usize) -> bool {
        if self.solo.iter().any(|&b| b) { !self.is_soloed(i) } else { self.is_muted(i) }
    }

    /// Per scene index, whether its points are hidden; `None` when nothing is
    /// muted or soloed, which turns the mask off.
    pub fn hidden(&self, n: usize) -> Option<Vec<bool>> {
        self.active().then(|| (0..n).map(|i| self.hides(i)).collect())
    }

    pub fn toggle_mute(&mut self, i: usize) {
        Self::flip(&mut self.mute, i);
    }

    pub fn toggle_solo(&mut self, i: usize) {
        Self::flip(&mut self.solo, i);
    }

    /// Alt+click on a solo: this map alone, or, if it already is, nothing
    /// soloed at all.
    pub fn solo_only(&mut self, i: usize) {
        let only = self.is_soloed(i) && self.solo.iter().filter(|&&b| b).count() == 1;
        self.solo.clear();
        if !only {
            Self::flip(&mut self.solo, i);
        }
    }

    pub fn clear(&mut self) {
        self.mute.clear();
        self.solo.clear();
    }

    fn flip(v: &mut Vec<bool>, i: usize) {
        if v.len() <= i {
            v.resize(i + 1, false);
        }
        v[i] = !v[i];
    }
}

/// The mask words for the GPU: "on", then a bit a GPU slot. `order` is GPU
/// slot -> scene index (the buffer is in layer order, not scene order).
pub fn mask_words(hidden: Option<&[bool]>, order: &[usize]) -> Vec<u32> {
    let Some(hidden) = hidden else { return vec![0] };
    let mut words = vec![0u32; 1 + order.len().div_ceil(32)];
    words[0] = 1;
    for (slot, &i) in order.iter().enumerate() {
        if hidden.get(i).copied().unwrap_or(false) {
            words[1 + slot / 32] |= 1 << (slot % 32);
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_wins_over_mute() {
        let mut m = MuteSolo::default();
        assert_eq!(m.hidden(3), None, "nothing set is the mask off");
        m.toggle_mute(0);
        assert_eq!(m.hidden(3), Some(vec![true, false, false]));
        m.toggle_solo(0);
        // Soloed and muted both: a solo is a question about that map, and it
        // gets answered.
        assert_eq!(m.hidden(3), Some(vec![false, true, true]));
        m.toggle_solo(0);
        m.toggle_mute(0);
        assert_eq!(m.hidden(3), None);
    }

    #[test]
    fn alt_solo_is_this_one_alone_then_nothing() {
        let mut m = MuteSolo::default();
        m.toggle_solo(1);
        m.toggle_solo(2);
        m.solo_only(2);
        assert_eq!(m.hidden(3), Some(vec![true, true, false]));
        m.solo_only(2);
        assert_eq!(m.hidden(3), None);
    }

    #[test]
    fn the_mask_is_in_gpu_order() {
        // Scene maps 0..40, the GPU holding them reversed.
        let order: Vec<usize> = (0..40).rev().collect();
        let mut hidden = vec![false; 40];
        hidden[0] = true; // GPU slot 39
        hidden[39] = true; // GPU slot 0
        let w = mask_words(Some(&hidden), &order);
        assert_eq!(w, vec![1, 1, 1 << 7]);
        assert_eq!(mask_words(None, &order), vec![0]);
    }

    /// The bit layout the shaders read, stated once in Rust: `hidden()` in
    /// render.wgsl and splat.wgsl must index the mask the way `mask_words`
    /// writes it.
    #[test]
    fn the_shaders_read_the_mask_this_way() {
        for (name, src) in [
            ("render.wgsl", include_str!("../../../shaders/points/render.wgsl")),
            ("splat.wgsl", include_str!("../../../shaders/points/splat.wgsl")),
        ] {
            assert!(src.contains("if map_mask[0] == 0u {"), "{name}");
            assert!(src.contains("((map_mask[1u + s / 32u] >> (s % 32u)) & 1u) != 0u"), "{name}");
        }
        let chaos = include_str!("../../../shaders/points/chaos.wgsl");
        assert!(chaos.contains("point_maps[output_idx] = transform_idx + 1u;"));
    }
}
