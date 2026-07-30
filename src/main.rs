mod app;
mod avif;
mod camera;
mod haze;
mod gpu;
mod history;
mod indicators;
mod mutate;
mod offline;
mod path;
mod prefs;
mod trace;
mod pick;
mod scene;
mod randomize;
mod render_job;
mod ui;
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

    /// Turn on atmospheric haze at the legacy default strength. A scene's
    /// own `haze` value wins over this; see "Haze" in AGENTS.md.
    #[arg(long)]
    fog: bool,

    /// Disable vsync (uncapped frame rate, useful for benchmarking)
    #[arg(long)]
    no_vsync: bool,

    /// Load a saved view file (camera framing, point size, haze).
    /// In windowed mode the orbit starts paused; press O to resume.
    #[arg(long)]
    view: Option<String>,

    /// Render headlessly (no window) and exit. A .png path renders stills
    /// (and grids); a .avif path renders an animation along the scene's
    /// [[camera.path]] (or a full-orbit loop when the scene has none).
    /// Prints camera mapping (for grids) and a timing breakdown to stdout.
    #[arg(long)]
    render: Option<String>,

    /// Animation frame rate for --render <out.avif>
    #[arg(long, default_value = "30")]
    fps: u32,

    /// Animation duration in seconds (default: the path's own duration —
    /// path_seconds in the scene, or 3s per spline segment)
    #[arg(long)]
    seconds: Option<f32>,

    /// Animation AV1 quality, 0-100 (higher = better quality, bigger file)
    #[arg(long, default_value = "60")]
    quality: u8,

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

    /// Use the splat renderer: additive log-density accumulation
    /// (flame-style) instead of opaque points. R toggles it in-app.
    #[arg(long)]
    splat: bool,

    /// Splat-renderer exposure multiplier (default 1.0, or the view's saved
    /// value). W / Shift+W adjust it in-app.
    #[arg(long)]
    exposure: Option<f32>,

    /// Render with a transparent background: the PNG gets an alpha channel
    /// carrying the fractal's own coverage, for compositing. Not supported for
    /// .avif output — the AV1 muxer has no alpha plane.
    #[arg(long)]
    transparent: bool,

    /// Start from a randomly generated flame instead of a scene file.
    /// Quality-checked on the CPU before it's handed back, so it always
    /// renders. Pair with --seed to reproduce a roll (the seed used is
    /// always logged); combine with --render to explore offline.
    #[arg(long, conflicts_with = "scene")]
    random: bool,

    /// Start from a blank scene instead of a scene file: two plain half-scale
    /// transforms and nothing else, for building an IFS up from nothing.
    #[arg(long, conflicts_with_all = ["scene", "random"])]
    blank: bool,
}

/// Roll a random flame for `--random`, honouring `--seed` and logging the
/// seed either way so any roll can be reproduced exactly.
fn random_scene(seed: Option<u64>) -> Scene {
    use rand::SeedableRng;
    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let scene = randomize::random_flame(&mut rng);
    log::info!("Random flame seed: {} (reproduce with --random --seed {})", seed, seed);
    println!("Random flame seed: {}", seed);
    scene
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

/// Wrapper to handle winit's async initialization pattern.
///
/// Owns the egui layer alongside `app` (rather than `App` owning it): the
/// per-frame UI closure needs `&mut App`, so `AppWrapper` must be able to
/// split-borrow `self.app` and `self.egui` independently.
struct AppWrapper {
    app: Option<App>,
    egui: Option<ui::EguiLayer>,
    args: Args,
}

impl AppWrapper {
    fn new(args: Args) -> Self {
        Self { app: None, egui: None, args }
    }
}

impl ApplicationHandler for AppWrapper {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }

        let window_attrs = WindowAttributes::default()
            .with_title("Fracturize - 3D IFS Fractal Renderer")
            // Roomy enough for the toolbar, a couple of floating panels and
            // the status bar without eating a 1080p desktop whole.
            .with_inner_size(PhysicalSize::new(1440u32, 860u32));

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
            None if self.args.blank => Scene::blank(),
            None if self.args.random => random_scene(self.args.seed),
            None => {
                log::info!("No scene specified, using built-in default");
                default_scene()
            }
        };
        // Point count is a render property, not scene data (todo.txt): the
        // Render window edits it and persists it to prefs, so it follows the
        // person across scenes. Precedence: --points > prefs > the scene
        // file's own point_count > the built-in default. `App::new` re-reads
        // prefs for its own copy; this read is only for the decision.
        if let Some(n) = self.args.points {
            scene.point_count = n;
        } else if let Some(n) = crate::prefs::Prefs::load().point_count {
            scene.point_count = n;
        }

        let view = self.args.view.as_ref().map(|path| {
            View::load(path).unwrap_or_else(|e| panic!("Failed to load view '{}': {}", path, e))
        });

        // Create app (blocking on async)
        let app = pollster::block_on(App::new(
            window.clone(),
            scene,
            self.args.fog,
            !self.args.no_vsync,
            self.args.scene.clone(),
            view,
            self.args.splat,
            self.args.exposure,
        ));

        self.egui = Some(ui::EguiLayer::new(&window, &app.gpu.device, app.gpu.format));
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
        let Some(egui) = self.egui.as_mut() else {
            return;
        };

        // egui must see every event first.
        let resp = egui.state.on_window_event(&app.window, &event);

        // Modifiers and cursor position always update app state, regardless
        // of whether egui consumed the event or a drag is in flight.
        if let WindowEvent::ModifiersChanged(mods) = &event {
            app.shift_held = mods.state().shift_key();
            app.ctrl_held = mods.state().control_key();
        }
        if let WindowEvent::CursorMoved { position, .. } = &event {
            // Suppress gizmo hover picking while the pointer is over an egui
            // area, unless the app is already mid-drag (active viewport
            // drags keep receiving motion even over panels).
            let suppress_hover = egui.ctx.is_pointer_over_egui() && !app.has_active_drag();
            app.on_cursor_moved(position.x as f32, position.y as f32, suppress_hover);
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::ModifiersChanged(_) => {} // handled above

            WindowEvent::CursorMoved { .. } => {} // handled above

            WindowEvent::MouseInput { state, button, .. } => {
                let is_release = !state.is_pressed();
                let gated = resp.consumed || egui.ctx.egui_wants_pointer_input();
                // A release always reaches the app while a drag is active,
                // so drags ending over a panel don't stick.
                if !gated || (is_release && app.has_active_drag()) {
                    if state.is_pressed() {
                        app.on_mouse_press(button);
                    } else {
                        app.on_mouse_release(button);
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let gated = resp.consumed || egui.ctx.egui_wants_pointer_input();
                if !gated {
                    let steps = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                    };
                    app.on_scroll(steps);
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // egui owns keyboard focus (e.g. typing into a text field),
                // OR the pointer is mid-drag on an egui widget (e.g. a
                // Slider — which doesn't take keyboard focus, only pointer
                // capture) — either way, don't also run keybinds. Without
                // the second check, S/F/D etc. leak through while dragging
                // a Render-window slider.
                if egui.ctx.egui_wants_keyboard_input() || egui.ctx.egui_is_using_pointer() {
                } else if event.state.is_pressed() {
                    // Handle special keys by physical key (layout-independent)
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            // Escape quits, which makes it the worst possible
                            // key to leave unhandled while something modal is
                            // up: dismiss that first.
                            if app.ui_state.transform_menu.is_some() {
                                app.ui_state.transform_menu = None;
                            } else if app.ui_state.save_as.is_open() {
                                app.ui_state.save_as = Default::default();
                            } else if app.show_browser {
                                app.toggle_browser();
                            } else {
                                event_loop.exit();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowUp) => {
                            if app.show_browser {
                                app.browser_move(false);
                            } else if app.selected_transform().is_some() {
                                app.select_prev_transform();
                            } else {
                                app.zoom_in();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            if app.show_browser {
                                app.browser_move(true);
                            } else if app.selected_transform().is_some() {
                                app.select_next_transform();
                            } else {
                                app.zoom_out();
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            if app.show_browser {
                                app.browser_load_selected();
                            } else if app.selected_transform().is_some() {
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
                                if app.ctrl_held && app.shift_held {
                                    ui::save_as::open(app);
                                } else if app.ctrl_held {
                                    app.save_scene();
                                } else {
                                    app.request_screenshot();
                                    log::info!("Screenshot requested");
                                }
                            }
                            "f" | "F" => {
                                app.adjust_haze_intensity(!app.shift_held);
                            }
                            "d" | "D" => {
                                app.adjust_color_falloff(!app.shift_held);
                            }
                            "c" | "C" => {
                                app.adjust_color_contrast(app.shift_held);
                            }
                            "g" | "G" => {
                                app.toggle_gizmos();
                            }
                            "h" | "H" | "?" => {
                                app.toggle_help();
                            }
                            "o" | "O" => {
                                app.toggle_camera_motion();
                            }
                            "z" | "Z" => {
                                // Ctrl must be checked first: Ctrl+Z / Ctrl+Shift+Z
                                // are undo/redo, not the path-play toggle.
                                if app.ctrl_held {
                                    if app.shift_held {
                                        app.redo();
                                    } else {
                                        app.undo();
                                    }
                                } else {
                                    // Same action as O. One motion, so one verb
                                    // — two keys for it, because both were in
                                    // people's fingers before they merged.
                                    app.toggle_camera_motion();
                                }
                            }
                            "y" | "Y" => {
                                if app.ctrl_held {
                                    app.toggle_path_closed();
                                } else if app.shift_held {
                                    app.remove_path_key();
                                } else {
                                    app.add_path_key();
                                }
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
                                ui::render_job::open(app);
                            }
                            "i" | "I" => {
                                app.toggle_invert_pitch();
                            }
                            "x" | "X" => {
                                app.toggle_traces(app.shift_held);
                            }
                            "u" | "U" => {
                                if app.shift_held {
                                    // Shift+U aliases the unified undo (kept for
                                    // muscle memory; identical to Ctrl+Z).
                                    app.undo();
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
                            "r" | "R" => {
                                app.toggle_render_mode();
                            }
                            "w" | "W" => {
                                app.adjust_exposure(!app.shift_held);
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

                // Frame flow: gather input -> run the egui pass -> hand
                // platform output back -> tessellate -> render (egui pass
                // replaces the old text-overlay pass inside App::render).
                let ui_start = std::time::Instant::now();
                let raw_input = egui.state.take_egui_input(&app.window);
                let full_output = egui.ctx.run_ui(raw_input, |ui| {
                    ui::draw(ui, app);
                });
                let t_run = ui_start.elapsed();
                egui.state.handle_platform_output(&app.window, full_output.platform_output);
                let t_plat = ui_start.elapsed();
                let pixels_per_point = full_output.pixels_per_point;
                let paint_jobs = egui.ctx.tessellate(full_output.shapes, pixels_per_point);
                static UI_PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                let ui_profile =
                    *UI_PROFILE.get_or_init(|| std::env::var_os("FRACTURIZE_UI_PROFILE").is_some());
                if ui_profile && app.frame_count % 120 == 0 {
                    let t_tess = ui_start.elapsed();
                    log::info!(
                        "ui: run_ui {:.2}ms, platform_output {:.2}ms, tessellate {:.2}ms",
                        t_run.as_secs_f32() * 1000.0,
                        (t_plat - t_run).as_secs_f32() * 1000.0,
                        (t_tess - t_plat).as_secs_f32() * 1000.0,
                    );
                }
                // The UI's own CPU cost, surfaced in the status bar: this is a
                // performance-sensitive app and "are the panels costing me
                // frames?" needs an answer you can read, not guess at.
                app.record_ui_time(ui_start.elapsed().as_secs_f32() * 1000.0);

                match app.render(
                    &mut egui.renderer,
                    &paint_jobs,
                    &full_output.textures_delta,
                    pixels_per_point,
                ) {
                    app::FrameOutcome::Presented | app::FrameOutcome::Skip => {}
                    app::FrameOutcome::Reconfigure => {
                        let (w, h) = app.gpu.size();
                        app.resize(w, h);
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
        haze: 0.0,
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
        camera_roll: default_cam.roll,
        background: scene::DEFAULT_BACKGROUND,
        camera_path: None,
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
            None if args.blank => Scene::blank(),
            None if args.random => random_scene(args.seed),
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

        // --splat wins; otherwise a splat-captured view selects the renderer
        let splat = args.splat || view.as_ref().is_some_and(|v| v.is_splat());
        let exposure = args
            .exposure
            .or(view.as_ref().and_then(|v| v.exposure))
            .unwrap_or(1.0);

        let params = offline::OfflineParams {
            scene,
            view,
            width: args.width,
            height: args.height,
            out_path: std::path::Path::new(out),
            accumulate,
            haze_enabled: args.fog,
            grid,
            splat,
            exposure,
            transparent: args.transparent,
            control: None,
        };
        let animated = std::path::Path::new(out)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("avif"));
        let result = if animated {
            if !matches!(grid, offline::GridMode::Single) || args.mutations.is_some() {
                eprintln!("animation (.avif) cannot be combined with grid or mutation sheets");
                std::process::exit(1);
            }
            if args.transparent {
                // Fail rather than quietly hand back opaque video: src/avif.rs
                // reads only r/g/b converting to YUV, so there is nowhere for
                // an alpha plane to go without muxer work.
                eprintln!(
                    "--transparent is not supported for .avif output (the AV1 muxer has no \
                     alpha plane). Render a PNG sequence instead, or drop --transparent."
                );
                std::process::exit(1);
            }
            offline::render_animation(
                params,
                offline::AnimParams {
                    fps: args.fps,
                    seconds: args.seconds,
                    quality: args.quality,
                },
            )
        } else {
            match args.mutations {
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
            }
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
