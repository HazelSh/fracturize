use std::sync::Arc;
use wgpu::{Device, Features, Instance, Queue, Surface, SurfaceConfiguration, TextureFormat};
use winit::window::Window;

/// Holds wgpu device, queue, and surface state
pub struct GpuContext {
    pub device: Device,
    pub queue: Queue,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    pub format: TextureFormat,
    /// Whether the GPU supports f16 shaders (needed for density renderer)
    pub _has_f16: bool,
    /// Sample counts this adapter can render the surface format at. Only 1 and
    /// 4 are guaranteed; 2 is common but optional, and the overlay pass wants
    /// the cheapest one that actually anti-aliases (see `src/gpu/overlay.rs`).
    pub surface_sample_counts: Vec<u32>,
}

impl GpuContext {
    /// Create a new GPU context for the given window. `transparent_window`
    /// asks for a swapchain the desktop can show through; the window itself
    /// has to have been created transparent for that to mean anything.
    pub async fn new(window: Arc<Window>, vsync: bool, transparent_window: bool) -> Self {
        let size = window.inner_size();

        // Create wgpu instance - prefer Vulkan on Linux to avoid EGL issues
        let instance = Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        // Create surface from window
        let surface = instance
            .create_surface(window.clone())
            .expect("Failed to create surface");

        // Request adapter
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a suitable GPU adapter");

        log::info!("Using adapter: {:?}", adapter.get_info());

        // Check for optional features
        let adapter_features = adapter.features();
        let has_f16 = adapter_features.contains(Features::SHADER_F16);
        log::info!("GPU capabilities: f16={}", has_f16);

        // Request f16 if available (needed for density renderer, not for points renderer)
        let mut required_features = if has_f16 {
            Features::SHADER_F16
        } else {
            Features::empty()
        };

        // The overlay pass wants 2x MSAA, which the WebGPU spec doesn't
        // guarantee for any format — only 1 and 4 are. Most desktop adapters
        // have it, but wgpu will refuse to create the texture unless the
        // device was asked for adapter-specific format features. Ask when it's
        // there; `surface_sample_counts` below reports what we actually got,
        // and `OverlayTargets::choose_samples` falls back to 4.
        let adapter_format_features =
            adapter_features.contains(Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
        if adapter_format_features {
            required_features |= Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        }

        // Default limits cap storage buffer bindings at 128MiB, which caps the
        // point buffer at ~8M points. Request whatever the adapter supports.
        let adapter_limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
            max_buffer_size: adapter_limits.max_buffer_size,
            // The chaos pass binds eight storage buffers since xaos — exactly the
            // WebGPU default. Ask for the adapter's figure so the next one isn't a
            // device-creation failure.
            max_storage_buffers_per_shader_stage: adapter_limits.max_storage_buffers_per_shader_stage,
            ..wgpu::Limits::default()
        };
        log::info!(
            "Storage buffer binding limit: {} MiB",
            adapter_limits.max_storage_buffer_binding_size / (1024 * 1024)
        );

        // Request device and queue
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("fracturize_device"),
                    required_features,
                    required_limits,
                    memory_hints: wgpu::MemoryHints::Performance,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    trace: Default::default(),
                },
            )
            .await
            .expect("Failed to create device");

        // Get preferred surface format (sRGB for gamma-correct output)
        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        log::info!("Using surface format: {:?}", format);

        // A transparent window wants the compositor to read our alpha, which
        // the first mode (Opaque, nearly always) tells it not to. The shaders
        // write premultiplied alpha for it (`Alpha::Premultiplied`), so prefer
        // the mode that says so; Inherit means "whatever the window's visual
        // says", and an X11 ARGB visual says premultiplied too. Some drivers
        // (NVIDIA on X11) offer only Opaque, and present the alpha anyway.
        let alpha_mode = if transparent_window {
            [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::Inherit]
                .into_iter()
                .find(|m| surface_caps.alpha_modes.contains(m))
                .unwrap_or(surface_caps.alpha_modes[0])
        } else {
            surface_caps.alpha_modes[0]
        };
        log::info!(
            "Surface alpha mode: {:?} (of {:?})",
            alpha_mode,
            surface_caps.alpha_modes
        );

        // Configure surface
        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: if vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Without the adapter-specific feature the device is held to the
        // WebGPU guarantee regardless of what the adapter reports, so don't
        // report more than we can actually create.
        let flags = adapter.get_texture_format_features(format).flags;
        let surface_sample_counts: Vec<u32> = [1u32, 2, 4, 8]
            .into_iter()
            .filter(|n| {
                if adapter_format_features {
                    flags.sample_count_supported(*n)
                } else {
                    *n == 1 || *n == 4
                }
            })
            .collect();
        log::info!("Surface MSAA sample counts: {:?}", surface_sample_counts);

        Self {
            device,
            queue,
            surface,
            config,
            format,
            _has_f16: has_f16,
            surface_sample_counts,
        }
    }

    /// Resize the surface when window size changes
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Get current surface dimensions
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }
}
