# Fracturize - AI Agent Guidelines

A 3D fractal flame renderer inspired by Apophysis, built with Rust and OpenGL (via miniquad).

## Project Overview

Fracturize renders IFS (Iterated Function System) fractals in 3D using the chaos game algorithm. Points are iterated through weighted random transform selection, accumulating color as they go, then rendered as a point cloud.

## Architecture

```
src/
  main.rs       # Currently monolithic - contains all code
```

Key structures:
- `Transform`: Affine transform with matrix, color, and weight
- `IfsSystem`: Collection of transforms with weighted random selection
- `Stage`: Main app state, owns rendering context and iteration state

## Tech Stack

- **Rust 2024 edition** - Note: `gen` is a reserved keyword, use `r#gen()` for rand
- **miniquad 0.4** - OpenGL abstraction, stores context in Stage as `Box<dyn RenderingBackend>`
- **glam** - Math (Vec3, Vec4, Mat4, Quat)
- **rand + rand_xoshiro** - Fast RNG with Xoshiro256++
- **bytemuck** - Safe transmutes for GPU buffers
- **quick-xml + serde** - For .flame file parsing (not yet implemented)

## Rendering Approach

Current: CPU iteration → GPU billboard rendering
- Chaos game runs on CPU, collecting points with positions and colors
- Each point rendered as a camera-facing quad (2 triangles, 6 vertices)
- Circle shader discards pixels outside radius for soft circular dots
- Points uploaded each frame to streaming vertex buffer
- Camera orbits the origin

Billboard setup:
- Compute right/up vectors from camera view direction
- Offset quad corners: center ± right ± up
- UV coords from -1 to 1, used for circle distance calculation

Future considerations:
- Histogram accumulation buffer for proper density rendering
- Log-density tone mapping
- Compute shader iteration (optional GPU acceleration)

## Planned Features

1. **Nonlinear transforms** - sinusoidal, spherical, swirl, horseshoe, etc.
2. **Transform blending** - interpolate between transform types
3. **Color palette system** - gradient-based coloring instead of per-transform colors
4. **.flame file support** - XML format from Apophysis/Flam3
5. **GUI** - Transform editing, parameter tweaking
6. **Proper accumulation** - Histogram buffer with tone mapping
7. **Render to image** - High-quality offline rendering

## Coding Conventions

- Keep it simple - avoid over-abstraction
- Prefer flat iteration over recursion for the chaos game
- Use glam for all math, not manual array ops
- Shader code: GLSL ES 100 (miniquad default)

## Known Quirks

- **miniquad 0.4 API**: Context is `Box<dyn RenderingBackend>`, use `window::` module for screen_size, request_quit, etc.
- **miniquad index buffer**: CRITICAL - miniquad requires valid index buffer even for non-indexed triangle rendering. Create sequential indices `[0, 1, 2, 3, 4, 5, ...]` matching vertex count.
- **Rust 2024**: raw identifier `r#gen()` needed for rand's gen method (gen is reserved keyword)
- **GLSL ES 100**: Use `attribute`/`varying` not `in`/`out`, need `precision mediump float` in fragment shader
