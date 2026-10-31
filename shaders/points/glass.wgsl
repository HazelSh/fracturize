// Glass, the two passes over the points (compiled after glass_common.wgsl).
//
// Every point is a sample of an absorbing, tinted fog, drawn as a flat disc
// of its own colour. Per pixel we keep the total absorbance of everything,
// exactly, and the K nearest fragments sorted, then share the opacity out
// among colours by how much lies in front of each fragment. This is a
// transcription of `src/glass_proto.rs` (`rasterize` and `front_k`), which is
// the reference: if the two disagree, the prototype is right.

struct Point {
    position: vec3<f32>,
    color_idx: u32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<uniform> glass: Glass;
@group(0) @binding(2) var<storage, read> colormap: array<vec4<f32>, 256>;
@group(0) @binding(3) var<storage, read> point_maps: array<u32>;
@group(0) @binding(4) var<storage, read> map_mask: array<u32>;
@group(0) @binding(5) var<storage, read_write> pixels: array<atomic<u32>>;
// Each pixel's K nearest fragments, one word each, in no order: the depth's
// f32 bits with the low byte replaced by a log-quantised tau, so the pair
// compares by depth and moves as one. Stored *inverted*, so a cleared buffer
// (zeros) is empty and an empty slot is "farther" than any fragment: in list
// words, larger is nearer.
@group(0) @binding(6) var<storage, read_write> list: array<atomic<u32>>;

// --- Colour: the same lookup as the other renderers -----------------------

fn unpack_rgb(packed: u32) -> vec3<f32> {
    let v = vec3<f32>(
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu),
    ) / 255.0;
    return v * v;
}

fn lookup_color(color_idx: u32) -> vec3<f32> {
    if glass.color_rgb_mode != 0u {
        return unpack_rgb(color_idx);
    }
    let f = (f32(color_idx & 0xFFu) + 0.5) / 256.0;
    let stretched = fract(0.5 + (f - 0.5) * glass.color_contrast);
    return colormap[u32(stretched * 256.0) & 0xFFu].rgb;
}

// --- Which points are drawn ------------------------------------------------

// `--isolate` and mute/solo, as in render.wgsl: a hidden point adds nothing.
fn hidden(i: u32, pos: vec3<f32>) -> bool {
    if abs(pos.x) > 1.0e36 {
        return true;
    }
    if map_mask[0] == 0u {
        return false;
    }
    let m = point_maps[i];
    if m == 0u {
        return true;
    }
    let s = m - 1u;
    return ((map_mask[1u + s / 32u] >> (s % 32u)) & 1u) != 0u;
}

// The infinite-zoom edge guard (see render.wgsl's guard_weight): the share of
// a point that survives at the zoom band's rim. Here it scales absorbance.
fn guard_weight(pos: vec3<f32>) -> f32 {
    if glass.guard_inv_ln_width == 0.0 {
        return 1.0;
    }
    let r = length(pos - glass.guard.xyz);
    let t = (log(max(r, 1e-20)) - glass.guard.w) * glass.guard_inv_ln_width;
    return 1.0 - smoothstep(0.0, 1.0, t);
}

// --- The K-list word -------------------------------------------------------

const DEPTH_MASK: u32 = 0xFFFFFF00u;

// tau to 8 bits, eight steps an octave from 2^-24 to 2^8; 0 means none.
fn tau_code(tau: f32) -> u32 {
    if tau <= 0.0 {
        return 0u;
    }
    return u32(clamp(round((log2(tau) + 24.0) * 8.0), 1.0, 255.0));
}

fn tau_value(code: u32) -> f32 {
    if code == 0u {
        return 0.0;
    }
    return exp2(f32(code) / 8.0 - 24.0);
}

// --- Rasterising a dot -----------------------------------------------------

struct Dot {
    ok: bool,
    cx: f32,
    cy: f32,
    z: f32,
    tau: f32,
    r_px: f32,
    color: vec3<f32>,
}

fn point_index(gid: vec3<u32>, groups: vec3<u32>) -> u32 {
    return gid.y * groups.x * 256u + gid.x;
}

fn make_dot(i: u32) -> Dot {
    var d: Dot;
    d.ok = false;
    if i >= glass.point_count {
        return d;
    }
    let p = points[i];
    if hidden(i, p.position) {
        return d;
    }
    let clip = glass.view_proj * vec4<f32>(p.position, 1.0);
    // A perspective projection's w is the view depth.
    let z = clip.w;
    if z <= glass.near {
        return d;
    }
    let ndc = clip.xy / z;
    d.cx = glass.rect.x + (ndc.x * 0.5 + 0.5) * glass.rect.z;
    d.cy = glass.rect.y + (0.5 - ndc.y * 0.5) * glass.rect.w;
    // Pixels per world unit at this depth, and the pixel's world area.
    let ppw = glass.ppw_unit / z;
    let pix_area = 1.0 / (ppw * ppw);
    d.r_px = glass.radius_world * ppw;
    let disc_area = 3.14159265 * glass.radius_world * glass.radius_world;
    // The one rule: a dot's absorbance spread over the larger of its own disc
    // and the pixel it lands in.
    d.tau = glass.tau_point * guard_weight(p.position) / max(disc_area, pix_area);
    if d.tau <= 0.0 {
        return d;
    }
    d.z = z;
    d.color = lookup_color(p.color_idx);
    d.ok = true;
    return d;
}

fn in_pane(ix: i32, iy: i32) -> bool {
    let x0 = i32(glass.rect.x);
    let y0 = i32(glass.rect.y);
    return ix >= x0 && iy >= y0 && ix < x0 + i32(glass.rect.z) && iy < y0 + i32(glass.rect.w);
}

// --- Pass 1: count everything, keep the K nearest ---------------------------

// A list word's tau, as the fixed point the LISTED sum keeps.
fn listed_fixed(word: u32) -> u32 {
    if word == 0u {
        return 0u;
    }
    return u32(tau_value((~word) & 0xFFu) * TAU_SCALE + 0.5);
}

fn front(pix: u32, z: f32, tau: f32) {
    let base = pix * STRIDE;
    // The total: everything counts, nearest or not.
    if atomicLoad(&pixels[base + TAU]) < u32(TAU_SATURATED * TAU_SCALE) {
        atomicAdd(&pixels[base + TAU], u32(min(tau, 64.0) * TAU_SCALE + 0.5));
    }
    let zb = bitcast<u32>(z);
    if zb > atomicLoad(&pixels[base + ZMAX]) {
        atomicMax(&pixels[base + ZMAX], zb);
    }

    let k = glass.k;
    let lb = pix * k;
    let entry = ~((zb & DEPTH_MASK) | tau_code(tau));
    // Most fragments in a busy pixel are behind its K-th: one read, and out.
    if entry <= atomicLoad(&list[lb + k - 1u]) {
        return;
    }
    // Bubble in: each slot keeps the nearer of what it held and what we
    // carry, and we carry the other one on, so the list stays sorted under
    // concurrency. Slots nearer than us are passed with a plain read: they
    // only ever get nearer, so they stay ahead of us.
    var v = entry;
    for (var i = 0u; i < k; i++) {
        if v <= atomicLoad(&list[lb + i]) {
            continue;
        }
        let old = atomicMax(&list[lb + i], v);
        v = min(old, v);
        if v == 0u {
            break;
        }
    }
    // What fell off the end left the list, and we joined it: keep the list's
    // own tau sum (a wrapping subtraction is exact).
    atomicAdd(&pixels[base + LISTED], listed_fixed(entry) - listed_fixed(v));
}

// --- Pass 2: each fragment's share of the colour ----------------------------

fn weigh(pix: u32, z: f32, tau: f32, color: vec3<f32>) {
    let base = pix * STRIDE;
    let fkey = bitcast<u32>(z) & DEPTH_MASK;
    let lb = pix * glass.k;
    let kth = atomicLoad(&list[lb + glass.k - 1u]);
    var tau_front = 0.0;
    if kth != 0u && fkey > ((~kth) & DEPTH_MASK) {
        // Behind the K nearest, which in a busy pixel is nearly everyone:
        // the list's tau, plus the leftover taken as spread evenly in depth
        // between the K-th and the farthest. Four words of this pixel's row.
        let tau_total = f32(atomicLoad(&pixels[base + TAU])) / TAU_SCALE;
        let z_max = bitcast<f32>(atomicLoad(&pixels[base + ZMAX]));
        let listed = f32(atomicLoad(&pixels[base + LISTED])) / TAU_SCALE;
        let z_k = bitcast<f32>((~kth) & DEPTH_MASK);
        let span = max(z_max - z_k, 1e-9);
        let rest = max(tau_total - listed, 0.0);
        tau_front = listed + rest * clamp((z - z_k) / span, 0.0, 1.0);
    } else {
        // Among the K nearest: exactly what is nearer. Sorted nearest first,
        // so stop at the first empty slot or the first entry not in front.
        for (var i = 0u; i < glass.k; i++) {
            let s = atomicLoad(&list[lb + i]);
            let e = ~s;
            if s == 0u || (e & DEPTH_MASK) >= fkey {
                break;
            }
            tau_front += tau_value(e & 0xFFu);
        }
    }
    if tau_front > TAU_HIDDEN {
        return;
    }
    let w = exp(-tau_front) * (1.0 - exp(-tau));
    if w * W_SCALE < 0.5 {
        return;
    }
    let c = color * w;
    atomicAdd(&pixels[base + SUM_R], u32(c.r * W_SCALE + 0.5));
    atomicAdd(&pixels[base + SUM_R + 1u], u32(c.g * W_SCALE + 0.5));
    atomicAdd(&pixels[base + SUM_R + 2u], u32(c.b * W_SCALE + 0.5));
    atomicAdd(&pixels[base + SUM_R + 3u], u32(w * W_SCALE + 0.5));
}

// --- Visiting a dot's pixels -------------------------------------------------

// Sub-pixel dots go into the one pixel they land in, their absorbance already
// shrunk by area; bigger ones are discs with a one-pixel linear feather, the
// coverage multiplying tau.
fn visit(d: Dot, pass_weigh: bool) {
    if d.r_px < 0.5 {
        let ix = i32(floor(d.cx));
        let iy = i32(floor(d.cy));
        if !in_pane(ix, iy) {
            return;
        }
        let pix = u32(iy) * glass.fb_width + u32(ix);
        if pass_weigh {
            weigh(pix, d.z, d.tau, d.color);
        } else {
            front(pix, d.z, d.tau);
        }
        return;
    }
    let r = min(d.r_px, glass.max_radius_px);
    let x0 = max(i32(floor(d.cx - r - 1.0)), i32(glass.rect.x));
    let x1 = min(i32(ceil(d.cx + r + 1.0)), i32(glass.rect.x + glass.rect.z) - 1);
    let y0 = max(i32(floor(d.cy - r - 1.0)), i32(glass.rect.y));
    let y1 = min(i32(ceil(d.cy + r + 1.0)), i32(glass.rect.y + glass.rect.w) - 1);
    for (var iy = y0; iy <= y1; iy++) {
        for (var ix = x0; ix <= x1; ix++) {
            let dx = f32(ix) + 0.5 - d.cx;
            let dy = f32(iy) + 0.5 - d.cy;
            let cover = clamp(r + 0.5 - sqrt(dx * dx + dy * dy), 0.0, 1.0);
            if cover > 0.0 {
                let pix = u32(iy) * glass.fb_width + u32(ix);
                if pass_weigh {
                    weigh(pix, d.z, d.tau * cover, d.color);
                } else {
                    front(pix, d.z, d.tau * cover);
                }
            }
        }
    }
}

@compute @workgroup_size(256)
fn cs_front(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let d = make_dot(point_index(gid, groups));
    if d.ok {
        visit(d, false);
    }
}

@compute @workgroup_size(256)
fn cs_weigh(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let d = make_dot(point_index(gid, groups));
    if d.ok {
        visit(d, true);
    }
}
