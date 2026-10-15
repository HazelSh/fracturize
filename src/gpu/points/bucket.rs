//! Spatial bucket sort of the point buffer, for volume mode: see
//! `shaders/points/bucket.wgsl` for what it does and why.
//!
//! The sorted copy is a second buffer the size of the ring (16 bytes a point),
//! allocated the first time volume mode draws and kept for the life of the
//! renderer. The three volume passes bind it in place of the ring; nothing else
//! ever reads it.

use wgpu::{BindGroup, BindGroupLayout, Buffer, ComputePipeline, Device};

/// Pixels on a side of one bucket. 128 measured best of 8 to 1280x64: 8x8 and
/// 16x16 are twice as slow (they put one pixel's points in one warp, where the
/// atomics serialise), 64x64 and 128x128 are within noise of each other and of
/// the bands, and the coarser grid is the cheaper sort.
const TILE_PX: f32 = 128.0;
/// The shader's shared arrays hold this many buckets, the last being "off
/// screen". Keep in step with `MAX_BUCKETS` in bucket.wgsl.
const MAX_GRID: u32 = 32;
/// Points a workgroup handles: `CHUNK` in bucket.wgsl.
const CHUNK: u32 = 2048;

/// Compiled once, with the rest of volume mode's pipelines.
#[derive(Clone)]
pub struct BucketPipelines {
    layout: BindGroupLayout,
    histogram: ComputePipeline,
    scan: ComputePipeline,
    scatter: ComputePipeline,
}

impl BucketPipelines {
    pub fn new(device: &Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bucket_sort_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/points/bucket.wgsl").into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        use wgpu::BufferBindingType::{Storage, Uniform};
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bucket_sort_bind_group_layout"),
            entries: &[
                entry(0, Storage { read_only: true }),
                entry(1, Uniform),
                entry(2, Storage { read_only: false }),
                entry(3, Storage { read_only: false }),
                entry(4, Uniform),
                entry(5, Storage { read_only: false }),
                entry(6, Storage { read_only: false }),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bucket_sort_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make = |name: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self { layout, histogram: make("histogram"), scan: make("scan"), scatter: make("scatter") }
    }
}

/// How big volume points are drawn, as the sort decides it (see `scan` in
/// bucket.wgsl). All in accumulation pixels.
#[derive(Clone, Copy, Debug)]
pub struct Footprint {
    /// Points per footprint the coverage rule aims for.
    pub k_cov: f32,
    /// The largest radius it may reach.
    pub max_radius: f32,
    /// A radius to use whatever the density says, for measurements; 0 is the rule.
    pub force_radius: f32,
}

/// The sorted copy of one renderer's ring, and what sorts into it.
pub struct BucketSorter {
    /// The points, bucket by bucket: what the volume passes draw.
    pub sorted: Buffer,
    counts: Buffer,
    /// Per bucket, two indirect draws, the 1px primitives then the quads: see
    /// `scan`.
    ///
    pub draw_args: Buffer,
    /// Word 0: the footprint radius the points are drawn at, in accumulation
    /// pixels. Bound to the vertex stages.
    pub info: Buffer,
    /// Buckets the last frame was sorted into, to draw them.
    buckets: std::cell::Cell<u32>,
    /// The ring and the camera buffer the sort reads, kept to bind each frame.
    inputs: (Buffer, Buffer),
}

impl BucketSorter {
    pub fn new(device: &Device, ring: &Buffer, camera: &Buffer) -> Self {
        let sorted = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_sorted_points"),
            size: ring.size(),
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bucket_counts"),
            size: (MAX_GRID * MAX_GRID + 1) as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_args = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_draw_args"),
            size: (MAX_GRID * MAX_GRID + 1) as u64 * 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let info = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume_info"),
            size: (MAX_GRID * MAX_GRID + 3) as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { sorted, counts, draw_args, info, buckets: std::cell::Cell::new(0), inputs: (ring.clone(), camera.clone()) }
    }

    /// Issue the draws the last sort decided on, one per bucket and kind, with
    /// the pipeline that matches.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, point: &wgpu::RenderPipeline, quad: &wgpu::RenderPipeline) {
        let buckets = self.buckets.get() as u64;
        pass.set_pipeline(point);
        for b in 0..buckets {
            pass.draw_indirect(&self.draw_args, b * 32);
        }
        pass.set_pipeline(quad);
        for b in 0..buckets {
            pass.draw_indirect(&self.draw_args, b * 32 + 16);
        }
    }

    /// What the last frame's sort decided, over the points on screen: the
    /// largest footprint radius (accumulation pixels), and how many points went
    /// as primitives and as quads. Blocks: a measurement.
    pub fn read_footprint(&self, device: &Device, queue: &wgpu::Queue) -> (f32, u32, u32) {
        let buckets = self.buckets.get() as u64;
        let draws = buckets * 32;
        let radii = (buckets + 3) * 4;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("footprint_read"),
            size: draws + radii.next_multiple_of(4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.draw_args, 0, &read, 0, draws);
        encoder.copy_buffer_to_buffer(&self.info, 0, &read, draws, radii);
        queue.submit(std::iter::once(encoder.finish()));
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let data = read.slice(..).get_mapped_range();
        let words: &[u32] = bytemuck::cast_slice(&data);
        let (mut prims, mut quads, mut widest) = (0u32, 0u32, 1.0f32);
        for b in 0..buckets as usize {
            prims += words[b * 8];
            quads += words[b * 8 + 4] / 6;
            if words[b * 8 + 4] > 0 {
                widest = widest.max(f32::from_bits(words[(draws / 4) as usize + 2 + b]));
            }
        }
        (widest, prims, quads)
    }

    /// Sort the first `count` points of the ring into `self.sorted`, for the
    /// camera now in the camera buffer, over a frame `width x height` pixels.
    pub fn encode(
        &self,
        device: &Device,
        pipelines: &BucketPipelines,
        encoder: &mut wgpu::CommandEncoder,
        count: u32,
        width: u32,
        height: u32,
        footprint: Footprint,
        stamp: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if count == 0 {
            return;
        }
        let nb = |px: u32| ((px as f32 / TILE_PX).ceil() as u32).clamp(1, MAX_GRID);
        let (nbx, nby) = (nb(width), nb(height));
        self.buckets.set(nbx * nby);
        // A few words, written at creation: this runs without a queue in hand,
        // and a fresh sixteen bytes a frame is nothing.
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bucket_params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        params.slice(..).get_mapped_range_mut().copy_from_slice(bytemuck::cast_slice(&[
            nbx,
            nby,
            count,
            0u32,
            footprint.k_cov.to_bits(),
            (width as f32 * height as f32).to_bits(),
            footprint.max_radius.to_bits(),
            footprint.force_radius.to_bits(),
        ]));
        params.unmap();
        let bind_group: BindGroup = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bucket_sort_bind_group"),
            layout: &pipelines.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.inputs.0.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.inputs.1.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.sorted.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.counts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: self.draw_args.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: self.info.as_entire_binding() },
            ],
        });
        encoder.clear_buffer(&self.counts, 0, None);
        let groups = count.div_ceil(CHUNK);
        let (gx, gy) = (groups.min(65_535), groups.div_ceil(65_535));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("bucket_sort"), timestamp_writes: stamp });
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_pipeline(&pipelines.histogram);
        pass.dispatch_workgroups(gx, gy, 1);
        pass.set_pipeline(&pipelines.scan);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&pipelines.scatter);
        pass.dispatch_workgroups(gx, gy, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader's constants are the host's, in another language.
    #[test]
    fn the_shader_agrees_about_chunks_and_buckets() {
        let wgsl = include_str!("../../../shaders/points/bucket.wgsl");
        let grab = |key: &str| -> u32 {
            let at = wgsl.find(key).unwrap_or_else(|| panic!("bucket.wgsl must declare {}", key));
            let rest = &wgsl[at + key.len()..];
            rest[..rest.find("u;").unwrap()].trim().parse().unwrap()
        };
        assert_eq!(grab("const CHUNK: u32 ="), CHUNK);
        assert_eq!(grab("const MAX_BUCKETS: u32 ="), MAX_GRID * MAX_GRID + 1);
        // The shared arrays are declared with the same length.
        assert!(wgsl.contains(&format!("array<atomic<u32>, {}>", MAX_GRID * MAX_GRID + 1)));
        assert!(wgsl.contains(&format!("array<u32, {}>", MAX_GRID * MAX_GRID + 1)));
    }
}
