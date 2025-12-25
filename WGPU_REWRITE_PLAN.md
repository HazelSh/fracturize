# Fracturize: wgpu Rewrite with GPU Compute

## Goal
Rewrite the miniquad-based 3D IFS fractal renderer to wgpu with GPU compute shaders for the chaos game iteration.

## Requirements
- GPU compute shader for chaos game (embarrassingly parallel)
- sRGB output with screen blend mode (linear blending, gamma-correct)
- Preserve current sharp/sparkly visual aesthetic
- Keep: TOML scene format, CLI, screenshot capture, keyboard controls
- Point size stays 1px for now (zoom behavior fix deferred)

---

## New File Structure

```
src/
  main.rs           # CLI, winit event loop
  app.rs            # Application state, orchestrates compute + render
  gpu/
    mod.rs
    context.rs      # wgpu device, queue, surface
    compute.rs      # Compute pipeline for chaos game
    render.rs       # Render pipeline for points
    buffers.rs      # Buffer management
  scene.rs          # TOML parsing (unchanged)
  camera.rs         # Camera math (extracted)
shaders/
  chaos.wgsl        # Compute shader
  points.wgsl       # Vertex + fragment shader
```

---

## GPU Buffer Layout

### Point Buffer (32 bytes per point)
```wgsl
struct Point {
    pos: vec3<f32>,    // 12 bytes
    _pad0: f32,        // 4 bytes (alignment)
    color: vec4<f32>,  // 16 bytes
}
```

### Transform Buffer (96 bytes per transform)
```wgsl
struct Transform {
    matrix: mat4x4<f32>,     // 64 bytes
    color: vec4<f32>,        // 16 bytes
    weight: f32,             // 4 bytes
    cumulative_weight: f32,  // 4 bytes
    _pad: vec2<f32>,         // 8 bytes
}
```

### Iteration State (persistent across frames)
```wgsl
struct IterationState {
    current_pos: vec3<f32>,
    _pad0: f32,
    current_color: vec4<f32>,
    point_write_idx: u32,
    total_iterations: u32,
    rng_state: vec4<u32>,  // xorshift128
}
```

---

## Key Rendering Parameters to Preserve

| Parameter | Value |
|-----------|-------|
| Window | 1280x720, high DPI |
| Clear color | `[0.02, 0.02, 0.05, 1.0]` |
| Blend | Screen: `src + dst * (1 - src_color)` |
| Fragment output | `color.rgb * mask * 0.5` |
| Circle mask | `smoothstep(0.8, 1.0, dist)` |
| Color accumulation | `old * 0.8 + new * 0.2` |
| Camera | 45deg FOV, orbit rotation (+0.005 rad/frame) |
| RNG | xorshift128 (GPU-friendly) |

---

## Implementation Phases

### Phase 1: Foundation
1. Update `Cargo.toml` - replace miniquad with wgpu/winit/pollster
2. Create `src/gpu/context.rs` - wgpu setup, sRGB surface
3. Create `src/app.rs` - winit event loop, window creation
4. Test: black window with correct clear color

### Phase 2: Render Pipeline
5. Create `shaders/points.wgsl` - vertex pulling + circular mask
6. Create `src/gpu/buffers.rs` - GpuPoint struct, buffer helpers
7. Create `src/gpu/render.rs` - render pipeline with screen blend
8. Test: CPU-generated points render correctly (port CPU iteration temporarily)

### Phase 3: Compute Pipeline
9. Create `shaders/chaos.wgsl` - xorshift RNG, chaos game loop
10. Create `src/gpu/compute.rs` - compute pipeline, state management
11. Integrate: compute pass before render pass
12. Test: visual output matches original

### Phase 4: Polish
13. Port camera to `src/camera.rs` (orbit, zoom controls)
14. Update `src/main.rs` - keyboard handling, CLI
15. Add screenshot support (render to texture, copy to CPU)
16. Final visual verification against original

---

## Cargo.toml Changes

```toml
# Remove
miniquad = "0.4"

# Add
wgpu = "24.0"
winit = "0.30"
pollster = "0.4"
env_logger = "0.11"
log = "0.4"

# Keep unchanged
glam, rand, rand_xoshiro, serde, toml, clap, bytemuck, image
```

---

## Critical Files

| File | Action |
|------|--------|
| `Cargo.toml` | Update dependencies |
| `src/main.rs` | Rewrite for winit |
| `src/scene.rs` | Keep unchanged |
| `shaders/chaos.wgsl` | New - compute shader |
| `shaders/points.wgsl` | New - vertex/fragment |
| `src/gpu/*.rs` | New - wgpu abstraction |
| `src/app.rs` | New - app state |
| `src/camera.rs` | New - camera math |

---

## Notes

- **Compute parallelism**: Start single-threaded (matches CPU behavior). Future: multiple independent random walks for GPU efficiency.
- **sRGB**: Use `Bgra8UnormSrgb` surface format - wgpu handles linear-to-sRGB conversion automatically.
- **Vertex pulling**: Read point data from storage buffer in vertex shader (no separate vertex buffer needed).
- **Screen blend**: `wgpu::BlendFactor::One` + `wgpu::BlendFactor::OneMinusSrc` - prevents white saturation.
