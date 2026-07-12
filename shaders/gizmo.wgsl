// Gizmo renderer: unit right-angled tetrahedron per IFS transform
// Renders faces (semi-transparent triangles), edges (screen-space quads), and origin dots (billboards)

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    screen_height: f32,
    point_size: f32,
    aspect_ratio: f32,
    min_point_pixels: f32,
    fog_near: f32,
    fog_far: f32,
    fog_brightness: f32,
    fog_saturation: f32,
    color_contrast: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) edge_other: vec3<f32>,
    @location(3) vertex_type: u32,   // 0=face, 1=edge, 2=dot
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) inst_alpha: f32,
}

@group(0) @binding(0) var<uniform> camera: CameraUniforms;
@group(0) @binding(1) var<storage, read> transforms: array<mat4x4<f32>>;
@group(0) @binding(2) var<storage, read> instance_alpha: array<f32>;
// Hovered gizmo part: x = instance index (0 = none), y = part id
// (0..5 = edge index in build order [ox oy oz xy yz xz], 6 = origin dot)
@group(0) @binding(3) var<uniform> highlight: vec4<u32>;

// 1.0 when this vertex belongs to the hovered part of the hovered instance.
// Edge identity comes from the vertex index: edges+dots live in 42-vertex
// blocks (6 edges x 6 verts, then a 6-vert dot billboard); faces are drawn
// from separate ranges and never highlight (vertex_type guards them).
fn highlight_factor(instance_index: u32, vertex_index: u32, vertex_type: u32) -> f32 {
    if instance_index != highlight.x {
        return 0.0;
    }
    let local = vertex_index % 42u;
    if vertex_type == 2u {
        // origin dot
        if highlight.y == 6u {
            return 1.0;
        }
    } else if vertex_type == 1u {
        if local < 36u && local / 6u == highlight.y {
            return 1.0;
        }
    }
    return 0.0;
}

@vertex
fn vs_main(
    in: VertexInput,
    @builtin(instance_index) instance_index: u32,
    @builtin(vertex_index) vertex_index: u32,
) -> VertexOutput {
    let transform = transforms[instance_index];
    let alpha_mult = instance_alpha[instance_index];
    let hl = highlight_factor(instance_index, vertex_index, in.vertex_type);

    var out: VertexOutput;
    out.color = in.color;
    out.inst_alpha = alpha_mult;

    if in.vertex_type == 0u {
        // === FACE VERTEX: simple transform + project ===
        let world_pos = transform * vec4<f32>(in.position, 1.0);
        out.clip_position = camera.view_proj * world_pos;

    } else if in.vertex_type == 1u {
        // === EDGE VERTEX: screen-space width quad ===
        // Each edge quad vertex stores which endpoint it belongs to in position,
        // and the other endpoint in edge_other. We also need a sign for which
        // side of the edge to offset to. We encode this: vertex_type=1 means
        // this vertex's position is endpoint A. The actual corner is determined
        // by the triangle winding within the 6-vertex quad.
        //
        // For edges, we use a trick: position.x/y/z is one endpoint,
        // edge_other is the other endpoint. We figure out the offset direction
        // from the projected edge direction in screen space.

        let world_a = transform * vec4<f32>(in.position, 1.0);
        let world_b = transform * vec4<f32>(in.edge_other, 1.0);

        let clip_a = camera.view_proj * world_a;
        let clip_b = camera.view_proj * world_b;

        // If behind camera, discard
        if clip_a.w <= 0.0 || clip_b.w <= 0.0 {
            out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
            return out;
        }

        let ndc_a = clip_a.xyz / clip_a.w;
        let ndc_b = clip_b.xyz / clip_b.w;

        // Edge direction in screen space (account for aspect ratio)
        let screen_a = vec2<f32>(ndc_a.x * camera.aspect_ratio, ndc_a.y);
        let screen_b = vec2<f32>(ndc_b.x * camera.aspect_ratio, ndc_b.y);
        let edge_dir = normalize(screen_b - screen_a);
        let perp = vec2<f32>(-edge_dir.y, edge_dir.x);

        // 2px width in NDC; hovered edges grow to 4.6px
        let half_width_ndc_y = (2.0 + 2.6 * hl) / camera.screen_height;
        let half_width_ndc_x = half_width_ndc_y / camera.aspect_ratio;
        let offset = vec2<f32>(perp.x * half_width_ndc_x, perp.y * half_width_ndc_y);

        // The edge_other field's first component sign tells us which side (+1 or -1)
        // Actually, we need to encode side and endpoint in the vertex data.
        // Let's use a different approach: we pack side info into color alpha for edges.
        // No — let's just use the vertex index pattern within the 6-vertex quad.
        //
        // We'll encode the side offset in a simpler way:
        // For edge vertices, color.a encodes the side: > 0.5 means +side, <= 0.5 means -side
        // And we need to know which endpoint this vertex is at.
        // We'll use vertex_type: still 1 for all edge vertices.
        // We encode: position = this endpoint, edge_other = other endpoint.
        // The sign is encoded as: if edge_other.x/y/z has a special marker...
        //
        // Simplest approach: encode side in the w component... but we only have vec3.
        // Let's use the color alpha channel to encode side direction:
        //   alpha > 0 means side = +1, alpha < 0 means side = -1
        //   (We'll fix the actual alpha in fragment shader)
        // Actually color is vec4 and alpha will be used for rendering.
        //
        // Better: we'll just add a side_sign to the vertex data encoded in a float.
        // But we want to keep the vertex format simple.
        //
        // Simplest working approach: encode which of the 6 quad corners this is
        // using the magnitude of edge_other minus position. If they're the same,
        // we know it's the same endpoint... No.
        //
        // Let me just use a clean approach:
        // The 6 vertices per edge come as pairs:
        //   v0: pos=A, other=B, color.a encodes +side  (actual alpha stored pre-multiplied)
        //   v1: pos=B, other=A, color.a encodes +side
        //   v2: pos=A, other=B, color.a encodes -side
        //   (triangle strip style as 2 triangles)
        //
        // I'll encode side as: if color.a >= 0 -> +1, if color.a < 0 -> -1
        // Then in fragment shader, use abs(color.a) as actual alpha.

        let side = sign(in.color.a);  // +1.0 or -1.0
        let ndc = ndc_a; // position is always "this" endpoint
        let depth = ndc_a.z;

        out.clip_position = vec4<f32>(
            ndc.x + offset.x * side,
            ndc.y + offset.y * side,
            depth,
            1.0
        );
        // Hovered edges glow toward white at full opacity
        let edge_rgb = mix(in.color.rgb, vec3<f32>(1.0), 0.55 * hl);
        out.color = vec4<f32>(edge_rgb, max(abs(in.color.a), hl));

    } else {
        // === DOT VERTEX: billboard ===
        let world_pos = transform * vec4<f32>(in.position, 1.0);
        let clip = camera.view_proj * world_pos;

        if clip.w <= 0.0 {
            out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
            return out;
        }

        let ndc = clip.xyz / clip.w;

        // 6px dot; grows to 10px on hover
        let half_size_ndc_y = (6.0 + 4.0 * hl) / camera.screen_height;
        let half_size_ndc_x = half_size_ndc_y / camera.aspect_ratio;

        // Billboard offset encoded in edge_other.xy (-1..1 range)
        let corner = in.edge_other.xy;

        out.clip_position = vec4<f32>(
            ndc.x + corner.x * half_size_ndc_x,
            ndc.y + corner.y * half_size_ndc_y,
            ndc.z,
            1.0
        );
    }

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var c = in.color;
    if in.inst_alpha < 1.0 {
        // Desaturate towards grey for disabled gizmos
        let lum = dot(c.rgb, vec3<f32>(0.3, 0.6, 0.1));
        c = vec4<f32>(vec3<f32>(lum), c.a * in.inst_alpha);
    }
    return c;
}
