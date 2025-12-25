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
    keyboard::{KeyCode, PhysicalKey},
    window::{WindowAttributes, WindowId},
};

use app::App;
use scene::Scene;

/// 3D IFS Fractal Renderer inspired by Apophysis
#[derive(Parser, Debug)]
#[command(name = "fracturize")]
#[command(about = "3D IFS Fractal Renderer", long_about = None)]
struct Args {
    /// Scene file to load (TOML format)
    #[arg(long, default_value = "scenes/sierpinski.toml")]
    scene: String,

    /// Capture screenshot and exit after delay
    #[arg(long)]
    screenshot: bool,

    /// Frames to wait before screenshot capture
    #[arg(long, default_value = "120")]
    delay: u32,
}

/// Wrapper to handle winit's async initialization pattern
struct AppWrapper {
    app: Option<App>,
    args: Args,
}

impl AppWrapper {
    fn new(args: Args) -> Self {
        Self { app: None, args }
    }
}

impl ApplicationHandler for AppWrapper {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }

        // Create window
        let window_attrs = WindowAttributes::default()
            .with_title("Fracturize - 3D IFS Fractal Renderer")
            .with_inner_size(LogicalSize::new(1280, 720));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("Failed to create window"),
        );

        // Load scene
        let scene_path = &self.args.scene;
        let scene = Scene::load(scene_path).unwrap_or_else(|e| {
            log::warn!("Failed to load scene '{}': {}", scene_path, e);
            log::info!("Using built-in default scene");
            default_scene()
        });

        // Create app (blocking on async)
        let app = pollster::block_on(App::new(window, scene));

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
                if event.state.is_pressed() {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            app.reset();
                            log::info!("Reset");
                        }
                        PhysicalKey::Code(KeyCode::ArrowUp) => {
                            app.zoom_in();
                        }
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            app.zoom_out();
                        }
                        PhysicalKey::Code(KeyCode::KeyS) => {
                            app.request_screenshot();
                            log::info!("Screenshot requested");
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
    Scene {
        name: "Default Sierpinski".to_string(),
        point_size: 0.012,
        iters: 50_000,
        max_points: 200_000,
        transforms: vec![
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.0, 0.0, 0.5),
                ),
                Vec3::new(1.0, 0.2, 0.2),
                1.0,
            ),
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.0, 0.47, -0.17),
                ),
                Vec3::new(0.2, 1.0, 0.2),
                1.0,
            ),
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(-0.41, -0.24, -0.17),
                ),
                Vec3::new(0.2, 0.2, 1.0),
                1.0,
            ),
            (
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.41, -0.24, -0.17),
                ),
                Vec3::new(1.0, 1.0, 0.2),
                1.0,
            ),
        ],
    }
}

fn main() {
    // Initialize logging
    env_logger::init();

    // Parse CLI args
    let args = Args::parse();
    log::info!("Starting Fracturize with scene: {}", args.scene);

    // Create event loop
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    // Run application
    let mut app_wrapper = AppWrapper::new(args);
    event_loop
        .run_app(&mut app_wrapper)
        .expect("Event loop error");
}
