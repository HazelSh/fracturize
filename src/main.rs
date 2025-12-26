mod app;
mod gpu;
mod scene;

use std::sync::Arc;

use clap::Parser;
use glam::{Mat4, Vec3};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
    window::{WindowAttributes, WindowId},
};

use app::App;
use scene::Scene;

/// 3D IFS Fractal Renderer inspired by Apophysis
#[derive(Parser, Debug)]
#[command(name = "fracturize")]
#[command(about = "3D IFS Fractal Renderer", long_about = None)]
struct Args {
    /// Scene file to load (TOML format). If not provided, uses built-in default.
    #[arg(long)]
    scene: Option<String>,

    /// Capture screenshot and exit after delay
    #[arg(long)]
    screenshot: bool,

    /// Frames to wait before screenshot capture
    #[arg(long, default_value = "120")]
    delay: u32,

    /// Enable fog effect for depth perception
    #[arg(long)]
    fog: bool,
}

/// Wrapper to handle winit's async initialization pattern
struct AppWrapper {
    app: Option<App>,
    args: Args,
    shift_held: bool,
}

impl AppWrapper {
    fn new(args: Args) -> Self {
        Self { app: None, args, shift_held: false }
    }
}

impl ApplicationHandler for AppWrapper {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }

        // Create window (fixed size for hash grid alignment)
        let window_attrs = WindowAttributes::default()
            .with_title("Fracturize - 3D IFS Fractal Renderer")
            .with_inner_size(LogicalSize::new(1024, 768))
            .with_resizable(false);

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("Failed to create window"),
        );

        // Load scene - panic if provided path fails, use default if no path given
        let scene = match &self.args.scene {
            Some(path) => Scene::load(path).unwrap_or_else(|e| {
                panic!("Failed to load scene '{}': {}", path, e);
            }),
            None => {
                log::info!("No scene specified, using built-in default");
                default_scene()
            }
        };

        // Create app (blocking on async)
        let app = pollster::block_on(App::new(window, scene, self.args.fog));

        self.app = Some(app);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(app) = self.app.as_mut() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Track shift key state
                if let PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight) = event.physical_key {
                    self.shift_held = event.state.is_pressed();
                }

                if event.state.is_pressed() {
                    // Handle special keys by physical key (layout-independent)
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowUp) => {
                            app.zoom_in();
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            app.zoom_out();
                            return;
                        }
                        _ => {}
                    }

                    // Handle letter keys by logical key (respects keyboard layout)
                    match &event.logical_key {
                        Key::Named(NamedKey::Space) => {
                            app.reset();
                            log::info!("Reset");
                        }
                        Key::Character(c) => match c.as_str() {
                            "s" | "S" => {
                                app.request_screenshot();
                                log::info!("Screenshot requested");
                            }
                            "f" | "F" => {
                                app.adjust_fog_intensity(!self.shift_held);
                            }
                            "n" | "N" => {
                                app.adjust_fog_near(!self.shift_held);
                            }
                            "m" | "M" => {
                                app.adjust_fog_far(!self.shift_held);
                            }
                            "=" | "+" => {
                                app.adjust_point_count(true);
                            }
                            "-" | "_" => {
                                app.adjust_point_count(false);
                            }
                            "[" => {
                                app.adjust_point_size(false);
                            }
                            "]" => {
                                app.adjust_point_size(true);
                            }
                            _ => {}
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::Resized(new_size) => {
                app.resize(new_size.width, new_size.height);
                app.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                app.update();

                // Handle --screenshot mode: take screenshot after delay and exit
                if self.args.screenshot && app.frame_count == self.args.delay {
                    app.request_screenshot();
                }

                match app.render() {
                    Ok(_) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        let (w, h) = app.gpu.size();
                        app.resize(w, h);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        log::error!("Out of GPU memory!");
                        event_loop.exit();
                    }
                    Err(e) => {
                        log::warn!("Surface error: {:?}", e);
                    }
                }

                // Exit after screenshot in --screenshot mode
                if self.args.screenshot && app.frame_count > self.args.delay {
                    event_loop.exit();
                    return;
                }

                // Request next frame
                app.window.request_redraw();
            }

            _ => {}
        }
    }
}

/// Default Sierpinski tetrahedron scene
fn default_scene() -> Scene {
    // Generate colormap from transform colors
    let colors = [
        Vec3::new(1.0, 0.2, 0.2), // Red
        Vec3::new(0.2, 1.0, 0.2), // Green
        Vec3::new(0.2, 0.2, 1.0), // Blue
        Vec3::new(1.0, 1.0, 0.2), // Yellow
    ];

    let mut colormap = [[0.0f32; 4]; 256];
    for i in 0..256 {
        let t = i as f32 / 255.0;
        let scaled = t * 3.0;
        let idx0 = (scaled.floor() as usize).min(2);
        let idx1 = idx0 + 1;
        let local_t = scaled - idx0 as f32;
        let c = colors[idx0] * (1.0 - local_t) + colors[idx1] * local_t;
        colormap[i] = [c.x, c.y, c.z, 1.0];
    }

    Scene {
        name: "Default Sierpinski".to_string(),
        author: "Claude Opus 4.5 (Claude Code 2.0.76)".to_string(),
        point_size: 0.002,
        points_per_frame: 100_000,
        decay: 0.8,
        color_speed: 0.5,
        transforms: vec![
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.0, 0.0, 0.5),
                ),
                0.0,  // color_value (maps to red)
                1.0,
                0.5,
            ),
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.0, 0.47, -0.17),
                ),
                0.333,  // color_value (maps to green)
                1.0,
                0.5,
            ),
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(-0.41, -0.24, -0.17),
                ),
                0.667,  // color_value (maps to blue)
                1.0,
                0.5,
            ),
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.41, -0.24, -0.17),
                ),
                1.0,  // color_value (maps to yellow)
                1.0,
                0.5,
            ),
        ],
        colormap,
    }
}

fn main() {
    // Initialize logging
    env_logger::init();

    // Parse CLI args
    let args = Args::parse();
    match &args.scene {
        Some(path) => log::info!("Starting Fracturize with scene: {}", path),
        None => log::info!("Starting Fracturize with built-in default scene"),
    }

    // Create event loop
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    // Run application
    let mut app_wrapper = AppWrapper::new(args);
    event_loop
        .run_app(&mut app_wrapper)
        .expect("Event loop error");
}
