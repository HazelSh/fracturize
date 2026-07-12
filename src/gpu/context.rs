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
}

impl GpuContext {
    /// Create a new GPU context for the given window
    pub async fn new(window: Arc<Window>, vsync: bool) -> Self {
        let size = window.inner_size();

        // Create wgpu instance - prefer Vulkan on Linux to avoid EGL issues
        let instance = Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
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
        let required_features = if has_f16 {
            Features::SHADER_F16
        } else {
            Features::empty()
        };

        // Default limits cap storage buffer bindings at 128MiB, which caps the
        // point buffer at ~8M points. Request whatever the adapter supports.
        let adapter_limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
            max_buffer_size: adapter_limits.max_buffer_size,
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
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Self {
            device,
            queue,
            surface,
            config,
            format,
            _has_f16: has_f16,
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
