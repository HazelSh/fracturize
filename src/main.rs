mod app;
mod gpu;
mod offline;
mod scene;
mod view;

use std::sync::Arc;

use clap::Parser;
use glam::{Mat4, Vec3};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
    window::{WindowAttributes, WindowId},
};

use app::App;
use scene::{Scene, TransformSpec};
use view::View;

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

    /// Disable vsync (uncapped frame rate, useful for benchmarking)
    #[arg(long)]
    no_vsync: bool,

    /// Load a saved view file (camera framing, point size, fog).
    /// In windowed mode the orbit starts paused; press O to resume.
    #[arg(long)]
    view: Option<String>,

    /// Render a single frame headlessly (no window) to this PNG path and exit
    #[arg(long)]
    render: Option<String>,

    /// Output width for --render
    #[arg(long, default_value = "1920")]
    width: u32,

    /// Output height for --render
    #[arg(long, default_value = "1080")]
    height: u32,

    /// Override the scene's point buffer capacity (more points = denser render)
    #[arg(long)]
    points: Option<usize>,

    /// Extra chaos-game frames after the buffer fills, for --render
    #[arg(long, default_value = "32")]
    accumulate: u32,
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
        // Bit layout: x:11 (max 2048) | y:10 (max 1024) | depth:11
        // Use 16:9 aspect ratio that fits within limits (PhysicalSize ignores DPI scaling)
        let window_attrs = WindowAttributes::default()
            .with_title("Fracturize - 3D IFS Fractal Renderer")
            .with_inner_size(PhysicalSize::new(1280u32, 720u32))
            .with_resizable(false);

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("Failed to create window"),
        );

        // Load scene - panic if provided path fails, use default if no path given
        let mut scene = match &self.args.scene {
            Some(path) => Scene::load(path).unwrap_or_else(|e| {
                panic!("Failed to load scene '{}': {}", path, e);
            }),
            None => {
                log::info!("No scene specified, using built-in default");
                default_scene()
            }
        };
        if let Some(n) = self.args.points {
            scene.point_count = n;
        }

        let view = self.args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| panic!("Failed to load view '{}': {}", path, e))
        });

        // Create app (blocking on async)
        let app = pollster::block_on(App::new(
            window,
            scene,
            self.args.fog,
            !self.args.no_vsync,
            self.args.scene.clone(),
            view,
        ));

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
                            if app.show_text {
                                app.select_prev_transform();
                            } else {
                                app.zoom_in();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            if app.show_text {
                                app.select_next_transform();
                            } else {
                                app.zoom_out();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            if app.show_text {
                                app.toggle_selected_transform();
                            }
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
                            "g" | "G" => {
                                app.toggle_gizmos();
                            }
                            "t" | "T" => {
                                app.toggle_text_overlay();
                            }
                            "h" | "H" | "?" => {
                                app.toggle_help();
                            }
                            "o" | "O" => {
                                app.toggle_orbit();
                            }
                            "v" | "V" => {
                                app.save_view();
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
        point_count: 500_000,
        decay: 0.8,
        color_speed: 0.5,
        transform_names: vec![None; 4],
        transforms: [
            (Vec3::new(0.0, 0.0, 0.5), 0.0),      // red
            (Vec3::new(0.0, 0.47, -0.17), 0.333), // green
            (Vec3::new(-0.41, -0.24, -0.17), 0.667), // blue
            (Vec3::new(0.41, -0.24, -0.17), 1.0), // yellow
        ]
        .iter()
        .map(|&(translation, color_value)| TransformSpec {
            matrix: Mat4::from_scale_rotation_translation(
                Vec3::splat(0.5),
                glam::Quat::IDENTITY,
                translation,
            ),
            color_value,
            weight: 1.0,
            color_speed: 0.5,
            color_delay: 0,
            color_detail: 1.0,
            variations: TransformSpec::linear_variations(),
        })
        .collect(),
        colormap,
        camera_focus: Vec3::ZERO,
        camera_offset: Vec3::new(0.0, 1.0, 0.0),
        camera_distance: 3.0,
    }
}

fn main() {
    // Initialize logging
    env_logger::init();

    // Parse CLI args
    let args = Args::parse();

    // Headless single-frame render mode: no window, no event loop
    if let Some(out) = &args.render {
        let mut scene = match &args.scene {
            Some(path) => Scene::load(path).unwrap_or_else(|e| {
                panic!("Failed to load scene '{}': {}", path, e);
            }),
            None => default_scene(),
        };
        if let Some(n) = args.points {
            scene.point_count = n;
        }
        let view = args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| panic!("Failed to load view '{}': {}", path, e))
        });

        let result = offline::render(offline::OfflineParams {
            scene,
            view,
            width: args.width,
            height: args.height,
            out_path: std::path::Path::new(out),
            accumulate: args.accumulate,
            fog_enabled: args.fog,
        });
        if let Err(e) = result {
            eprintln!("Offline render failed: {}", e);
            std::process::exit(1);
        }
        return;
    }

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
