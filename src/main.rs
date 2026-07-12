mod app;
mod camera;
mod gpu;
mod mutate;
mod offline;
mod prefs;
mod trace;
mod pick;
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

    /// Render headlessly (no window) to this PNG path and exit.
    /// Prints camera mapping (for grids) and a timing breakdown to stdout.
    #[arg(long)]
    render: Option<String>,

    /// Output width for --render (per tile when a grid mode is used)
    #[arg(long, default_value = "1920")]
    width: u32,

    /// Output height for --render (per tile when a grid mode is used)
    #[arg(long, default_value = "1080")]
    height: u32,

    /// Render effort preset for --render: sets point count and accumulation.
    /// Explicit --points / --accumulate override it.
    #[arg(long, value_enum)]
    effort: Option<Effort>,

    /// Override the scene's point buffer capacity (more points = denser render)
    #[arg(long)]
    points: Option<usize>,

    /// Extra chaos-game frames after the buffer fills, for --render (default 32)
    #[arg(long)]
    accumulate: Option<u32>,

    /// For --render: contact sheet of COLSxROWS views (e.g. 4x2) evenly
    /// spaced around a full horizontal orbit, one fill shared by all tiles
    #[arg(long, value_name = "COLSxROWS")]
    orbit_grid: Option<String>,

    /// For --render: contact sheet of COLSxROWS views (e.g. 3x3) with the
    /// camera nudged left/right (columns) and up/down (rows) in the view
    /// plane, all still looking at the focus
    #[arg(long, value_name = "COLSxROWS")]
    move_grid: Option<String>,

    /// Camera nudge per --move-grid step, as a fraction of orbit distance
    #[arg(long, default_value = "0.25")]
    move_step: f32,

    /// For --render: contact sheet of the scene plus N random mutations
    /// (tile 0 = original). Each variant is saved as <out>.mutN.toml and
    /// described on stdout. Mutually exclusive with the camera grids.
    #[arg(long, value_name = "N")]
    mutations: Option<u32>,

    /// Scale factor for mutation perturbations (--mutations)
    #[arg(long, default_value = "1.0")]
    mutation_strength: f32,

    /// RNG seed for --mutations (default: time-based; printed for reproduction)
    #[arg(long)]
    seed: Option<u64>,
}

/// Named effort presets for offline rendering: (points, accumulate frames).
/// draft is for fast composition checks, ultra for final frames on a real GPU.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum Effort {
    Draft,
    Low,
    Medium,
    High,
    Ultra,
}

impl Effort {
    fn preset(self) -> (usize, u32) {
        match self {
            Effort::Draft => (1_000_000, 4),
            Effort::Low => (4_000_000, 16),
            Effort::Medium => (12_000_000, 48),
            Effort::High => (40_000_000, 128),
            Effort::Ultra => (100_000_000, 256),
        }
    }
}

/// Parse a "COLSxROWS" grid spec like "4x2"
fn parse_grid(spec: &str) -> Result<(u32, u32), String> {
    let (c, r) = spec
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("Invalid grid '{}': expected COLSxROWS, e.g. 4x2", spec))?;
    let cols: u32 = c.trim().parse().map_err(|_| format!("Invalid grid columns '{}'", c))?;
    let rows: u32 = r.trim().parse().map_err(|_| format!("Invalid grid rows '{}'", r))?;
    if cols == 0 || rows == 0 || cols * rows > 64 {
        return Err(format!("Grid {}x{} out of range (1..=64 tiles)", cols, rows));
    }
    Ok((cols, rows))
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

        let window_attrs = WindowAttributes::default()
            .with_title("Fracturize - 3D IFS Fractal Renderer")
            .with_inner_size(PhysicalSize::new(1280u32, 720u32));

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

            WindowEvent::ModifiersChanged(mods) => {
                app.shift_held = mods.state().shift_key();
                app.ctrl_held = mods.state().control_key();
            }

            WindowEvent::CursorMoved { position, .. } => {
                app.on_cursor_moved(position.x as f32, position.y as f32);
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if state.is_pressed() {
                    app.on_mouse_press(button);
                } else {
                    app.on_mouse_release(button);
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                app.on_scroll(steps);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() {
                    // Handle special keys by physical key (layout-independent)
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            if app.show_browser {
                                app.toggle_browser();
                            } else {
                                event_loop.exit();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowUp) => {
                            if app.show_browser {
                                app.browser_move(false);
                            } else if app.show_text {
                                app.select_prev_transform();
                            } else {
                                app.zoom_in();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            if app.show_browser {
                                app.browser_move(true);
                            } else if app.show_text {
                                app.select_next_transform();
                            } else {
                                app.zoom_out();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            if app.show_browser {
                                app.browser_load_selected();
                            } else if app.show_text {
                                app.toggle_selected_transform();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Delete) => {
                            app.delete_selected_transform();
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
                                if app.ctrl_held {
                                    app.save_scene();
                                } else {
                                    app.request_screenshot();
                                    log::info!("Screenshot requested");
                                }
                            }
                            "f" | "F" => {
                                app.adjust_fog_intensity(!app.shift_held);
                            }
                            "d" | "D" => {
                                app.adjust_color_falloff(!app.shift_held);
                            }
                            "c" | "C" => {
                                app.adjust_color_contrast(app.shift_held);
                            }
                            "n" | "N" => {
                                app.adjust_fog_near(!app.shift_held);
                            }
                            "m" | "M" => {
                                app.adjust_fog_far(!app.shift_held);
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
                            "a" | "A" => {
                                app.add_transform(app.shift_held);
                            }
                            "b" | "B" => {
                                app.toggle_browser();
                            }
                            "p" | "P" => {
                                app.start_hq_render();
                            }
                            "i" | "I" => {
                                app.toggle_invert_pitch();
                            }
                            "x" | "X" => {
                                app.toggle_traces(app.shift_held);
                            }
                            "u" | "U" => {
                                if app.shift_held {
                                    app.undo_mutation();
                                } else {
                                    app.mutate_scene();
                                }
                            }
                            "," | "<" => {
                                app.adjust_weight(false);
                            }
                            "." | ">" => {
                                app.adjust_weight(true);
                            }
                            "j" | "J" => {
                                app.adjust_color(0, !app.shift_held);
                            }
                            "k" | "K" => {
                                app.adjust_color(1, !app.shift_held);
                            }
                            "l" | "L" => {
                                app.adjust_color(2, !app.shift_held);
                            }
                            "e" | "E" => {
                                app.cycle_variation(!app.shift_held);
                            }
                            "-" | "_" => {
                                app.adjust_variation_weight(false);
                            }
                            "=" | "+" => {
                                app.adjust_variation_weight(true);
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

    // Legacy elevated-eye framing, folded onto the orbit sphere
    let default_cam = camera::OrbitCamera::from_legacy(
        Vec3::ZERO,
        Vec3::new(0.0, 1.0, 0.0),
        3.0,
        0.0,
        0.0,
    );

    Scene {
        name: "Default Sierpinski".to_string(),
        author: "Claude Opus 4.5 (Claude Code 2.0.76)".to_string(),
        point_size: 0.002,
        points_per_frame: 100_000,
        point_count: 500_000,
        decay: 0.8,
        color_speed: 0.5,
        color_falloff: 0.0,
        color_contrast: 1.0,
        transform_names: vec![None; 4],
        colors: colors.to_vec(),
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
            explicit_color_speed: None,
            variations: TransformSpec::linear_variations(),
        })
        .collect(),
        colormap,
        camera_focus: default_cam.focus,
        camera_distance: default_cam.distance,
        camera_yaw: default_cam.yaw,
        camera_pitch: default_cam.pitch,
    }
}

fn main() {
    // Initialize logging
    env_logger::init();

    // Parse CLI args
    let args = Args::parse();

    // Headless render mode: no window, no event loop
    if let Some(out) = &args.render {
        let mut scene = match &args.scene {
            Some(path) => Scene::load(path).unwrap_or_else(|e| {
                panic!("Failed to load scene '{}': {}", path, e);
            }),
            None => default_scene(),
        };

        // Effort presets set points + accumulation; explicit flags win
        let (effort_points, effort_accumulate) = match args.effort {
            Some(e) => {
                let (p, a) = e.preset();
                (Some(p), Some(a))
            }
            None => (None, None),
        };
        if let Some(n) = args.points.or(effort_points) {
            scene.point_count = n;
        }
        let accumulate = args.accumulate.or(effort_accumulate).unwrap_or(32);

        let view = args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| panic!("Failed to load view '{}': {}", path, e))
        });

        let grid = match (&args.orbit_grid, &args.move_grid) {
            (Some(_), Some(_)) => {
                eprintln!("--orbit-grid and --move-grid are mutually exclusive");
                std::process::exit(1);
            }
            (Some(spec), None) => match parse_grid(spec) {
                Ok((cols, rows)) => offline::GridMode::Orbit { cols, rows },
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            },
            (None, Some(spec)) => match parse_grid(spec) {
                Ok((cols, rows)) => offline::GridMode::Move {
                    cols,
                    rows,
                    step: args.move_step,
                },
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            },
            (None, None) => offline::GridMode::Single,
        };

        let params = offline::OfflineParams {
            scene,
            view,
            width: args.width,
            height: args.height,
            out_path: std::path::Path::new(out),
            accumulate,
            fog_enabled: args.fog,
            grid,
        };
        let result = match args.mutations {
            Some(n) => {
                if !matches!(grid, offline::GridMode::Single) {
                    eprintln!("--mutations cannot be combined with --orbit-grid/--move-grid");
                    std::process::exit(1);
                }
                if n == 0 || n > 24 {
                    eprintln!("--mutations must be 1..=24");
                    std::process::exit(1);
                }
                offline::render_mutations(params, n, args.mutation_strength, args.seed)
            }
            None => offline::render(params),
        };
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
