# Plan: View-Space Hash Grid Accumulation with Reprojection

## Goal
Replace point rasterization with sparse 3D accumulation for 10-100x point density scaling, Apophysis-style grain aesthetic, and future lighting support. Use reprojection for temporal stability during camera movement.

## Architecture

```
Per Frame (double-buffered):

┌─────────────────────────────────────────────────────────────────────────┐
│ 1. REPROJECT: grid_A (prev view) → grid_B (curr view)                   │
│    • Unproject voxel coords to world via prev_inv_view_proj             │
│    • Reproject to current view via curr_view_proj                       │
│    • Insert into grid_B with slight decay (0.98)                        │
├─────────────────────────────────────────────────────────────────────────┤
│ 2. ACCUMULATE: chaos game iterations → grid_B                           │
│    • New points project and hash-insert into same grid                  │
├─────────────────────────────────────────────────────────────────────────┤
│ 3. COMPACT: extract non-empty cells → render buffer                     │
├─────────────────────────────────────────────────────────────────────────┤
│ 4. RENDER: billboard quads from compacted voxels                        │
├─────────────────────────────────────────────────────────────────────────┤
│ 5. SWAP: grid_A ↔ grid_B, store curr matrices as prev for next frame   │
└─────────────────────────────────────────────────────────────────────────┘
```

**Why reprojection:**
- Voxels represent world-space density, just addressed in screen coords
- Camera movement = different screen address for same world point
- Reprojection transforms old addresses to new addresses
- No ghosting, no invalidation, full temporal stability

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Voxel space | View/clip space (NDC) | Auto-adapts to frustum, no bounds needed |
| Sparse storage | GPU hash table | 4M entries vs 4B theoretical cells |
| Resolution | 10-bit X, 10-bit Y, 12-bit depth | ~1024×1024×4096 effective, stored sparse |
| Depth mapping | Logarithmic | More resolution near camera |
| Color | 8-bit colormap index + weight | Preserves existing color system, 8 bits > 6 required |
| Temporal | 0.98 decay/frame | Smooth 30FPS motion, grain fades organically |
| Depth handling | Keep nearest per cell | Automatic occlusion culling, reduces overdraw |
| Old renderer | Keep as fallback | Toggle for debugging until new system proven |

## Memory Budget (~161MB total)

| Buffer | Size | Notes |
|--------|------|-------|
| Hash grid A | 64 MB | 4M entries × 16 bytes (previous frame) |
| Hash grid B | 64 MB | 4M entries × 16 bytes (current frame) |
| Render voxels | 32 MB | 2M voxels × 16 bytes (compaction output) |
| Misc (counters, params, walkers) | ~1 MB | Unchanged from current |

## Data Structures

```rust
// Hash cell: 16 bytes (padded for alignment)
struct HashCell {
    key: u32,           // encoded (screen_x:10, screen_y:10, depth:12)
    density: u32,       // accumulated hit count (fixed-point u16.8)
    color_weight: u32,  // colormap_idx:8 | reserved:8 | weight:16
    min_depth: u32,     // nearest depth (for occlusion) - atomicMin
}

// Output voxel for rendering: 16 bytes
struct RenderVoxel {
    clip_pos: [f16; 4], // position in clip space + w for depth
    color: [f16; 3],    // RGB from colormap lookup
    density: f16,       // for alpha/size modulation
}
```

## New Shaders

1. **`reproject.wgsl`** - For each voxel in grid_A: unproject → world → reproject → insert into grid_B
2. **`chaos.wgsl` (modified)** - Project points to clip space, quantize, hash-insert into grid_B
3. **`compact.wgsl`** - Scan grid_B, emit non-empty cells to dense render buffer
4. **`voxel_render.wgsl`** - Billboard quads from compacted voxels (adapted from points.wgsl)

**Reprojection shader pseudocode:**
```wgsl
@compute @workgroup_size(256)
fn reproject(@builtin(global_invocation_id) id: vec3<u32>) {
    let cell = grid_a[id.x];
    if cell.key == EMPTY { return; }

    // Decode screen coords from key
    let (sx, sy, depth_slice) = decode_key(cell.key);

    // Unproject to world space using PREVIOUS frame's inverse VP
    let ndc = vec3(sx/res_x * 2 - 1, sy/res_y * 2 - 1, slice_to_ndc_z(depth_slice));
    let depth = slice_to_depth(depth_slice);
    let world_pos = prev_inv_vp * vec4(ndc * depth, depth);

    // Reproject to CURRENT frame's view
    let new_clip = curr_vp * world_pos;
    if new_clip.w <= 0 { return; }  // Behind camera
    let new_ndc = new_clip.xyz / new_clip.w;
    if any(abs(new_ndc.xy) > 1.0) { return; }  // Off screen

    // Hash-insert into grid_B with decayed density
    let new_key = encode_key(new_ndc, new_clip.w);
    hash_insert(grid_b, new_key, cell.density * 0.98, cell.color);
}
```

## Implementation Steps

### Phase 1: Infrastructure
1. Add `HashCell`, `RenderVoxel`, `HashGridParams` to `src/gpu/buffers.rs`
2. Create `src/gpu/hash_grid.rs` with double-buffer allocation and swap logic
3. Add `prev_inv_view_proj` storage for reprojection

### Phase 2: Reprojection
4. Create `shaders/reproject.wgsl` - unproject/reproject pass
5. Create `src/gpu/reproject.rs` pipeline wrapper
6. Test with moving camera - verify voxels track correctly

### Phase 3: Accumulation
7. Modify `shaders/chaos.wgsl` to project points and hash-insert into grid_B
8. Update `src/gpu/compute.rs` to pass view-projection matrix and grid bindings
9. Test with fixed camera - verify density builds up

### Phase 4: Extraction & Rendering
10. Create `shaders/compact.wgsl` - atomic append of non-empty cells
11. Create `shaders/voxel_render.wgsl` - billboard rendering from voxel buffer
12. Create `src/gpu/voxel_renderer.rs` (adapt from render.rs)
13. Implement voxel count readback for draw calls

### Phase 5: Integration
14. Modify `src/app.rs` frame loop: reproject → accumulate → compact → render → swap
15. Wire up scene parameters for decay factor, grid resolution
16. Performance tuning

## Files to Modify/Create

| File | Action |
|------|--------|
| `src/gpu/buffers.rs` | Add HashCell, RenderVoxel, HashGridParams |
| `src/gpu/hash_grid.rs` | **NEW** - double-buffered hash grid, swap logic |
| `src/gpu/reproject.rs` | **NEW** - reprojection pipeline |
| `src/gpu/voxel_renderer.rs` | **NEW** - voxel billboard rendering |
| `src/gpu/compute.rs` | Add hash grid bindings, projection uniforms |
| `src/gpu/mod.rs` | Export new modules |
| `src/app.rs` | New frame loop: reproject → accumulate → compact → render → swap |
| `shaders/chaos.wgsl` | Project + hash-insert instead of ring buffer |
| `shaders/reproject.wgsl` | **NEW** - unproject/reproject with decay |
| `shaders/compact.wgsl` | **NEW** - hash table compaction |
| `shaders/voxel_render.wgsl` | **NEW** - voxel billboard shader |

## Future: Lighting (stretch goal design hooks)

The hash grid structure supports computing normals via density gradients:
```
normal = normalize(density[+x] - density[-x], density[+y] - density[-y], density[+z] - density[-z])
```
This would require a neighbor-lookup pass after compaction, querying adjacent cells in the hash table.

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Hash collision hotspots | Linear probing with 64-probe max; drop samples gracefully |
| Voxel count readback latency | Double-buffer staging; use previous frame's count |
| Large camera jumps cause ghosting | Detect via matrix delta; use aggressive decay (0.5) or clear |
| Memory exceeded | Reduce hash table to 2M entries (24MB) if needed |
