//! Simple chaos game compute pipeline
//!
//! Runs the IFS chaos game and writes points directly to a buffer.
//! Uses a circular buffer approach for temporal stability.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::gpu::buffers::{GpuTransform, Point, PointComputeParams};
use crate::scene::TransformSpec;

/// Number of threads per workgroup (must match shader)
const WORKGROUP_SIZE: u32 = 256;

/// `wgpu::Limits::max_compute_workgroups_per_dimension`'s guaranteed floor.
/// Anything wider than this has to spill into a second dispatch dimension.
const MAX_WORKGROUPS_PER_DIM: u32 = 65_535;

/// Per-walker state (64 bytes, one per parallel walker)
/// Must match WGSL WalkerState struct
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct WalkerState {
    pub current_pos: [f32; 3],
    pub _pad0: f32,
    pub current_color: f32,
    pub _pad1: f32,
    pub _pad2: f32,
    pub _pad3: f32,
    pub rng_state: [u32; 4],
    /// How far this walker's point moves per unit nudge of the transform the
    /// emphasis is watching, carried forward through every map since. Zero and
    /// untouched while the emphasis is off; see `carry_tangent` in chaos.wgsl.
    pub tangent: [f32; 3],
    /// The (GPU-order) index of the map this walker applied last, for xaos;
    /// `xaos::NO_ROW` when it has no history — fresh, or just re-seeded.
    pub last: u32,
}

impl WalkerState {
    pub fn new(seed: u64) -> Self {
        let mut state = [
            (seed & 0xFFFFFFFF) as u32,
            ((seed >> 32) & 0xFFFFFFFF) as u32,
            seed.wrapping_mul(0x5DEECE66D) as u32,
            (seed.wrapping_mul(0x5DEECE66D) >> 32) as u32,
        ];
        for s in &mut state {
            if *s == 0 {
                *s = 0xDEADBEEF;
            }
        }

        Self {
            current_pos: [0.0, 0.0, 0.0],
            _pad0: 0.0,
            current_color: 0.5,
            _pad1: 0.0,
            _pad2: 0.0,
            _pad3: 0.0,
            rng_state: state,
            tangent: [0.0; 3],
            last: crate::xaos::NO_ROW,
        }
    }
}

/// The chaos game's default seed.
///
/// Until this existed, `WalkerState::new` derived every walker's RNG state from
/// its **index alone** — no clock, no entropy, no user seed — so two runs of
/// the same command produced byte-identical PNGs. That is good for
/// reproducibility and was never a bug, but it meant there was no way to ask
/// for an *independent* sample stream at all: a second pass over the same scene
/// replays the identical walker trajectory and re-counts the same samples, so
/// the picture gets **brighter, not better**.
///
/// Zero, and that is load-bearing rather than arbitrary: at seed 0 the mixing
/// in [`walker_seed`] collapses to exactly the expression that was there
/// before, so every render made before this parameter existed is reproduced
/// byte for byte. Verified, not assumed.
pub const DEFAULT_SEED: u64 = 0;

/// Golden-ratio odd constant, which is what the implicit per-walker seed
/// already multiplied the index by.
const WALKER_STRIDE: u64 = 0x9E37_79B9_7F4A_7C15;

/// The seed a *resumed* run must use, given how many laps are already folded in.
///
/// Resuming with the original seed replays the identical walker stream, so the
/// "new" lap deposits a perfect copy of samples the histogram already holds.
/// Measured before this existed: a 20 spp render checkpointed and resumed to
/// 40 spp came out **byte-identical** to the 20 spp one. The sample count
/// doubles, the noise does not fall at all, and the render reports a quality
/// it did not deliver — which is the worst shape a bug can have here.
///
/// Mixing the lap count in deals an independent hand while staying
/// deterministic: resuming the same checkpoint twice gives the same picture.
/// `prior_laps == 0` returns the seed untouched, so a fresh render is
/// unchanged and [`DEFAULT_SEED`] still reproduces every earlier one.
pub fn resume_seed(seed: u64, prior_laps: u32) -> u64 {
    if prior_laps == 0 {
        return seed;
    }
    // A different constant from WALKER_STRIDE: that one is already applied per
    // walker index, and reusing it here would make lap `k` of a resume collide
    // with walker `k` of a fresh run.
    seed ^ (prior_laps as u64).wrapping_mul(0xA24B_AED4_963E_E407)
}

/// Per-walker seed: the index's own stride, xored with a mixed user seed.
///
/// Xor rather than addition so that [`DEFAULT_SEED`] contributes nothing at
/// all, and the multiply so that seeds 1 and 2 give unrelated streams rather
/// than two that differ in one bit.
fn walker_seed(seed: u64, i: u32) -> u64 {
    (i as u64).wrapping_mul(WALKER_STRIDE) ^ seed.wrapping_mul(0xD6E8_FEB8_6659_FD93)
}

/// One layer as the chaos pass sees it. Must match `Layer` in chaos.wgsl
/// (96 bytes; a uniform array, so the stride is a multiple of 16).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuLayer {
    /// Scene-space placement, applied to each point as it is written.
    pub placement: [[f32; 4]; 4],
    /// This layer's run of the (layer-sorted) transform buffer.
    pub t_start: u32,
    pub t_count: u32,
    /// Walkers `[previous layer's end, walker_end)` belong to this layer.
    pub walker_end: u32,
    /// 1 when the layer is in mix mode, so its colour is the walker's mixed
    /// RGB rather than a lookup in its colormap.
    pub mix: u32,
    pub contrast: f32,
    /// The placement's uniform scale, which carries an emphasis sensitivity.
    pub scale: f32,
    /// 1 when any map of this layer has an xaos row, so its walkers consult
    /// the row table; 0 takes the plain selection, exactly as before xaos.
    pub xaos: u32,
    pub _pad: f32,
}

/// Everything the chaos pass needs to know about a scene's layers, built once
/// per edit from the scene (see `crate::subscene` for the model).
///
/// An ordinary scene is one layer holding every map, with every walker, and no
/// colour resolution: that is bit for bit the chaos pass that existed before
/// subscenes, and `composite == false` is what keeps it so.
pub struct LayerTable {
    /// Transforms in GPU order — grouped by layer — as scene indices.
    pub order: Vec<usize>,
    pub gpu_transforms: Vec<GpuTransform>,
    pub groups: GroupTable,
    /// Per layer: the GPU record (walker_end still unset) and its share of
    /// the walkers, zero when hidden or when none of its maps can fire.
    pub layers: Vec<(GpuLayer, f32)>,
    /// `MAX_LAYERS * 256` colormap entries, layer k's at `k * 256`.
    pub colormaps: Vec<[f32; 4]>,
    pub composite: bool,
    /// Every layer's xaos rows end to end (`crate::xaos::Row`), indexed by
    /// `GpuTransform::xaos_start`. Never empty: a buffer can't be bound at
    /// zero size, so a scene without xaos still uploads one unread entry.
    pub xaos_rows: Vec<[f32; 4]>,
}

impl LayerTable {
    /// `enabled` is the app's per-transform on/off state (all on offline);
    /// `root_contrast` the live value of the scene's own layer's contrast,
    /// which the app keeps outside the scene.
    pub fn build(scene: &crate::scene::Scene, enabled: &[bool], root_contrast: f32) -> Self {
        let transforms = &scene.transforms;
        let is_on = |i: usize| enabled.get(i).copied().unwrap_or(true);
        let composite = scene.is_composite();
        let n_layers = scene.layer_count().min(crate::subscene::MAX_LAYERS);

        let mut order: Vec<usize> = (0..transforms.len()).collect();
        // Stable, so an ordinary scene (all layer 0) keeps its order exactly.
        order.sort_by_key(|&i| transforms[i].layer);

        let ordered: Vec<TransformSpec> = order.iter().map(|&i| transforms[i].clone()).collect();
        let groups = GroupTable::build(&ordered);

        let mut gpu_transforms = Vec::with_capacity(order.len());
        let mut layers = Vec::with_capacity(n_layers);
        let mut colormaps = vec![[0.0f32; 4]; crate::subscene::MAX_LAYERS * 256];
        let mut xaos_rows: Vec<[f32; 4]> = Vec::new();
        let mut pos = 0usize;
        for k in 0..n_layers {
            let start = pos;
            while pos < order.len() && transforms[order[pos]].layer as usize == k {
                pos += 1;
            }
            let run = start..pos;
            let total: f32 = run
                .clone()
                .map(|p| if is_on(order[p]) { transforms[order[p]].weight } else { 0.0 })
                .sum();
            let mut cumulative = 0.0;
            let mut cum = Vec::with_capacity(run.len());
            for p in run.clone() {
                let i = order[p];
                let w = if is_on(i) { transforms[i].weight } else { 0.0 };
                if total > 0.0 {
                    cumulative += w / total;
                }
                cum.push(cumulative);
            }
            // Xaos rows, built from exactly the cumulative weights uploaded
            // below, so the shader's squeezed-axis arithmetic lands on the
            // same intervals its plain search does.
            let maps: Vec<TransformSpec> = run.clone().map(|p| transforms[order[p]].clone()).collect();
            let has_xaos = crate::xaos::layer_has_xaos(&maps) || crate::xaos::forced();
            let rows = if has_xaos {
                crate::xaos::build_rows(&maps, &cum, crate::xaos::forced())
            } else {
                vec![None; maps.len()]
            };
            for (n, p) in run.clone().enumerate() {
                let i = order[p];
                let w = if is_on(i) { transforms[i].weight } else { 0.0 };
                let xaos = match &rows[n] {
                    Some(row) => {
                        let at = xaos_rows.len() as u32;
                        xaos_rows.extend_from_slice(row);
                        (at, row.len() as u32 - 1)
                    }
                    None => (crate::xaos::NO_ROW, 0),
                };
                gpu_transforms.push(GpuTransform::new(
                    &transforms[i],
                    cum[n],
                    w,
                    transform_color(&scene.colors, i),
                    groups.slices.get(p).copied().unwrap_or((0, 0)),
                    xaos,
                ));
            }
            let (mode, contrast, colormap) = scene.layer_colour(k);
            let contrast = if k == 0 { root_contrast } else { contrast };
            colormaps[k * 256..(k + 1) * 256].copy_from_slice(colormap);
            let share = if !composite {
                // Everything, always — including a scene with every map
                // switched off, which then does what it always did.
                1.0
            } else if scene.layer_visible(k) && total > 0.0 {
                scene.layer_share(k).max(0.0)
            } else {
                0.0
            };
            let placement = scene.layer_placement(k);
            layers.push((
                GpuLayer {
                    placement: placement.to_cols_array_2d(),
                    t_start: start as u32,
                    t_count: (pos - start) as u32,
                    walker_end: 0,
                    mix: u32::from(mode.packs_rgb()),
                    contrast,
                    scale: placement.x_axis.truncate().length(),
                    xaos: u32::from(has_xaos),
                    _pad: 0.0,
                },
                share,
            ));
        }
        if xaos_rows.is_empty() {
            xaos_rows.push([0.0; 4]);
        }
        Self { order, gpu_transforms, groups, layers, colormaps, composite, xaos_rows }
    }

    /// The uniform: layers with their walker ranges dealt.
    fn gpu_layers(&self, num_walkers: u32) -> [GpuLayer; crate::subscene::MAX_LAYERS] {
        let shares: Vec<f32> = self.layers.iter().map(|(_, s)| *s).collect();
        let counts = crate::subscene::allocate_walkers(&shares, num_walkers);
        let mut out = [GpuLayer::zeroed(); crate::subscene::MAX_LAYERS];
        let mut end = 0;
        for (k, ((layer, _), n)) in self.layers.iter().zip(counts).enumerate() {
            end += n;
            out[k] = GpuLayer { walker_end: end, ..*layer };
        }
        out
    }
}

/// What `--isolate` keeps: the points `map` placed (scene index) — and with
/// `after`, only those it placed straight after that map, which is one xaos
/// edge's own material: what that edge puts down at the top level, and the
/// first thing cutting it removes.
///
/// Both ride in the one `isolate` word of the compute params, sixteen bits
/// each, one up so zero means "any". A layer of more than 65,534 maps can't
/// name its last ones; isolating one of those keeps nothing, which says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Isolate {
    pub map: usize,
    pub after: Option<usize>,
}

impl From<usize> for Isolate {
    fn from(map: usize) -> Self {
        Self { map, after: None }
    }
}

/// Simple chaos game compute pipeline
/// Writes points directly to a circular buffer
pub struct PointCompute {
    pub pipeline: wgpu::ComputePipeline,
    /// Carries the whole buffer through a camera wrap; see `rewrap` in
    /// `shaders/points/chaos.wgsl`. Same module, same bind group, same
    /// layout — only the entry point differs.
    pub rewrap_pipeline: wgpu::ComputePipeline,
    pub bind_group: wgpu::BindGroup,
    pub transform_buffer: wgpu::Buffer,
    /// Symmetry group elements, all groups concatenated. Fixed capacity
    /// ([`MAX_GROUP_ELEMENTS`]), so a group change is a write, not a rebuild.
    pub group_buffer: wgpu::Buffer,
    pub walker_states_buffer: wgpu::Buffer,
    pub params_buffer: wgpu::Buffer,
    pub point_buffer: wgpu::Buffer,
    pub colormap_buffer: wgpu::Buffer,
    pub num_transforms: u32,
    pub num_walkers: u32,
    pub num_workgroups: u32,
    pub iterations_per_walker: u32,
    /// Higher iteration count used during warmup to fill buffer faster
    pub warmup_iterations_per_walker: u32,
    pub buffer_capacity: u32,
    pub write_offset: u32,
    pub warmup_frames: u32,
    pub current_frame: u32,
    /// Points written since the last reset, never wrapped. The emphasis needs
    /// to know which slice of the ring belongs to the transform it is watching,
    /// and a wrapped offset cannot tell "one lap ago" from "just now".
    pub written_total: u64,
    /// Per-point sensitivity, parallel to `point_buffer` — or a 16-byte
    /// stand-in while the emphasis is off, which is most of the time. Four
    /// bytes a point is a quarter again on top of the point ring, and this is a
    /// renderer whose whole constraint is VRAM, so it is allocated on the first
    /// frame that asks for it and kept thereafter.
    pub sensitivity_buffer: wgpu::Buffer,
    /// [mean, warm]: the reference the glow normalizes against. See
    /// `emphasis_stats` in chaos.wgsl.
    pub stats_buffer: wgpu::Buffer,
    stats_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// Which transform the emphasis is measuring, and `written_total` when it
    /// was chosen.
    emphasis: Option<(u32, u64)>,
    /// `--isolate`: the points alone that are drawn.
    isolate: Option<Isolate>,
    /// Infinite-zoom renormalization applied to every emitted point, if the
    /// scene asked for one (see `renorm.rs`). Rebuilt on transform edits.
    pub zoom: Option<crate::renorm::Renorm>,
    /// What the walkers were seeded from. Kept so `reset` restarts the same
    /// stream rather than silently falling back to the default.
    pub seed: u64,
    /// The layer table (see [`LayerTable`]) and the per-layer colormaps a
    /// composite resolves its colour through.
    pub layer_buffer: wgpu::Buffer,
    pub layer_colormap_buffer: wgpu::Buffer,
    /// The xaos row table (`LayerTable::xaos_rows`). Sized to the scene and
    /// grown — with a bind-group rebuild — only when an edit outgrows it.
    xaos_buffer: wgpu::Buffer,
    num_layers: u32,
    composite: bool,
    /// Scene index -> GPU index, for the emphasis target.
    gpu_order: Vec<usize>,
    /// Mute/solo (see `mask.rs`): which map placed each point — a stand-in
    /// until something is first muted or soloed — and the mask the renderers
    /// clip against, with what was last written to it.
    map_buffer: wgpu::Buffer,
    mask_buffer: wgpu::Buffer,
    mask_words: Vec<u32>,
    mask_generation: u64,
    /// `written_total` when the map record was allocated: until a lap has
    /// been written since, some points have no map recorded.
    maps_since: Option<u64>,
}

fn create_mask_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("map_mask"),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The xaos row buffer, with headroom: a live edit that adds an exception or
/// two should be a write, not a bind-group rebuild.
fn create_xaos_buffer(device: &wgpu::Device, rows: &[[f32; 4]]) -> wgpu::Buffer {
    let mut contents = rows.to_vec();
    contents.resize((rows.len() * 2).max(64), [0.0; 4]);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("xaos_rows"),
        contents: bytemuck::cast_slice(&contents),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

/// A transform's colour, or white if the parallel `colors` array is short.
/// It can be, briefly: adding a transform pushes onto `transforms` and
/// `colors` separately, and a redraw can land between the two.
fn transform_color(colors: &[glam::Vec3], i: usize) -> glam::Vec3 {
    colors.get(i).copied().unwrap_or(glam::Vec3::ONE)
}

/// Slots in the symmetry group-element buffer.
///
/// Fixed capacity, allocated once, deliberately. The alternative — sizing the
/// buffer to the scene — makes every symmetry edit a bind-group rebuild, and
/// changing `C5` to `I` is an edit you make *by dragging a picker*, so it has to
/// be as cheap as moving a slider. At 64 bytes an element the whole table is
/// 32 KB, which is nothing next to a point buffer measured in hundreds of
/// megabytes, and it holds the largest group this can name (`I` with a mirror,
/// 120) four times over.
pub const MAX_GROUP_ELEMENTS: usize = 512;

/// Every distinct symmetry group in a scene, laid end to end, with each
/// transform's slice into it.
///
/// Distinct is the operative word: three motifs enrolled in one icosahedral
/// group share one 60-element run, so the table is sized by how many *different*
/// groups a scene uses, not by how many maps carry one.
pub struct GroupTable {
    /// The elements, concatenated. Never longer than [`MAX_GROUP_ELEMENTS`].
    pub elements: Vec<[[f32; 4]; 4]>,
    /// `(start, count)` per transform, parallel to the transform list.
    /// `(0, 0)` for a transform with no symmetry.
    pub slices: Vec<(u32, u32)>,
}

impl GroupTable {
    /// Collect the groups a transform list carries.
    pub fn build(transforms: &[TransformSpec]) -> Self {
        let mut elements: Vec<[[f32; 4]; 4]> = Vec::new();
        let mut slices = Vec::with_capacity(transforms.len());
        // Small and linear: a scene has a handful of distinct groups at most,
        // and comparing two `Symmetry`s is four field comparisons (the elements
        // are a pure function of them, so they never have to be walked).
        let mut seen: Vec<(&crate::symmetry::Symmetry, (u32, u32))> = Vec::new();

        for t in transforms {
            let Some(sym) = t.symmetry.as_ref() else {
                slices.push((0, 0));
                continue;
            };
            if let Some((_, slice)) = seen.iter().find(|(s, _)| *s == sym) {
                slices.push(*slice);
                continue;
            }
            if elements.len() + sym.order() > MAX_GROUP_ELEMENTS {
                // Unreachable through the loader, which rejects a scene whose
                // groups don't fit; a live edit that somehow got here renders
                // that motif unsymmetrized rather than overrunning the buffer.
                log::warn!(
                    "symmetry group {} does not fit in the {}-element table; \
                     rendering this motif without it",
                    sym.label(),
                    MAX_GROUP_ELEMENTS
                );
                slices.push((0, 0));
                continue;
            }
            let slice = (elements.len() as u32, sym.order() as u32);
            elements.extend(sym.elements().iter().map(|m| m.to_cols_array_2d()));
            seen.push((sym, slice));
            slices.push(slice);
        }

        Self { elements, slices }
    }

    /// The buffer contents: the table padded out to the fixed capacity.
    /// Element 0 of an unused run is never read (`group_count` is 0), so the
    /// padding's value doesn't matter — but it must be *written*, or the first
    /// upload leaves the tail uninitialized.
    fn padded(&self) -> Vec<[[f32; 4]; 4]> {
        let mut all = self.elements.clone();
        all.resize(MAX_GROUP_ELEMENTS, glam::Mat4::IDENTITY.to_cols_array_2d());
        all
    }
}

/// The interactive walker count, in workgroups: 16,384 walkers.
pub const DEFAULT_WORKGROUPS: u32 = 64;

/// The chaos game's compiled pipelines, which depend on nothing but the device.
/// Build once per device and hand to [`PointCompute::with_pipelines`]; wgpu
/// handles are reference counted, so every instance shares one compile.
#[derive(Clone)]
pub struct ChaosPipelines {
    pipeline: wgpu::ComputePipeline,
    rewrap_pipeline: wgpu::ComputePipeline,
    stats_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl ChaosPipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point_chaos_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/points/chaos.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("point_compute_bind_group_layout"),
            entries: &[
                // Points output buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Transforms
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Walker states
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Params
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Symmetry group elements
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Per-point sensitivity (emphasis)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Emphasis normalization stats
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Layer table (subscenes)
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Per-layer colormaps, for a composite's colour resolution
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Xaos rows (see crate::xaos). The eighth storage binding in
                // this stage — exactly the WebGPU default limit, which the
                // device also asks the adapter to raise where it can.
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Which map placed each point, for mute/solo (see mask.rs).
                // The ninth storage binding here: past the WebGPU default of
                // eight, so it relies on the device asking for the adapter's.
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_compute_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point_compute_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let rewrap_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point_rewrap_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("rewrap"),
            compilation_options: Default::default(),
            cache: None,
        });

        let stats_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("emphasis_stats_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("emphasis_stats"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self { pipeline, rewrap_pipeline, stats_pipeline, bind_group_layout }
    }
}

impl PointCompute {
    pub fn new(
        device: &wgpu::Device,
        table: &LayerTable,
        buffer_capacity: u32,
        seed: u64,
    ) -> Self {
        Self::with_pipelines(
            device,
            &ChaosPipelines::new(device),
            table,
            buffer_capacity,
            seed,
            DEFAULT_WORKGROUPS,
        )
    }

    /// [`PointCompute::new`], on pipelines compiled once and shared, and with
    /// `workgroups` × 256 walkers rather than the interactive 16,384.
    ///
    /// Compiling `chaos.wgsl` costs ~6 ms on the GTX 1080 (11 ms cold), and
    /// every instance used to pay it. That is a stall on every transform add,
    /// and it would be one per scene in a folder of thumbnails.
    pub fn with_pipelines(
        device: &wgpu::Device,
        pipelines: &ChaosPipelines,
        // The scene's maps, grouped by layer, with their colours and
        // colormaps. Per-transform RGB is uploaded whatever the mode: the
        // mode is a uniform flip at draw time, so the buffer has to be ready
        // for either.
        table: &LayerTable,
        buffer_capacity: u32,
        // Seeds the walkers. `DEFAULT_SEED` reproduces every render made before
        // this parameter existed.
        seed: u64,
        // Walkers, in workgroups of 256. Changing it changes walker seeding
        // and therefore the image, so everything that must match a render
        // uses `DEFAULT_WORKGROUPS`.
        num_workgroups: u32,
    ) -> Self {
        // Calculate parallelism - aim for ~0.125% of buffer per frame (full cycle every 800 frames)
        let points_per_frame = (buffer_capacity / 800).max(1000);
        let num_walkers = num_workgroups * WORKGROUP_SIZE;
        let iterations_per_walker = ((points_per_frame + num_walkers - 1) / num_walkers).max(1);
        let actual_points_per_frame = num_walkers * iterations_per_walker;
        
        // Warmup uses 10x more iterations to fill buffer in ~80 frames instead of 800
        let warmup_iterations_per_walker = iterations_per_walker * 10;
        let warmup_points_per_frame = num_walkers * warmup_iterations_per_walker;
        let warmup_frames = (buffer_capacity + warmup_points_per_frame - 1) / warmup_points_per_frame;

        log::info!(
            "Point compute: {} walkers × {} iters = {} points/frame (warmup: {} iters, {} frames to fill)",
            num_walkers, iterations_per_walker, actual_points_per_frame, warmup_iterations_per_walker, warmup_frames
        );


        // Transform data, layer by layer, with per-layer cumulative weights
        let groups = &table.groups;
        let colormap: &[[f32; 4]; 256] =
            table.colormaps[..256].try_into().expect("layer 0's colormap");

        let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("transform_buffer"),
            contents: bytemuck::cast_slice(&table.gpu_transforms),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // The symmetry groups. Fixed capacity so that changing a group is a
        // buffer write rather than a pipeline rebuild — see MAX_GROUP_ELEMENTS.
        let group_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("symmetry_group_buffer"),
            contents: bytemuck::cast_slice(&groups.padded()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Initialize walker states with different seeds
        let walker_states: Vec<WalkerState> =
            (0..num_walkers).map(|i| WalkerState::new(walker_seed(seed, i))).collect();

        let walker_states_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("walker_states_buffer"),
            contents: bytemuck::cast_slice(&walker_states),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Parameters uniform
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_compute_params"),
            size: std::mem::size_of::<PointComputeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Point buffer - the main output
        let point_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_buffer"),
            size: (buffer_capacity as usize * std::mem::size_of::<Point>()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // The emphasis buffers. The big one is not allocated until something
        // asks for it (see `set_emphasis`); until then this stand-in keeps the
        // binding legal, and the shader never writes through it.
        let sensitivity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_sensitivity_placeholder"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let map_buffer = super::mask::stand_in(device, "point_maps_placeholder");
        let mask_buffer = create_mask_buffer(device, 16);
        let stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("emphasis_stats_buffer"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Colormap buffer
        let colormap_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("colormap_buffer"),
            contents: bytemuck::cast_slice(colormap),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let layer_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("layer_table"),
            contents: bytemuck::cast_slice(&table.gpu_layers(num_walkers)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layer_colormap_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("layer_colormaps"),
            contents: bytemuck::cast_slice(&table.colormaps),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let xaos_buffer = create_xaos_buffer(device, &table.xaos_rows);

        // Bind group layout
        let ChaosPipelines { pipeline, rewrap_pipeline, stats_pipeline, bind_group_layout } =
            pipelines.clone();

        // Every buffer but the sensitivity one is fixed for the lifetime of the
        // pipeline, so the bind group is built once here and only rebuilt when
        // the emphasis swaps the stand-in for the real thing.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_compute_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: point_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: transform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: walker_states_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: group_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: sensitivity_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: stats_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: layer_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: layer_colormap_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: xaos_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: map_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline,
            rewrap_pipeline,
            bind_group,
            transform_buffer,
            group_buffer,
            walker_states_buffer,
            params_buffer,
            point_buffer,
            colormap_buffer,
            num_transforms: table.gpu_transforms.len() as u32,
            num_walkers,
            num_workgroups,
            iterations_per_walker,
            warmup_iterations_per_walker,
            buffer_capacity,
            write_offset: 0,
            warmup_frames,
            current_frame: 0,
            written_total: 0,
            sensitivity_buffer,
            stats_buffer,
            stats_pipeline,
            bind_group_layout,
            emphasis: None,
            isolate: None,
            zoom: None,
            seed,
            layer_buffer,
            layer_colormap_buffer,
            xaos_buffer,
            num_layers: table.layers.len() as u32,
            composite: table.composite,
            gpu_order: table.order.clone(),
            map_buffer,
            mask_buffer,
            mask_words: vec![0],
            // Unique even for the stand-ins, so a renderer that outlives one
            // compute always moves over to the next one's pair.
            mask_generation: super::mask::next_generation(),
            maps_since: None,
        }
    }

    /// Whether we're still in warmup phase (buffer not yet full)
    pub fn is_warming_up(&self) -> bool {
        self.current_frame < self.warmup_frames
    }


    /// Returns number of valid points to render this frame
    pub fn valid_point_count(&self) -> u32 {
        if self.is_warming_up() {
            // During warmup, calculate total written accounting for warmup rate
            let total_written = self.current_frame * self.num_walkers * self.warmup_iterations_per_walker;
            total_written.min(self.buffer_capacity)
        } else {
            // After warmup, buffer is always full
            self.buffer_capacity
        }
    }

    /// Update for next frame - advances write offset and returns valid point
    /// count. `dt` is the wall-clock frame time: after warmup, the churn rate
    /// (points regenerated per second) is normalized to it, so a 120 Hz
    /// display refreshes the buffer at the same real-time rate as 60 Hz
    /// (the historical baseline: `iterations_per_walker` per 1/60 s).
    pub fn advance_frame(&mut self, queue: &wgpu::Queue, dt: f32) -> u32 {
        let current_iters = if self.is_warming_up() {
            // Warmup ignores dt: fill the buffer as fast as possible
            self.warmup_iterations_per_walker
        } else {
            let target = self.iterations_per_walker as f32 * (dt * 60.0);
            (target.round() as u32).clamp(1, self.warmup_iterations_per_walker)
        };
        self.advance_with_iters(queue, current_iters)
    }

    /// Advance one dispatch at the *fill* rate — the one warmup uses — however
    /// many frames have gone by.
    ///
    /// The interactive rate exists to make the ring churn at a watchable speed;
    /// an accumulating render has no such constraint and wants throughput, so
    /// it stays at the fill rate forever. Ten times the points per dispatch,
    /// and `frames_per_lap` counts in these.
    pub fn advance_fill(&mut self, queue: &wgpu::Queue) -> u32 {
        self.advance_with_iters(queue, self.warmup_iterations_per_walker)
    }

    pub fn advance_with_iters(&mut self, queue: &wgpu::Queue, current_iters: u32) -> u32 {
        let pending = self.num_walkers * current_iters;
        // Counting this dispatch, because the chaos pass is encoded before
        // anything reads a sensitivity and the points it is about to write are
        // as much part of the run as the ones already there.
        let (run_start, run_len) = self.emphasis_run(pending);

        // Upload params with current write offset and iteration count
        let params = PointComputeParams {
            num_transforms: self.num_transforms,
            num_walkers: self.num_walkers,
            iterations_per_walker: current_iters,
            write_offset: self.write_offset,
            buffer_capacity: self.buffer_capacity,
            emphasis_enabled: u32::from(self.emphasis.is_some()),
            // The emphasis is chosen by scene index; the buffer is in layer order.
            emphasis_target: self.emphasis.map_or(0, |(t, _)| {
                self.gpu_order.iter().position(|&j| j == t as usize).unwrap_or(0) as u32
            }),
            num_layers: self.num_layers,
            composite: u32::from(self.composite),
            // Chosen by scene index, like the emphasis; stored one up so that
            // zero can mean "off".
            isolate: self.isolate.map_or(0, |iso| {
                // Low half: the map, one up. High half: the map that must have
                // run just before it, one up, or 0 for any. See `Isolate`.
                let gpu = |t: usize| {
                    self.gpu_order.iter().position(|&j| j == t).map_or(0xFFFF, |k| (k as u32 + 1).min(0xFFFF))
                };
                gpu(iso.map) | iso.after.map_or(0, |a| gpu(a) << 16)
            }),
            emphasis_run_start: run_start,
            emphasis_run_len: run_len,
            ..PointComputeParams::zeroed()
        }
        .with_zoom(self.zoom.as_ref());
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        // Advance for next frame
        self.write_offset = (self.write_offset + pending) % self.buffer_capacity;
        self.written_total += pending as u64;
        self.current_frame += 1;

        self.valid_point_count()
    }

    /// Which transform the emphasis is measuring, if any.
    pub fn emphasis_target(&self) -> Option<u32> {
        self.emphasis.map(|(t, _)| t)
    }

    /// Start and length of the ring's run of points that belong to the current
    /// emphasis target — what the glow pass is allowed to draw. Empty when the
    /// emphasis is off.
    pub fn emphasis_window(&self) -> (u32, u32) {
        self.emphasis_run(0)
    }

    fn emphasis_run(&self, pending: u32) -> (u32, u32) {
        let Some((_, mark)) = self.emphasis else {
            return (0, 0);
        };
        let written = (self.written_total + pending as u64).saturating_sub(mark);
        let len = written.min(self.buffer_capacity as u64) as u32;
        ((mark % self.buffer_capacity as u64) as u32, len)
    }

    /// Keep only the points one transform placed last, or every point again.
    ///
    /// The rest are still computed and still written — the walk is unchanged,
    /// so the kept points are exactly the ones the full picture has — but at a
    /// position every point shader clips. Points already in the ring are not
    /// touched, so set this before filling it.
    pub fn set_isolate(&mut self, target: Option<Isolate>) {
        self.isolate = target;
    }

    /// Point the emphasis at a transform, or turn it off.
    ///
    /// Changing the target does not disturb the picture: the points already in
    /// the ring are still the same points, they just no longer carry a
    /// sensitivity anyone may read. So the run resets to empty and the glow
    /// grows back in as the ring churns — which is honest, and much better than
    /// a second of glow that belongs to the transform you just stopped looking
    /// at. Callers that want it back quickly should churn at the fill rate.
    pub fn set_emphasis(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: Option<u32>) {
        if self.emphasis_target() == target {
            return;
        }
        match target {
            None => self.emphasis = None,
            Some(t) => {
                self.ensure_sensitivity_buffer(device);
                self.emphasis = Some((t, self.written_total));
                // The reference described a population that is no longer being
                // measured; seeding it again is a frame's work.
                queue.write_buffer(&self.stats_buffer, 0, bytemuck::cast_slice(&[0.0f32; 4]));
            }
        }
    }

    /// Allocate the real per-point sensitivity buffer, once, and rebind.
    fn ensure_sensitivity_buffer(&mut self, device: &wgpu::Device) {
        let wanted = self.buffer_capacity as u64 * 4;
        if self.sensitivity_buffer.size() >= wanted {
            return;
        }
        log::info!(
            "Emphasis: allocating {:.1} MB of per-point sensitivity",
            wanted as f64 / (1024.0 * 1024.0)
        );
        self.sensitivity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_sensitivity_buffer"),
            size: wanted,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        self.rebuild_bind_group(device);
    }

    /// Mute/solo: hide the points these maps placed (by scene index), or
    /// nothing with `None`. Cheap to call every frame: the mask is only
    /// written when it changes, and the per-point record is allocated once,
    /// the first time anything is hidden.
    pub fn set_hidden(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hidden: Option<&[bool]>) {
        let words = super::mask::mask_words(hidden, &self.gpu_order);
        if words == self.mask_words {
            return;
        }
        if hidden.is_some() {
            self.ensure_map_buffer(device);
        }
        let bytes = (words.len() * 4) as u64;
        if self.mask_buffer.size() < bytes {
            self.mask_buffer = create_mask_buffer(device, bytes.next_power_of_two());
            self.mask_generation = super::mask::next_generation();
        }
        queue.write_buffer(&self.mask_buffer, 0, bytemuck::cast_slice(&words));
        self.mask_words = words;
    }

    /// What the renderers bind to clip against the mute/solo mask.
    pub fn mask_binding(&self) -> super::mask::Binding {
        super::mask::Binding {
            maps: self.map_buffer.clone(),
            mask: self.mask_buffer.clone(),
            generation: self.mask_generation,
        }
    }

    /// Allocate the real per-point map record, once, and rebind.
    fn ensure_map_buffer(&mut self, device: &wgpu::Device) {
        let wanted = self.buffer_capacity as u64 * 4;
        if self.map_buffer.size() >= wanted {
            return;
        }
        log::info!("Mute/solo: allocating {:.1} MB of per-point map record", wanted as f64 / (1024.0 * 1024.0));
        self.map_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_maps"),
            size: wanted,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        self.mask_generation = super::mask::next_generation();
        self.maps_since = Some(self.written_total);
        self.rebuild_bind_group(device);
    }

    /// Whether the mask is on and the ring still holds points written before
    /// the map record existed — hidden until they are replaced, so callers
    /// should churn at the fill rate meanwhile, as for the emphasis.
    pub fn mask_filling(&self) -> bool {
        self.mask_words[0] != 0
            && self.maps_since.is_some_and(|m| self.written_total - m < self.buffer_capacity as u64)
    }

    /// After a buffer the bind group holds has been replaced.
    fn rebuild_bind_group(&mut self, device: &wgpu::Device) {
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_compute_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.point_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.transform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.walker_states_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: self.group_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: self.sensitivity_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: self.stats_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: self.layer_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: self.layer_colormap_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 9, resource: self.xaos_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 10, resource: self.map_buffer.as_entire_binding() },
            ],
        });
    }

    /// Re-measure the reference the glow normalizes against. One workgroup,
    /// once a frame, and only while the emphasis is on.
    pub fn dispatch_emphasis_stats(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.emphasis.is_none() {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("emphasis_stats_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.stats_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    /// Dispatches needed to replace every point in the ring once, at the fill
    /// rate `advance_frame` uses during warmup.
    ///
    /// This is the accumulating render's batch: it folds the histogram once per
    /// *lap* of the ring rather than once per dispatch, so each sample is
    /// counted exactly once and the cost of the fold — a compute pass over every
    /// texel of the accumulation — is amortized over a whole buffer of points
    /// instead of a eightieth of one.
    pub fn frames_per_lap(&self) -> u32 {
        self.warmup_frames.max(1)
    }

    /// Carry the whole point buffer through `levels` zoom periods, so the
    /// points move with a camera that just wrapped and every dot keeps its
    /// pixel. See `rewrap` in `shaders/points/chaos.wgsl` for why.
    ///
    /// Submits on its own rather than joining the frame's encoder: a wrap
    /// happens once a period — every several seconds, at the frame rates this
    /// is watched at — and the alternative is threading a pending flag through
    /// the update/render split for something that costs one pass over the
    /// buffer. It must land *before* the frame's chaos dispatch, which writes
    /// fresh points into the band this pass is re-folding, and queue order
    /// gives that for free.
    ///
    /// A no-op without a zoom, or when nothing wrapped.
    pub fn rewrap(&self, device: &wgpu::Device, queue: &wgpu::Queue, levels: i32) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("point_rewrap_encoder"),
        });
        self.rewrap_in(queue, &mut encoder, levels);
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// The same, recorded into a caller's encoder. The window uses this: the
    /// pass belongs to the frame that wrapped, and a submit of its own would
    /// put a queue flush in the middle of it.
    pub fn rewrap_in(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        levels: i32,
    ) {
        if levels == 0 || self.zoom.is_none() {
            return;
        }
        // One thread per point, dispatched as a 2D grid. A dispatch dimension
        // stops at 65,535 workgroups, and a 100M point buffer is 390,625 of
        // them — so `--effort ultra` on a zoom scene used to die inside wgpu
        // with a number the user never typed.
        let groups = self.buffer_capacity.div_ceil(WORKGROUP_SIZE);
        let groups_x = groups.min(MAX_WORKGROUPS_PER_DIM);
        let groups_y = groups.div_ceil(groups_x);
        let params = PointComputeParams {
            num_transforms: self.num_transforms,
            num_walkers: self.num_walkers,
            buffer_capacity: self.buffer_capacity,
            rewrap_stride: groups_x * WORKGROUP_SIZE,
            ..PointComputeParams::zeroed()
        }
        .with_zoom(self.zoom.as_ref());
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&PointComputeParams { zoom_rewrap: levels as f32, ..params }),
        );

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("point_rewrap_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.rewrap_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(groups_x, groups_y, 1);
    }

    /// Dispatch the compute shader
    pub fn dispatch(&self, encoder: &mut wgpu::CommandEncoder) {
        self.dispatch_timed(encoder, None);
    }

    /// The same, optionally measured.
    ///
    /// A separate entry point rather than a parameter on `dispatch` because
    /// every live-window caller wants the unmeasured one and should not have to
    /// say so — timing is an offline investigation, not part of the frame.
    pub fn dispatch_timed(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&mut crate::gpu::GpuTimer>,
    ) {
        let timestamp_writes = timer.and_then(|t| t.compute_pass());
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("point_chaos_pass"),
            timestamp_writes,
        });
        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, &self.bind_group, &[]);
        compute_pass.dispatch_workgroups(self.num_workgroups, 1, 1);
    }

    /// Reupload transform weights with some transforms disabled.
    ///
    /// Also re-uploads the symmetry groups, because a live edit can change one
    /// (the panel's group picker) and there is no cheaper place to notice: the
    /// table is 32 KB and this already runs only on edits, never per frame.
    ///
    /// The xaos rows too, which change size under editing (cutting an edge
    /// adds an exception), so this is the one write that may need the device:
    /// a table that outgrows its buffer gets a new one.
    pub fn update_weights(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, table: &LayerTable) {
        let xaos_bytes = std::mem::size_of_val(table.xaos_rows.as_slice()) as u64;
        if xaos_bytes > self.xaos_buffer.size() {
            self.xaos_buffer = create_xaos_buffer(device, &table.xaos_rows);
            self.rebuild_bind_group(device);
        } else {
            queue.write_buffer(&self.xaos_buffer, 0, bytemuck::cast_slice(&table.xaos_rows));
        }
        queue.write_buffer(&self.transform_buffer, 0, bytemuck::cast_slice(&table.gpu_transforms));
        queue.write_buffer(&self.group_buffer, 0, bytemuck::cast_slice(&table.groups.padded()));
        queue.write_buffer(&self.layer_buffer, 0, bytemuck::cast_slice(&table.gpu_layers(self.num_walkers)));
        queue.write_buffer(&self.layer_colormap_buffer, 0, bytemuck::cast_slice(&table.colormaps));
        self.num_layers = table.layers.len() as u32;
        self.composite = table.composite;
        self.gpu_order = table.order.clone();
    }

    /// Re-upload the colormap after live color edits. Existing points keep
    /// their colormap indices, so they recolor immediately.
    pub fn update_colormap(&self, queue: &wgpu::Queue, colormap: &[[f32; 4]; 256]) {
        queue.write_buffer(&self.colormap_buffer, 0, bytemuck::cast_slice(colormap));
    }

    /// Reset to initial state (for scene reload or reset key)
    pub fn reset(&mut self, queue: &wgpu::Queue) {
        self.write_offset = 0;
        self.current_frame = 0;
        self.written_total = 0;
        // Every point the emphasis had measured is about to be overwritten.
        if let Some((t, _)) = self.emphasis {
            self.emphasis = Some((t, 0));
            queue.write_buffer(&self.stats_buffer, 0, bytemuck::cast_slice(&[0.0f32; 4]));
        }

        // Re-initialize walker states, from the same seed: a reset restarts
        // the *same* stream, which is what makes a rebuild reproducible.
        let walker_states: Vec<WalkerState> =
            (0..self.num_walkers).map(|i| WalkerState::new(walker_seed(self.seed, i))).collect();
        queue.write_buffer(&self.walker_states_buffer, 0, bytemuck::cast_slice(&walker_states));
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    /// Zero laps means "not a resume", and must leave the stream alone or
    /// every ordinary render changes.
    #[test]
    fn resuming_from_nothing_does_not_move_the_seed() {
        for seed in [DEFAULT_SEED, 1, 7, u64::MAX] {
            assert_eq!(resume_seed(seed, 0), seed);
        }
    }

    /// The point of the function: each resume deals a different hand, or the
    /// new laps re-deposit samples the histogram already has.
    #[test]
    fn each_resume_deals_a_different_hand() {
        let mut seen = std::collections::HashSet::new();
        for laps in 0..64u32 {
            assert!(seen.insert(resume_seed(DEFAULT_SEED, laps)), "lap {} collided", laps);
        }
    }

    /// And a resumed lap must not collide with a *walker* of a fresh run,
    /// which is why the constant differs from WALKER_STRIDE.
    #[test]
    fn a_resumed_lap_does_not_collide_with_a_walker() {
        let fresh: std::collections::HashSet<u64> =
            (0..4096u32).map(|i| walker_seed(DEFAULT_SEED, i)).collect();
        for laps in 1..64u32 {
            let s = resume_seed(DEFAULT_SEED, laps);
            for i in 0..64u32 {
                assert!(!fresh.contains(&walker_seed(s, i)),
                    "resume {} walker {} collides with a fresh walker", laps, i);
            }
        }
    }

    /// Deterministic: the same checkpoint resumed twice is the same picture.
    #[test]
    fn resuming_is_reproducible() {
        assert_eq!(resume_seed(9, 5), resume_seed(9, 5));
    }

    
    /// The default has to be a no-op, or every render in the project changes
    /// the day a seed parameter is added.
    #[test]
    fn the_default_seed_reproduces_the_implicit_one() {
        for i in [0u32, 1, 7, 16383, 65535] {
            assert_eq!(
                walker_seed(DEFAULT_SEED, i),
                (i as u64).wrapping_mul(0x9E3779B97F4A7C15),
                "walker {}",
                i
            );
        }
    }

    /// And a non-default seed has to actually be a different stream — the
    /// whole point is that re-running with the same seed re-counts the same
    /// samples, making the picture brighter rather than better.
    #[test]
    fn different_seeds_give_different_streams() {
        let a: Vec<u64> = (0..64).map(|i| walker_seed(1, i)).collect();
        let b: Vec<u64> = (0..64).map(|i| walker_seed(2, i)).collect();
        let d: Vec<u64> = (0..64).map(|i| walker_seed(DEFAULT_SEED, i)).collect();
        assert_ne!(a, b);
        assert_ne!(a, d);
        // Adjacent seeds must not merely shift the same sequence along
        assert!(a.iter().all(|x| !b.contains(x)), "seeds 1 and 2 share walker states");
    }

    /// A walker whose RNG words are all zero never advances. The seeding must
    /// not be able to produce one, whatever seed it is handed.
    #[test]
    fn no_seed_produces_a_dead_walker() {
        for seed in [0u64, 1, u64::MAX, 0x9E3779B97F4A7C15] {
            for i in [0u32, 1, 12345] {
                let w = WalkerState::new(walker_seed(seed, i));
                assert!(w.rng_state.iter().any(|&s| s != 0), "seed {} walker {}", seed, i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_walker_state_size() {
        assert_eq!(std::mem::size_of::<WalkerState>(), 64, "WalkerState must be 64 bytes to match WGSL struct");
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    /// `GpuLayer` and chaos.wgsl's `Layer` are one struct in two languages,
    /// and the array length is one number in two languages. WGSL is opaque to
    /// the compiler, so this is what holds them together.
    #[test]
    fn the_shader_agrees_about_the_layer_table() {
        assert_eq!(std::mem::size_of::<GpuLayer>(), 96);
        let src = include_str!("../../../shaders/points/chaos.wgsl");
        let decl = format!("var<uniform> layers: array<Layer, {}>", crate::subscene::MAX_LAYERS);
        assert!(src.contains(&decl), "chaos.wgsl must declare `{decl}`");
    }

    fn composite() -> crate::scene::Scene {
        let mut s = crate::scene::Scene::blank();
        let mut sub = s.transforms.clone();
        for t in &mut sub {
            t.layer = 1;
        }
        // Interleave, to check the table groups by layer whatever the order.
        s.transforms = vec![sub[0].clone(), s.transforms[0].clone(), sub[1].clone(), s.transforms[1].clone()];
        s.colors = vec![glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z, glam::Vec3::ONE];
        s.transform_names = vec![None; 4];
        s.subscenes.push(crate::subscene::Subscene {
            name: "b".into(),
            author: None,
            file: None,
            file_abs: None,
            base: None,
            source: None,
            visible: true,
            share: 3.0,
            placement: crate::subscene::Placement::IDENTITY,
            colour: crate::subscene::LayerColour {
                color_mode: crate::scene::ColorMode::Mix,
                palette: None,
                color_speed: 0.5,
                color_falloff: 0.0,
                color_contrast: 1.0,
                colormap: s.colormap,
            },
        });
        s
    }

    #[test]
    fn layers_are_contiguous_with_their_own_cumulative_weights() {
        let s = composite();
        let t = LayerTable::build(&s, &[], 1.0);
        assert!(t.composite);
        assert_eq!(t.order, vec![1, 3, 0, 2]);
        let cum: Vec<f32> = t.gpu_transforms.iter().map(|g| g.cumulative_weight).collect();
        assert_eq!(cum, vec![0.5, 1.0, 0.5, 1.0]);
        let g = t.gpu_layers(16384);
        assert_eq!((g[0].t_start, g[0].t_count, g[0].walker_end), (0, 2, 4096));
        assert_eq!((g[1].t_start, g[1].t_count, g[1].walker_end), (2, 2, 16384));
        assert_eq!(g[1].mix, 1);
    }

    #[test]
    fn a_hidden_or_silenced_layer_gives_its_walkers_away() {
        let mut s = composite();
        s.subscenes[0].visible = false;
        let g = LayerTable::build(&s, &[], 1.0).gpu_layers(100);
        assert_eq!((g[0].walker_end, g[1].walker_end), (100, 100));
        let s = composite();
        // Both of layer 0's maps switched off.
        let g = LayerTable::build(&s, &[true, false, true, false], 1.0).gpu_layers(100);
        assert_eq!((g[0].walker_end, g[1].walker_end), (0, 100));
    }

    #[test]
    fn an_ordinary_scene_is_one_layer_with_everything() {
        let s = crate::scene::Scene::blank();
        let t = LayerTable::build(&s, &[false, false], 1.0);
        assert!(!t.composite);
        let g = t.gpu_layers(16384);
        assert_eq!((g[0].t_start, g[0].t_count, g[0].walker_end), (0, 2, 16384));
    }
}
