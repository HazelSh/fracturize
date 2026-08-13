// Persistent accumulation histogram: fold a batch of new samples into a
// storage buffer that outlives the point ring.
//
// This is what removes the sample ceiling. The chaos game writes into a
// *circular* point buffer, so the total distinct samples in a finished image
// equals the buffer's capacity and nothing more — `--accumulate` changes which
// samples survive, never how many. A 1.45 s run generates 2.1e9 samples and
// throws away 95% of them. Here the ring becomes a streaming working set: each
// batch of new points is splatted into a transient texture, added into this
// buffer, and then may be overwritten freely, because the buffer already has it.
//
// ## Why fixed point in u32 pairs
//
// Each thread owns exactly one texel, so no atomics are needed however many
// points landed there — which sidesteps every risky alternative at once. fp32
// blending is not guaranteed exposed by wgpu; storage-texture atomics are not
// reliably available; and non-atomic scattered writes from overlapping fragment
// invocations are outright unsound, silently losing samples.
//
// **Not f32**, though f32 would need no carry logic and never overflow. Its
// 24-bit mantissa stops registering an increment once the running sum exceeds
// it by ~1.7e7 — and a long render reaches that. Batches add roughly a constant
// amount to a hot texel, so the sum-to-increment ratio is just the batch count,
// and an overnight run is millions of batches. The failure is not a rounding
// error but a *stall*: hot cores stop growing while everything around them
// keeps growing, so the picture's contrast quietly drifts. Fixed point in 64
// bits has no such point.
//
// **Not 32-bit fixed point** either, which is what a 16-byte texel would allow.
// The two requirements pull apart: representing the faint outer tail of a large
// gaussian splat (~1.6e-4 of a sample) needs a fine scale, and accumulating a
// bright core over an overnight render needs a huge range. 32 bits cannot have
// both — at a scale fine enough for the tails the range tops out around 65k
// samples, and at a range large enough for the cores the tails quantize to zero
// and the halo around every near-field mote disappears. 64 bits has room for
// both with a scale of 1/65536, and the cost is 32 bytes a texel.

struct AccumParams {
    width: u32,
    height: u32,
    /// Fixed-point scale: density is stored as `round(value * scale)`.
    scale: f32,
    _pad: u32,
}

/// One texel is four channels, each a 64-bit fixed-point value stored
/// little-endian as two u32s: `[r_lo, r_hi, g_lo, g_hi, b_lo, b_hi, a_lo, a_hi]`.
const WORDS_PER_TEXEL: u32 = 8u;

/// 2^32, for splitting and rejoining the halves.
const TWO_32: f32 = 4294967296.0;

@group(0) @binding(0) var batch_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> accum: array<u32>;
@group(0) @binding(2) var<uniform> params: AccumParams;

// Dispatched over the texel grid in 2D, not as a flat 1D range. A 4x
// supersampled 1080p accumulation is 33 million texels, and a 1D dispatch of
// those at 64 per group wants 519,000 workgroups against a limit of 65,535.
@compute @workgroup_size(8, 8)
fn accumulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.width || gid.y >= params.height {
        return;
    }
    let i = gid.y * params.width + gid.x;
    let v = textureLoad(batch_tex, vec2<i32>(i32(gid.x), i32(gid.y)), 0);
    let base = i * WORDS_PER_TEXEL;

    for (var c = 0u; c < 4u; c = c + 1u) {
        let x = max(v[c], 0.0);
        // Scaled into fixed point, then split across the word boundary.
        //
        // The split is not optional. `u32(x * scale)` overflows silently once
        // `x` passes 65536 at a scale of 65536, and an fp32 batch texture
        // reaches that easily — a bright core takes tens of thousands of
        // samples in one pass. The earlier version clamped at the fp16 ceiling
        // instead, which was only ever safe because the batch texture *was*
        // fp16 and had already stalled long before.
        //
        // f32 has 24 mantissa bits, so `scaled` loses its low bits once it
        // passes 2^24. That is fine and is not the failure this replaced: the
        // lost bits are a *relative* error on a large per-batch value, whereas
        // the fp16 stall was an absolute ceiling that discarded everything
        // above it.
        let scaled = x * params.scale;
        let hi_add = u32(floor(scaled / TWO_32));
        // Round to nearest, not truncate: truncation is a *biased* error, and
        // a bias repeated over millions of batches is a systematic darkening
        // of exactly the dim regions where each batch's contribution is small.
        let lo_add = u32(scaled - f32(hi_add) * TWO_32 + 0.5);

        let lo_index = base + c * 2u;
        let lo = accum[lo_index];
        let sum = lo + lo_add;
        accum[lo_index] = sum;
        // Unsigned wraparound is defined, so a carry is just "the sum came out
        // smaller than what it started from".
        var carry = hi_add;
        if sum < lo {
            carry = carry + 1u;
        }
        if carry != 0u {
            accum[lo_index + 1u] = accum[lo_index + 1u] + carry;
        }
    }
}

// === Resolve ===
//
// The persistent buffer back out as a float texture, once, at the end of a run
// — so the existing reconstruction filter and tonemap need to know nothing
// about any of the above.
//
// Rgba32Float and not Rgba16Float: fp16 tops out at 65504, and an accumulated
// density is routinely far past that. It is still *linear* density, not a
// picture; the log tonemap has not run.

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
}

@vertex
fn vs_resolve(@builtin(vertex_index) idx: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(idx & 1u) * 4 - 1);
    let y = f32(i32(idx >> 1u) * 4 - 1);
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var<storage, read> accum_ro: array<u32>;
@group(0) @binding(1) var<uniform> resolve_params: AccumParams;

@fragment
fn fs_resolve(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<u32>(floor(in.clip_position.xy));
    let i = p.y * resolve_params.width + p.x;
    let base = i * WORDS_PER_TEXEL;

    var out = vec4<f32>(0.0);
    for (var c = 0u; c < 4u; c = c + 1u) {
        let lo = accum_ro[base + c * 2u];
        let hi = accum_ro[base + c * 2u + 1u];
        // f32 loses the low bits of a large total, and that is fine here in a
        // way it was not in the accumulator: this value is about to have its
        // logarithm taken and be quantized to a pixel, so what matters is
        // relative precision, which f32 keeps. What f32 could not do is *carry
        // the running sum*, where absolute precision decides whether an
        // increment registers at all.
        out[c] = (f32(hi) * TWO_32 + f32(lo)) / resolve_params.scale;
    }
    return out;
}
