// The in-world UI's multisampled overlay: a depth blit in, a composite out.
//
// See `src/gpu/overlay.rs` for why the UI gets its own pass. In short: the
// point cloud must not be multisampled (its aliasing is the artwork), and the
// gizmos and camera path must be, so they can't share a target.

// A fullscreen triangle, shared by both passes. Vertices 0,1,2 map to
// (-1,-1), (3,-1), (-1,3), which covers the viewport with one primitive.
@vertex
fn vs_fullscreen(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(idx & 1u) * 4 - 1);
    let y = f32(i32(idx >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

// === Depth blit ===
//
// Copies the main pass's single-sample depth into the overlay's multisampled
// depth buffer, so the fractal still occludes the gizmos exactly as it did
// when they shared one buffer. Every sample of a pixel gets that pixel's
// depth: the occluder is the point cloud, which has no sub-pixel coverage to
// preserve, and the anti-aliasing we're here for is on the overlay's own
// edges, not on the silhouette it's hidden behind.

@group(0) @binding(0) var scene_depth: texture_depth_2d;

@fragment
fn fs_depth_blit(@builtin(position) pos: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(scene_depth, vec2<i32>(pos.xy), 0);
}

// === Composite ===
//
// Resolves the overlay by averaging its samples and blends the result over the
// swapchain. Done in the shader rather than with a hardware `resolve_target`
// because that would need a third full-size texture to resolve *into*, and the
// average of N loads is the same arithmetic.
//
// The overlay's contents are premultiplied: its target is cleared to
// (0,0,0,0), and the gizmo and line pipelines' `SrcAlpha / OneMinusSrcAlpha`
// blending against zero leaves `rgb * a` in the buffer. Averaging premultiplied
// samples is a linear operation and stays premultiplied, so the pipeline that
// draws this composites with `One / OneMinusSrcAlpha`.

@group(0) @binding(0) var overlay: texture_multisampled_2d<f32>;

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(pos.xy);
    let samples = i32(textureNumSamples(overlay));
    var sum = vec4<f32>(0.0);
    for (var s = 0; s < samples; s = s + 1) {
        sum = sum + textureLoad(overlay, coord, s);
    }
    return sum / f32(samples);
}
