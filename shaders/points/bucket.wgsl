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
    /// Points per footprint the coverage rule aims for, the frame's area in
    /// accumulation pixels, and the largest footprint radius (accumulation
    /// pixels): see `scan`.
    k_cov: f32,
    area: f32,
    max_radius: f32,
    /// Footprint radius forced for a measurement, or 0.
    force_radius: f32,
}

@group(0) @binding(0) var<storage, read> points_in: array<Point>;
@group(0) @binding(1) var<uniform> camera: CameraUniforms;
@group(0) @binding(2) var<storage, read_write> points_out: array<Point>;
@group(0) @binding(3) var<storage, read_write> counts: array<atomic<u32>>;
@group(0) @binding(4) var<uniform> params: SortParams;
// Per bucket, two indirect draws of 4 words: the 1px primitives, then the quads
// (six vertices a point, as triangles). One of them is empty.
@group(0) @binding(5) var<storage, read_write> draw_args: array<u32>;
// The grid, then each bucket's footprint radius: see `scan`.
@group(0) @binding(6) var<storage, read_write> info: array<f32>;

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
    let fu = x * 0.5 + 0.5;
    let fv = 0.5 - y * 0.5;
    // Off screen, give or take the footprint of a point just past the edge, goes
    // in the last bucket: it is not drawn (or barely), and it must not count as
    // part of the sample the picture is made of.
    if fu < -0.03 || fu > 1.03 || fv < -0.03 || fv > 1.03 {
        return last;
    }
    let u = clamp(fu, 0.0, 0.99999);
    let v = clamp(fv, 0.0, 0.99999);
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
//
// It also decides how big the points are drawn. A point of the attractor has no
// size; a footprint exists to turn a finite sample into an estimate of a
// continuous measure, and the footprint that does that is the one that covers a
// point's share of the screen. A bucket with `n` points over `a` pixels gets a
// radius of sqrt(k * a / (pi * n)): `k` points per footprint, so 1 is as many
// footprints as it takes to tile the bucket and 3 leaves a twentieth of it
// uncovered by chance (e^-k, for a random sample). Local, because a sample is
// anything but uniform: the dense core of an attractor wants single pixels while
// its sparse fringe, a few buckets away, wants discs. Zooming in until most of
// the sample is off screen grows every footprint and the picture keeps its
// coverage, and the fragments a pass shades come to about `k` a pixel whatever
// the zoom, which bounds the cost.
//
// A bucket at or under a pixel is drawn as native 1px primitives, the fast path
// (a radius of 0.6 covers about one pixel; 1 is about three): each bucket gets
// two indirect draws, 16 bytes each, one of them empty, over its stretch of the
// sorted points. `info` holds the grid (words 0 and 1) then each bucket's radius
// in accumulation pixels, for the vertex stage, which interpolates between the
// buckets' centres by position so that the footprint does not jump at a bucket's
// edge.
// The last bucket, off screen, is not drawn.
// The radii are smoothed over each bucket's neighbours (in the log, a few passes
// of a 3x3 average) before they are used, because a footprint that changes
// fourfold at a bucket's edge shows as a visible rectangle.
var<private> raw_radius: array<f32, 1025>;
var<private> blurred: array<f32, 1025>;

@compute @workgroup_size(1)
fn scan() {
    let nbx = params.nbx;
    let nby = params.nby;
    let buckets = nbx * nby + 1u;
    let tile_area = params.area / f32(nbx * nby);
    info[0] = f32(nbx);
    info[1] = f32(nby);
    var running = 0u;
    for (var b = 0u; b < buckets; b += 1u) {
        let c = atomicLoad(&counts[b]);
        atomicStore(&counts[b], running);
        var radius = sqrt(params.k_cov * tile_area / (3.14159265 * f32(max(c, 1u))));
        if params.force_radius > 0.0 {
            radius = params.force_radius;
        }
        raw_radius[b] = log(min(radius, params.max_radius));
        running += c;
    }
    // Smooth (the forced radius is already uniform).
    for (var round = 0u; round < 2u; round += 1u) {
        for (var y = 0u; y < nby; y += 1u) {
            for (var x = 0u; x < nbx; x += 1u) {
                var sum = 0.0;
                var n = 0.0;
                for (var dy = -1; dy <= 1; dy += 1) {
                    for (var dx = -1; dx <= 1; dx += 1) {
                        let qx = i32(x) + dx;
                        let qy = i32(y) + dy;
                        if qx >= 0 && qy >= 0 && qx < i32(nbx) && qy < i32(nby) {
                            sum += raw_radius[u32(qy) * nbx + u32(qx)];
                            n += 1.0;
                        }
                    }
                }
                blurred[y * nbx + x] = sum / n;
            }
        }
        for (var i = 0u; i < nbx * nby; i += 1u) {
            raw_radius[i] = blurred[i];
        }
    }
    // Start offsets are in `counts` now; the counts are their differences.
    for (var b = 0u; b < buckets; b += 1u) {
        let start = atomicLoad(&counts[b]);
        var end = running;
        if b + 1u < buckets {
            end = atomicLoad(&counts[b + 1u]);
        }
        let c = end - start;
        let radius = exp(raw_radius[b]);
        let quads = radius > 0.6;
        info[2u + b] = radius;
        let live = select(c, 0u, b == buckets - 1u);
        let at = b * 8u;
        draw_args[at] = select(live, 0u, quads);
        draw_args[at + 1u] = 1u;
        draw_args[at + 2u] = start;
        draw_args[at + 3u] = 0u;
        draw_args[at + 4u] = 6u * select(0u, live, quads);
        draw_args[at + 5u] = 1u;
        draw_args[at + 6u] = 6u * start;
        draw_args[at + 7u] = 0u;
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
