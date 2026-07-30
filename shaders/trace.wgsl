// In-world line renderer: chaos traces, camera paths, selection indicators.
//
// Anti-aliased screen-space quads, not native `LineList` primitives. wgpu can
// only rasterize 1px hardware lines, with no width and no coverage — which on
// a fractal that is itself made of single pixels reads as *more dust*, exactly
// the thing these lines are supposed to be legible against. Each segment is
// expanded (CPU side, in `LineRenderer::set_lines`) into a camera-facing
// ribbon, given a fixed pixel width here, and given a smooth coverage falloff
// across it — so the UI drawn into the scene looks like UI, and the artwork
// keeps its own aliasing, which is the point of it.

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    haze_near: f32,
    haze_far: f32,
    haze_transmittance: f32,
    haze_saturation: f32,
    color_contrast: f32,
    // Linear RGB, in what used to be tail padding — haze fades toward it.
    bg_r: f32,
    bg_g: f32,
    bg_b: f32,
}

@group(0) @binding(0) var<uniform> camera: CameraUniforms;

// Half-width of a line's solid core, in pixels. Thin enough not to fatten the
// gizmo indicators, wide enough to have something to anti-alias.
const HALF_WIDTH_PX: f32 = 1.1;
// Pixels across which coverage falls from 1 to 0 either side of the core.
const FEATHER_PX: f32 = 1.0;

struct VertexInput {
    // This vertex's own end of the segment
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    // The segment's other end, for the screen-space direction
    @location(2) other: vec3<f32>,
    // Which side of the line this corner sits on: -1 or +1
    @location(3) side: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Signed position across the ribbon, -1..1 at its edges
    @location(1) across: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.color = in.color;
    out.across = in.side;

    let clip_a = camera.view_proj * vec4<f32>(in.position, 1.0);
    let clip_b = camera.view_proj * vec4<f32>(in.other, 1.0);

    // Either end behind the eye: drop the whole quad rather than let it wrap
    // around through the singularity into a stripe across the view.
    if clip_a.w <= 0.0 || clip_b.w <= 0.0 {
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        return out;
    }

    let ndc_a = clip_a.xyz / clip_a.w;
    let ndc_b = clip_b.xyz / clip_b.w;

    // Direction in a square screen space, so the perpendicular really is
    // perpendicular on screen and the width doesn't stretch with aspect.
    let screen_a = vec2<f32>(ndc_a.x * camera.aspect_ratio, ndc_a.y);
    let screen_b = vec2<f32>(ndc_b.x * camera.aspect_ratio, ndc_b.y);
    let delta = screen_b - screen_a;
    let len = length(delta);
    // A zero-length segment has no direction to be perpendicular to; any
    // offset will do, since the quad is degenerate either way.
    var perp = vec2<f32>(0.0, 1.0);
    if len > 1e-9 {
        let dir = delta / len;
        perp = vec2<f32>(-dir.y, dir.x);
    }

    // Widened by the feather, so the falloff has room outside the solid core.
    let half_ndc_y = (HALF_WIDTH_PX + FEATHER_PX) / camera.screen_height;
    let offset = vec2<f32>(
        perp.x * half_ndc_y / camera.aspect_ratio,
        perp.y * half_ndc_y,
    );

    out.clip_position = vec4<f32>(
        ndc_a.x + offset.x * in.side,
        ndc_a.y + offset.y * in.side,
        ndc_a.z,
        1.0,
    );
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // `across` spans the widened ribbon, so the solid core ends at
    // HALF_WIDTH / (HALF_WIDTH + FEATHER) and coverage reaches zero at 1.
    let core = HALF_WIDTH_PX / (HALF_WIDTH_PX + FEATHER_PX);
    let coverage = 1.0 - smoothstep(core, 1.0, abs(in.across));
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
