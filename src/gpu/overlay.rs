//! A multisampled target for the in-world UI, and the two passes that get it
//! on and off screen.
//!
//! The gizmos, the camera path, the selection indicators and the chaos traces
//! are *interface* drawn into the scene, and interface should not be jagged.
//! The point cloud is the opposite case: its aliasing is the artwork, and
//! multisampling 1px point primitives would soften exactly the grit this
//! renderer exists to make. Those two requirements can't share a render
//! target, so they don't — the overlays render here, at `samples` per pixel,
//! and are composited over the finished frame.
//!
//! An earlier attempt anti-aliased the lines analytically instead (screen-space
//! ribbons with a coverage feather) and was reverted: without mitre joins the
//! ribbon wobbles along a polyline, and a feathered band that writes depth
//! punches holes through whatever is drawn behind it. The rasterizer knows how
//! to do this and doesn't need help.
//!
//! Three things happen per frame:
//!
//! 1. **Depth blit.** The overlays depth-test against the point cloud, so the
//!    main pass's single-sample depth is copied into the multisampled depth
//!    buffer by a fullscreen triangle writing `frag_depth`. Same pass as (2),
//!    so it costs a draw call rather than a pass.
//! 2. **Overlays**, into a colour target cleared to transparent black.
//! 3. **Composite**, averaging the samples and blending over the swapchain.

use crate::gpu::points::DEPTH_FORMAT;

/// Samples per pixel, in order of preference. Two is the cheap one and the one
/// asked for: it halves a hard edge's step into two, which is most of the
/// improvement, at half the memory of 4x. Only 1 and 4 are guaranteed by the
/// spec, so the adapter's list decides (see `GpuContext::surface_sample_counts`).
const PREFERRED_SAMPLES: [u32; 3] = [2, 4, 1];

pub struct OverlayTargets {
    /// Samples per pixel. 1 means the adapter offered nothing better, in which
    /// case this whole thing is a pass-through and costs one composite.
    samples: u32,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,

    depth_blit_layout: wgpu::BindGroupLayout,
    depth_blit_pipeline: wgpu::RenderPipeline,
    depth_blit_bind: wgpu::BindGroup,

    composite_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    composite_bind: wgpu::BindGroup,
}

impl OverlayTargets {
    /// Pick a sample count the adapter actually supports for this format.
    pub fn choose_samples(supported: &[u32]) -> u32 {
        PREFERRED_SAMPLES
            .into_iter()
            .find(|n| supported.contains(n))
            .unwrap_or(1)
    }

    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
        width: u32,
        height: u32,
        scene_depth: &wgpu::TextureView,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/overlay.wgsl").into()),
        });

        let depth_blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_depth_blit_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_composite_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
                count: None,
            }],
        });

        let depth_blit_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("overlay_depth_blit_pipeline_layout"),
                bind_group_layouts: &[Some(&depth_blit_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay_depth_blit"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_depth_blit"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        // Depth only: the colour attachment is along for the
                        // ride because a pass needs consistent attachments,
                        // and its clear is what makes the overlay transparent.
                        write_mask: wgpu::ColorWrites::empty(),
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };

        let composite_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("overlay_composite_pipeline_layout"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay_composite"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_composite"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        // Premultiplied "over": the overlay was built by
                        // blending straight-alpha shaders against a zero
                        // clear, which leaves `rgb * a` behind (see
                        // shaders/overlay.wgsl).
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let (color, depth) = Self::make_targets(device, format, samples, width, height);
        let depth_blit_bind = Self::make_depth_bind(device, &depth_blit_layout, scene_depth);
        let composite_bind = Self::make_composite_bind(device, &composite_layout, &color);

        Self {
            samples,
            format,
            size: (width, height),
            color,
            depth,
            depth_blit_layout,
            depth_blit_pipeline,
            depth_blit_bind,
            composite_layout,
            composite_pipeline,
            composite_bind,
        }
    }

    fn make_targets(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
        width: u32,
        height: u32,
    ) -> (wgpu::TextureView, wgpu::TextureView) {
        let size = wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        };
        let color = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("overlay_color"),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("overlay_depth"),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        (color, depth)
    }

    fn make_depth_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        scene_depth: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_depth_blit_bind"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_depth),
            }],
        })
    }

    fn make_composite_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        color: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_composite_bind"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(color),
            }],
        })
    }

    /// Rebuild the targets for a new surface size. `scene_depth` is the main
    /// pass's depth view, which the caller has just recreated too.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        scene_depth: &wgpu::TextureView,
    ) {
        let (color, depth) = Self::make_targets(device, self.format, self.samples, width, height);
        self.composite_bind = Self::make_composite_bind(device, &self.composite_layout, &color);
        self.depth_blit_bind = Self::make_depth_bind(device, &self.depth_blit_layout, scene_depth);
        self.color = color;
        self.depth = depth;
        self.size = (width, height);
    }

    /// Begin the overlay pass: clears to transparent, then blits the scene's
    /// depth in so the overlays are occluded by the point cloud. The caller
    /// draws into the returned pass and drops it.
    pub fn begin<'a>(&'a self, encoder: &'a mut wgpu::CommandEncoder) -> wgpu::RenderPass<'a> {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.depth_blit_pipeline);
        pass.set_bind_group(0, &self.depth_blit_bind, &[]);
        pass.draw(0..3, 0..1);
        pass
    }

    /// Average the samples and blend the overlay over `target`.
    pub fn composite(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay_composite_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &self.composite_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_two_samples_then_four_then_none() {
        // The cheap one when it's there, the guaranteed one when it isn't, and
        // a pass-through rather than a panic when neither is.
        assert_eq!(OverlayTargets::choose_samples(&[1, 2, 4]), 2);
        assert_eq!(OverlayTargets::choose_samples(&[1, 4]), 4);
        assert_eq!(OverlayTargets::choose_samples(&[1]), 1);
        assert_eq!(OverlayTargets::choose_samples(&[]), 1);
    }
}
