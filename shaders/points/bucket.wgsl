// Spatial bucket sort of the point buffer, for volume mode.
//
// The ring is in chaos order, and a walker's consecutive points are anywhere in
// space, so drawing it in order sends each fragment of the volume passes to a
// random pixel of a frame-sized buffer of slices: every access a cache miss.
// Drawn in screen-tile order the same fragments hit memory a few thousand
// pixels at a time, and the slice and emission passes take half as long
// (nautilus, 8M points, 1280x720: 26 ms -> 9). What matters is the *coarse*
// order, not an exact one: tiles of 128x128 pixels beat 8x8 by 2x, because a
// fine sort puts neighbours of one pixel in the same warp and their atomics
// serialise. So this is a counting sort into ~60 buckets, not a sort.
//
// Three passes: `histogram` counts each bucket, `scan` turns the counts into
// start offsets, `scatter` writes every point to its bucket's next free slot.
// A workgroup handles a chunk of CHUNK points and reserves space for all of its
// points in a bucket with one global atomic, so the contended counters see
// one add per workgroup and bucket, not one per point.

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

// Only the leading fields are read; the layout matches `CameraUniforms` in
// splat.wgsl, and the host binds the same buffer.
struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    projected_point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    haze_near: f32,
    haze_far: f32,
    haze_transmittance: f32,
    haze_saturation: f32,
    color_contrast: f32,
    bg_r: f32,
    bg_g: f32,
    bg_b: f32,
    transparent: f32,
    color_rgb_mode: f32,
    guard_x: f32,
    guard_y: f32,
    guard_z: f32,
    guard_ln_near: f32,
    guard_inv_ln_width: f32,
    max_point_pixels: f32,
    subfrustum: vec4<f32>,
}

struct SortParams {
    /// Buckets across and down the frame; the last bucket (index nbx*nby) holds
    /// what is behind the eye or dropped.
    nbx: u32,
    nby: u32,
    count: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> points_in: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read_write> points_out: array<Point>;
@group(0) @binding(3) var<storage, read_write> counts: array<atomic<u32>>;
@group(0) @binding(4) var<uniform> params: SortParams;

const WG: u32 = 256u;
const PER_THREAD: u32 = 8u;
const CHUNK: u32 = 2048u; // WG * PER_THREAD
// Shared arrays are this long; the host never asks for more buckets.
const MAX_BUCKETS: u32 = 1025u;

var<workgroup> local_count: array<atomic<u32>, 1025>;
var<workgroup> local_base: array<u32, 1025>;

fn bucket_of(pos: vec3<f32>) -> u32 {
    let last = params.nbx * params.nby;
    // The same two clips every vertex shader here applies: behind the eye, and
    // `--isolate`'s sentinel.
    let clip = camera.view_proj * vec4<f32>(pos, 1.0);
    if clip.w <= 0.0 || abs(pos.x) > 1.0e36 {
        return last;
    }
    let ndc = clip.xy / clip.w;
    let x = ndc.x * camera.subfrustum.x + camera.subfrustum.z;
    let y = ndc.y * camera.subfrustum.y + camera.subfrustum.w;
    let u = clamp(x * 0.5 + 0.5, 0.0, 0.99999);
    let v = clamp(0.5 - y * 0.5, 0.0, 0.99999);
    return u32(v * f32(params.nby)) * params.nbx + u32(u * f32(params.nbx));
}

fn chunk_start(wg: vec3<u32>, num: vec3<u32>) -> u32 {
    return (wg.y * num.x + wg.x) * CHUNK;
}

@compute @workgroup_size(256)
fn histogram(
    @builtin(local_invocation_index) tid: u32,
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(num_workgroups) num: vec3<u32>,
) {
    let buckets = params.nbx * params.nby + 1u;
    for (var b = tid; b < buckets; b += WG) {
        atomicStore(&local_count[b], 0u);
    }
    workgroupBarrier();
    let start = chunk_start(wg, num);
    for (var k = 0u; k < PER_THREAD; k += 1u) {
        let i = start + k * WG + tid;
        if i < params.count {
            atomicAdd(&local_count[bucket_of(points_in[i].position)], 1u);
        }
    }
    workgroupBarrier();
    for (var b = tid; b < buckets; b += WG) {
        let c = atomicLoad(&local_count[b]);
        if c > 0u {
            atomicAdd(&counts[b], c);
        }
    }
}

// One thread: ~60 buckets (at most 1025) is nothing next to the passes around it.
@compute @workgroup_size(1)
fn scan() {
    let buckets = params.nbx * params.nby + 1u;
    var running = 0u;
    for (var b = 0u; b < buckets; b += 1u) {
        let c = atomicLoad(&counts[b]);
        atomicStore(&counts[b], running);
        running += c;
    }
}

@compute @workgroup_size(256)
fn scatter(
    @builtin(local_invocation_index) tid: u32,
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(num_workgroups) num: vec3<u32>,
) {
    let buckets = params.nbx * params.nby + 1u;
    for (var b = tid; b < buckets; b += WG) {
        atomicStore(&local_count[b], 0u);
    }
    workgroupBarrier();
    let start = chunk_start(wg, num);
    var keep: array<Point, 8>;
    var key: array<u32, 8>;
    var rank: array<u32, 8>;
    for (var k = 0u; k < PER_THREAD; k += 1u) {
        let i = start + k * WG + tid;
        if i < params.count {
            keep[k] = points_in[i];
            key[k] = bucket_of(keep[k].position);
            rank[k] = atomicAdd(&local_count[key[k]], 1u);
        }
    }
    workgroupBarrier();
    // One reservation per bucket this chunk has points in.
    for (var b = tid; b < buckets; b += WG) {
        let c = atomicLoad(&local_count[b]);
        if c > 0u {
            local_base[b] = atomicAdd(&counts[b], c);
        }
    }
    workgroupBarrier();
    for (var k = 0u; k < PER_THREAD; k += 1u) {
        let i = start + k * WG + tid;
        if i < params.count {
            points_out[local_base[key[k]] + rank[k]] = keep[k];
        }
    }
}
