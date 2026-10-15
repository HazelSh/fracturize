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
// The passes, in one compute pass: `histogram` counts the points in each screen
// bucket; `scan` turns those counts into a footprint radius for each bucket (how
// big to draw its points, see there); `classify_histogram` counts again, this
// time per bucket *and kind* (1px primitive or quad, chosen per point from the
// radius at its position and a hash of its index); `offsets` turns those counts
// into start offsets and writes the indirect draws; `scatter` writes every point
// to the next free slot of its class. A workgroup handles a chunk of CHUNK points
// and reserves space for all of its points in a class with one global atomic, so
// the contended counters see one add per workgroup and class, not one per point.

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
// Shared arrays are this long: two kinds for each of the most buckets the host
// asks for, plus the off-screen one. Keep in step with `MAX_CLASSES` in bucket.rs.
const MAX_CLASSES: u32 = 2049u;
// Where the per-class counts start in `counts`, after the per-bucket ones.
const CLASS0: u32 = 1025u;

var<workgroup> local_count: array<atomic<u32>, 2049>;
var<workgroup> local_base: array<u32, 2049>;

/// Where a point lands in the frame, as fractions of its width and height (y
/// down), and whether it is in the sample at all: not behind the eye, not
/// dropped by `--isolate`, and not off screen give or take the footprint of a
/// point just past the edge. What is not goes in the last bucket: it is not
/// drawn, and it must not count as part of the sample the picture is made of.
struct Where {
    fu: f32,
    fv: f32,
    on: bool,
}

fn where_is(pos: vec3<f32>) -> Where {
    let clip = camera.view_proj * vec4<f32>(pos, 1.0);
    if clip.w <= 0.0 || abs(pos.x) > 1.0e36 {
        return Where(0.0, 0.0, false);
    }
    let ndc = clip.xy / clip.w;
    let x = ndc.x * camera.subfrustum.x + camera.subfrustum.z;
    let y = ndc.y * camera.subfrustum.y + camera.subfrustum.w;
    let fu = x * 0.5 + 0.5;
    let fv = 0.5 - y * 0.5;
    return Where(fu, fv, !(fu < -0.03 || fu > 1.03 || fv < -0.03 || fv > 1.03));
}

fn bucket_at(w: Where) -> u32 {
    if !w.on {
        return params.nbx * params.nby;
    }
    let u = clamp(w.fu, 0.0, 0.99999);
    let v = clamp(w.fv, 0.0, 0.99999);
    return u32(v * f32(params.nby)) * params.nbx + u32(u * f32(params.nbx));
}

/// The footprint radius at a point of the frame, bilinear between the bucket
/// centres around it (the field `scan` leaves in `info`). The vertex stage
/// computes the same thing in `footprint_at`: keep them in step.
fn radius_at(fu: f32, fv: f32) -> f32 {
    let nbx = params.nbx;
    let nby = params.nby;
    if nbx < 2u || nby < 2u {
        return info[2];
    }
    let gu = clamp(fu, 0.0, 1.0) * f32(nbx) - 0.5;
    let gv = clamp(fv, 0.0, 1.0) * f32(nby) - 0.5;
    let i0 = clamp(i32(floor(gu)), 0, i32(nbx) - 2);
    let j0 = clamp(i32(floor(gv)), 0, i32(nby) - 2);
    let tu = clamp(gu - f32(i0), 0.0, 1.0);
    let tv = clamp(gv - f32(j0), 0.0, 1.0);
    let at = u32(j0) * nbx + u32(i0);
    let top = mix(info[2u + at], info[2u + at + 1u], tu);
    let bottom = mix(info[2u + at + nbx], info[2u + at + nbx + 1u], tu);
    return mix(top, bottom, tv);
}

fn hash01(i: u32) -> f32 {
    var x = i * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return f32(x) * (1.0 / 4294967296.0);
}

/// The class of the point at ring index `i`: its bucket's primitive class
/// (`2 * bucket`) or quad class (`2 * bucket + 1`), the off-screen bucket's single
/// class last. A radius under half a pixel draws every point as a primitive, over
/// 0.8 every one as a quad, and in between a share of them that grows with it, so
/// the picture changes from one to the other gradually and not along a line.
fn class_of(i: u32, w: Where) -> u32 {
    let last = params.nbx * params.nby;
    if !w.on {
        return 2u * last;
    }
    let r = radius_at(w.fu, w.fv);
    let quad = hash01(i) < clamp((r - 0.5) / 0.3, 0.0, 1.0);
    return 2u * bucket_at(w) + select(0u, 1u, quad);
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
            atomicAdd(&local_count[bucket_at(where_is(points_in[i].position))], 1u);
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

// The radii are smoothed over each bucket's neighbours (in the log, two rounds of
// a 3x3 average) because a footprint that changes fourfold at a bucket's edge
// shows as a visible rectangle.
var<private> raw_radius: array<f32, 1025>;
var<private> blurred: array<f32, 1025>;
var<private> valid: array<f32, 1025>;
var<private> blurred_valid: array<f32, 1025>;

// One thread: ~60 buckets (at most 1025) is nothing next to the passes around it.
//
// It decides how big the points are drawn. A point of the attractor has no size;
// a footprint exists to turn a finite sample into an estimate of a continuous
// measure, and the footprint that does that is the one that covers a point's
// share of the screen. A bucket with `n` points over `a` pixels gets a radius of
// sqrt(k * a / (pi * n)): `k` points per footprint, so 1 is as many footprints as
// it takes to tile the bucket and 3 leaves a twentieth of it uncovered by chance
// (e^-k, for a random sample). Local, because a sample is anything but uniform:
// the dense core of an attractor wants single pixels while its sparse fringe, a
// few buckets away, wants discs. Zooming in until most of the sample is off
// screen grows every footprint and the picture keeps its coverage, and the
// fragments a pass shades come to about `k` a pixel whatever the zoom, which
// bounds the cost. `info` gets the grid (words 0 and 1) and then each bucket's
// radius in accumulation pixels.
@compute @workgroup_size(1)
fn scan() {
    let nbx = params.nbx;
    let nby = params.nby;
    let tile_area = params.area / f32(nbx * nby);
    info[0] = f32(nbx);
    info[1] = f32(nby);
    for (var b = 0u; b < nbx * nby; b += 1u) {
        let c = atomicLoad(&counts[b]);
        var radius = sqrt(params.k_cov * tile_area / (3.14159265 * f32(max(c, 1u))));
        if params.force_radius > 0.0 {
            radius = params.force_radius;
        }
        raw_radius[b] = log(min(radius, params.max_radius));
        // An empty bucket has no density to speak of; it must not drag its
        // neighbours' footprints up (a compact attractor in a mostly empty frame
        // would be drawn all in discs), so the smoothing below leaves it out and
        // it takes the average of what is around it.
        valid[b] = select(0.0, 1.0, c > 0u);
    }
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
                            let q = u32(qy) * nbx + u32(qx);
                            sum += raw_radius[q] * valid[q];
                            n += valid[q];
                        }
                    }
                }
                let here = y * nbx + x;
                blurred[here] = select(raw_radius[here], sum / n, n > 0.0);
                blurred_valid[here] = select(valid[here], 1.0, n > 0.0);
            }
        }
        for (var i = 0u; i < nbx * nby; i += 1u) {
            raw_radius[i] = blurred[i];
            valid[i] = blurred_valid[i];
        }
    }
    for (var b = 0u; b < nbx * nby; b += 1u) {
        info[2u + b] = exp(raw_radius[b]);
    }
}

@compute @workgroup_size(256)
fn classify_histogram(
    @builtin(local_invocation_index) tid: u32,
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(num_workgroups) num: vec3<u32>,
) {
    let classes = 2u * params.nbx * params.nby + 1u;
    for (var c = tid; c < classes; c += WG) {
        atomicStore(&local_count[c], 0u);
    }
    workgroupBarrier();
    let start = chunk_start(wg, num);
    for (var k = 0u; k < PER_THREAD; k += 1u) {
        let i = start + k * WG + tid;
        if i < params.count {
            atomicAdd(&local_count[class_of(i, where_is(points_in[i].position))], 1u);
        }
    }
    workgroupBarrier();
    for (var c = tid; c < classes; c += WG) {
        let n = atomicLoad(&local_count[c]);
        if n > 0u {
            atomicAdd(&counts[CLASS0 + c], n);
        }
    }
}

// One thread: start offsets for the classes, and each bucket's two indirect draws
// over its stretch of the sorted points (16 bytes each: the primitives, then the
// quads at six vertices a point, as triangles). The last class, off screen, is
// not drawn.
@compute @workgroup_size(1)
fn offsets() {
    let buckets = params.nbx * params.nby;
    var running = 0u;
    for (var b = 0u; b < buckets; b += 1u) {
        let prims = atomicLoad(&counts[CLASS0 + 2u * b]);
        let quads = atomicLoad(&counts[CLASS0 + 2u * b + 1u]);
        let at = b * 8u;
        draw_args[at] = prims;
        draw_args[at + 1u] = 1u;
        draw_args[at + 2u] = running;
        draw_args[at + 3u] = 0u;
        atomicStore(&counts[CLASS0 + 2u * b], running);
        running += prims;
        draw_args[at + 4u] = 6u * quads;
        draw_args[at + 5u] = 1u;
        draw_args[at + 6u] = 6u * running;
        draw_args[at + 7u] = 0u;
        atomicStore(&counts[CLASS0 + 2u * b + 1u], running);
        running += quads;
    }
    atomicStore(&counts[CLASS0 + 2u * buckets], running);
}

@compute @workgroup_size(256)
fn scatter(
    @builtin(local_invocation_index) tid: u32,
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(num_workgroups) num: vec3<u32>,
) {
    let classes = 2u * params.nbx * params.nby + 1u;
    for (var c = tid; c < classes; c += WG) {
        atomicStore(&local_count[c], 0u);
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
            key[k] = class_of(i, where_is(keep[k].position));
            rank[k] = atomicAdd(&local_count[key[k]], 1u);
        }
    }
    workgroupBarrier();
    // One reservation per class this chunk has points in.
    for (var c = tid; c < classes; c += WG) {
        let n = atomicLoad(&local_count[c]);
        if n > 0u {
            local_base[c] = atomicAdd(&counts[CLASS0 + c], n);
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
