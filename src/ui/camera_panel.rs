//! Camera window: framing, saved views, the camera path, and the output actions
//! that capture what's on screen (screenshot, render job, save scene).
//!
//! There is exactly one path here, because there is exactly one path (see
//! `App::camera_path`): a scene's own keypoints when it has two or more,
//! otherwise the default full orbit. The panel doesn't distinguish them beyond
//! saying which you're looking at — the same list, the same transport, the same
//! loop and duration controls either way, and editing any of them is what turns
//! the default into scene data.
//!
//! Moving the camera isn't an edit to the artwork, so the framing controls are
//! not history entries — nobody expects Ctrl+Z to un-orbit a camera, and a drag
//! produces thousands of intermediate states, none of which are decisions.
//! Path keypoints are the other side of that line: adding one is a discrete,
//! deliberate authoring act that changes what Ctrl+S writes, so every one of
//! the six path operations goes through `App::commit_edit`.

use crate::app::App;
use crate::camera::{Fov, OrbitStyle};

use super::hints::hinted;
use super::icons;
use super::num;

/// Character budget for the `yaw · pitch · focus` readout.
const FRAMING_CHARS: usize = 46;
/// Budget for the horizontal-FoV readout beside the FoV slider: `150°h`.
const FOV_READOUT_CHARS: usize = 5;
/// Budget for a `N. d=X.XX yaw=X.XX pitch=X.XX` keypoint row.
const PATH_KEY_CHARS: usize = 36;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.camera_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Camera, "Camera")
        .open(&mut open)
        .show(ctx, |ui| {
            // The framing block and views are pinned to the top so the keypoint
            // list below gets every point of slack when the window is dragged
            // taller — that list is the one thing here that's ever long.
            egui::Panel::top("fracturize_camera_framing")
                .show(ui, |ui| {
                    draw_framing(ui, app);
                    ui.separator();
                    draw_views(ui, app);
                    ui.separator();
                });
            draw_path(ui, app);
        });

    super::remember(ctx, app, super::WindowKey::Camera);
    app.ui_state.panels.camera_open = open;
}

/// The two input preferences that used to head this window: orbit style and
/// invert pitch.
///
/// They live in the Controls window (`ui::shortcuts`) now, on scope grounds.
/// Neither moves the camera — they change how the *next drag is interpreted*,
/// which is a preference about the person's hands rather than state of the
/// artwork, and `App::set_orbit_style` says as much in its own doc. Drawn from
/// there so the radio's tooltips stay in one place.
pub fn draw_input_prefs(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Orbit");
        let style = app.orbit_style();
        let chosen = super::radio::radio(&mut app.ui_state, "orbit_style", style)
            .option(
                OrbitStyle::Trackball,
                "trackball",
                "Dragging sideways turns the scene around the screen's own up \
                 direction, so the controls feel the same wherever you are.\n\n\
                 The catch: dragging in circles slowly tilts the horizon. The \
                 Camera window's \"level\" button puts it back.",
                "click: orbit relative to the screen",
            )
            .option(
                OrbitStyle::Turntable,
                "turntable",
                "Dragging sideways turns the scene around the world's up \
                 direction, so the horizon stays level.\n\n\
                 The catch: looking straight up or down, a sideways drag spins \
                 the view instead of orbiting.",
                "click: orbit around world up, horizon stays level",
            )
            .show(ui);
        if let Some(style) = chosen {
            app.set_orbit_style(style);
        }
    });

    ui.horizontal(|ui| {
        // Vertical, because that is what the projection is built from and what
        // stays put as the window is resized. The readout beside it gives the
        // horizontal figure, which is the one games usually quote and the one
        // most people have a number in mind for.
        let mut fov = app.camera_fov().degrees();
        let resp = ui.add(
            egui::Slider::new(&mut fov, Fov::MIN_DEGREES..=Fov::MAX_DEGREES)
                .fixed_decimals(0)
                .suffix("°")
                .text("field of view"),
        );
        let aspect = app.surface_size().0 as f32 / app.surface_size().1.max(1) as f32;
        let natural = app.camera_fov().horizontal_degrees(aspect);
        let resp = hinted(
            resp,
            &mut app.ui_state,
            format!(
                "How wide the camera's lens is, measured top to bottom. Wider \
                 exaggerates depth; narrower flattens it. Moving the camera in as \
                 you widen the lens makes the perspective more dramatic.\n\n\
                 This is your own setting and applies to every scene, unless a \
                 scene sets its own.\n\n\
                 In this window that's {:.0}° side to side. The default is {:.0}°.",
                natural,
                crate::camera::DEFAULT_FOV_DEGREES,
            ),
            "drag: widen or narrow the lens",
        );
        if resp.changed() {
            app.set_fov_preference(Fov::from_degrees(fov));
        }
        // Fixed-width cell: "150°h" is the widest it gets, and a readout that
        // resizes its own row as you drag the slider above it is the vibration
        // `ui::num` exists to stop.
        num::text_cell(ui, &format!("{:>3.0}°h", natural), FOV_READOUT_CHARS);
    });

    ui.horizontal(|ui| {
        let mut invert = app.invert_pitch();
        let resp = ui.checkbox(&mut invert, "invert pitch");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Drag down to tilt the top of the scene toward you, like a flight \
             stick (I)",
            "click: invert mouse pitch",
        );
        if resp.changed() {
            app.toggle_invert_pitch();
        }
    });
}

fn draw_framing(ui: &mut egui::Ui, app: &mut App) {

    ui.horizontal(|ui| {
        let mut distance = app.camera.distance;
        // `distance` is 0.05..=100.0, so "distance: 100.00" is the widest it
        // gets — 16 characters, and the field is that wide whatever's in it, so
        // the roll field and the level button beside it hold still.
        let resp = num::drag(
            ui,
            16,
            2,
            num::Drag::new(&mut distance)
                .speed(0.02)
                .range(0.05..=100.0)
                .prefix("distance: "),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "How far the camera is from what it's looking at. Scrolling and \
             the Up and Down keys change it too",
            "drag: move the camera closer or further",
        );
        if resp.changed() {
            app.camera.distance = distance.clamp(0.05, 100.0);
            // Setting the framing by hand means taking the camera off the path,
            // the same as dragging in the viewport does.
            app.stop_camera_motion();
        }

        // Roll's home. Right-drag sets it, but a gesture with no readout
        // can't be undone precisely, and "level" is a thing you want back
        // exactly rather than approximately. Looking straight up or down,
        // yaw and roll are one control and the number here means nothing on
        // its own, so the field greys out rather than scrambling the framing.
        let faithful = app.camera.orientation.chart_is_faithful();
        let mut roll_deg = app.camera.chart().roll.degrees();
        let resp = ui.add_enabled_ui(faithful, |ui| {
            num::drag(
                ui,
                14,
                1,
                num::Drag::new(&mut roll_deg).speed(0.5).suffix("°").prefix("roll: "),
            )
        })
        .inner;
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if faithful {
                "Tilt the view sideways. Right-dragging the viewport does this too"
            } else {
                "Not available when looking straight up or down. Right-drag the \
                 viewport to spin the view instead"
            },
            "drag: roll the camera",
        );
        if resp.changed() {
            app.set_camera_roll(roll_deg.to_radians());
        }

        let resp = ui.add_enabled(!app.camera.is_level(), egui::Button::new((icons::TARGET, "level")));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Make the horizon level again",
            "click: level the camera",
        );
        if resp.clicked() {
            app.level_camera();
        }
    });

    // Five signed values that all cross zero while you orbit — the single
    // worst vibration source outside the status bar, because every one of them
    // flicks a minus sign in and out at whatever rate you're dragging.
    let chart = app.camera.chart();
    let focus = app.camera.focus;
    let text = format!(
        "yaw {} · pitch {} · focus ({}, {}, {})",
        num::cell(chart.yaw.radians(), 2, 5),
        num::cell(chart.pitch.radians(), 2, 5),
        num::cell(focus.x, 2, 6),
        num::cell(focus.y, 2, 6),
        num::cell(focus.z, 2, 6),
    );
    num::small_cell(ui, &text, FRAMING_CHARS);
}

fn draw_views(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Saved views").strong());
        let resp = ui.button((icons::SAVE, "Save current"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Save the current framing, with the point size, haze and colour \
             settings, to the views folder (V)",
            "click: save this view",
        );
        if resp.clicked() {
            app.save_view();
        }
    });

    // Cloned out of the cache so the row bodies below can take `&mut App`
    // (clicking one loads a view). The clone is a handful of small strings
    // and only happens while the panel is open.
    let views: Vec<(String, std::path::PathBuf)> = app.saved_views().to_vec();
    if views.is_empty() {
        ui.label(
            egui::RichText::new("No saved views for this scene yet.")
                .small()
                .weak(),
        );
        return;
    }

    // An editor left open on a view that is no longer in this list — the scene
    // changed under it — belongs to nothing, so drop it.
    if let Some((p, _)) = &app.ui_state.renaming_view {
        if !views.iter().any(|(_, path)| path == p) {
            app.ui_state.renaming_view = None;
        }
    }

    let mut load: Option<std::path::PathBuf> = None;
    let mut rename: Option<(std::path::PathBuf, String)> = None;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_saved_views")
        .max_height(110.0)
        .show(ui, |ui| {
            for (name, path) in &views {
                if app.ui_state.renaming_view.as_ref().is_some_and(|(p, _)| p == path) {
                    // Same in-place editor the Transforms pane header uses:
                    // the name becomes a field where it is read, Enter or
                    // click-away commits, Esc cancels.
                    let resp = {
                        let (_, buf) = app.ui_state.renaming_view.as_mut().unwrap();
                        ui.add(
                            egui::TextEdit::singleline(buf)
                                .id(egui::Id::new(("fracturize_view_name", path)))
                                .hint_text("name this view")
                                .desired_width(f32::INFINITY),
                        )
                    };
                    resp.request_focus();
                    let enter = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    let escape = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                    if escape {
                        app.ui_state.renaming_view = None;
                        app.ui_state.view_rename_error = None;
                    } else if enter || resp.lost_focus() {
                        if let Some((p, text)) = app.ui_state.renaming_view.take() {
                            rename = Some((p, text));
                        }
                    }
                    continue;
                }

                let shown = crate::app::view_display_name(name);
                let resp = ui.add(egui::Button::new(shown.as_str()).frame(false));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    path.display().to_string(),
                    "click: load this view · right-click: rename",
                );
                resp.context_menu(|ui| {
                    if ui.button((icons::PENCIL, "Rename")).clicked() {
                        // A name that is still the birth timestamp is not a
                        // name, so the field starts empty rather than
                        // prefilled with ten digits to delete first.
                        let seed = if name.parse::<u64>().is_ok() {
                            String::new()
                        } else {
                            name.clone()
                        };
                        app.ui_state.renaming_view = Some((path.clone(), seed));
                        app.ui_state.view_rename_error = None;
                        ui.close();
                    }
                });
                if resp.clicked() {
                    load = Some(path.clone());
                }
            }
        });
    if let Some((path, text)) = rename {
        app.ui_state.view_rename_error = app.rename_saved_view(&path, &text).err();
    }
    if let Some(err) = &app.ui_state.view_rename_error {
        ui.label(
            egui::RichText::new(err.clone())
                .small()
                .color(ui.visuals().warn_fg_color),
        );
    }
    if let Some(path) = load {
        app.load_saved_view(&path);
    }
}

fn draw_path(ui: &mut egui::Ui, app: &mut App) {
    // Which path this is looking at. With no keys of its own the scene is on
    // the default orbit, and that's shown read-only: it isn't authored content,
    // it's derived from the framing, and it re-derives itself as you move. Your
    // first "+ Add key" starts a list of your own, which the ✕s can edit.
    let own: Option<Vec<(f32, f32, f32)>> = app.scene.camera_path.as_ref().map(|p| {
        p.keys
            .iter()
            .map(|k| {
                let c = k.orientation.yaw_pitch_roll();
                (k.distance, c.yaw.radians(), c.pitch.radians())
            })
            .collect()
    });
    let authored = own.is_some();
    let flying_default = app.path_is_default();

    let keys: Vec<(f32, f32, f32)> = own.unwrap_or_else(|| {
        app.camera_path()
            .keys
            .iter()
            .map(|k| {
                let c = k.orientation.yaw_pitch_roll();
                (k.distance, c.yaw.radians(), c.pitch.radians())
            })
            .collect()
    });
    // Pinned below the list, so the list gets the slack from a taller window.
    egui::Panel::bottom("fracturize_camera_path_controls").show(ui, |ui| {
        ui.add_space(2.0);
        draw_path_controls(ui, app, flying_default, authored);
    });

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Camera path").strong());
        let resp = ui.label(
            egui::RichText::new(if authored { "(this scene's own)" } else { "(default orbit)" })
                .small()
                .weak(),
        );
        // What the default orbit is, on hover rather than as two permanent
        // lines of prose. It only ever appeared on the default orbit, which is
        // also the case that shows four keypoints — so it cost the list its
        // room in exactly the situation where the list was longest.
        if !authored {
            resp.on_hover_text(
                "One full turn around the current view. Every scene uses this \
                 until you add keypoints of your own.",
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add_enabled(authored, egui::Button::new((icons::RESET, "Reset")));
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if authored {
                    "Delete this scene's keypoints and go back to the default orbit"
                } else {
                    "Already on the default orbit"
                },
                "click: reset to the default orbit",
            );
            if resp.clicked() {
                app.reset_path_to_default();
            }

            let resp = ui.button("+ Add key");
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if authored {
                    "Add the current view to the end of the path (Y)"
                } else {
                    "Start a path of your own from the current view (Y). The \
                     default orbit is used until there are two keypoints."
                },
                "click: add a keypoint here",
            );
            if resp.clicked() {
                app.add_path_key();
            }
        });
    });

    // Every remaining point of the window goes to the list — see `draw`'s
    // panel layout. `auto_shrink` off on the vertical axis is what makes the
    // scroll area claim that space rather than hugging its rows.
    //
    // The scrollbar is pinned into the layout rather than left floating: a
    // floating bar draws *over* the right edge of the content, which is
    // exactly where each row's ✕ is, so removing a keypoint from a list long
    // enough to scroll meant threading the pointer between the two.
    ui.spacing_mut().scroll.floating = false;
    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_path_keys")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, (d, yaw, pitch)) in keys.iter().enumerate() {
                ui.horizontal(|ui| {
                    num::small_cell(
                        ui,
                        &format!(
                            "{:>3}. d={} yaw={} pitch={}",
                            i + 1,
                            num::cell(*d, 2, 6),
                            num::cell(*yaw, 2, 6),
                            num::cell(*pitch, 2, 6),
                        ),
                        PATH_KEY_CHARS,
                    );
                    if !authored {
                        return;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // A deliberate hit target rather than a glyph's own
                        // ink: this is a destructive button in a scrolling
                        // list, and it should be easy to hit on purpose.
                        let resp = ui.add(
                            egui::Button::new(super::icons::X)
                                .frame(false)
                                .min_size(egui::vec2(22.0, 18.0)),
                        );
                        let resp = hinted(
                            resp,
                            &mut app.ui_state,
                            "Remove this keypoint",
                            "click: remove this keypoint",
                        );
                        if resp.clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
        });
    if let Some(i) = remove {
        app.remove_path_key_at(i);
    }
}

/// The path's transport and shape controls, pinned below the keypoint list.
pub(super) fn draw_path_controls(ui: &mut egui::Ui, app: &mut App, flying_default: bool, authored: bool) {
    // One key of your own isn't a path yet, and the list above doesn't show the
    // orbit that's actually flying — so say which is which.
    if authored && flying_default {
        ui.label(
            egui::RichText::new(
                "Add one more keypoint to fly this path instead of the default orbit.",
            )
            .small()
            .color(ui.visuals().warn_fg_color),
        );
    }

    let moving = app.camera_moving();
    let seconds = app.camera_path().duration();
    let explicit_seconds = app.camera_path().seconds.is_some();
    let zoom_loop = app.path_zoom_loop();
    // A zoom loop closes under the scene's scale symmetry, so it only exists
    // when there is one to close under.
    let zoom = app.zoom().map(|z| (z.map, z.log_scale / std::f32::consts::LN_2));

    ui.horizontal(|ui| {
        let resp = ui.button(if moving { (icons::PAUSE, "Stop") } else { (icons::PLAY, "Play") });
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Fly the camera along the path (O, Z or Space)",
            "click: play / stop the camera",
        );
        if resp.clicked() {
            app.toggle_camera_motion();
        }

        let mut secs = seconds;
        let resp = ui.add(
            num::Drag::new(&mut secs)
                .speed(0.1)
                .range(0.1..=600.0)
                .suffix("s"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if explicit_seconds {
                "How long the whole path takes"
            } else {
                "How long the whole path takes. At the moment that's the \
                 default of 3 seconds between keypoints"
            },
            "drag: set the path duration",
        );
        if resp.changed() {
            app.set_path_seconds(Some(secs));
        }
    });

    draw_loop(ui, app, zoom, zoom_loop);
}

/// How the path gets back to its start: a four-way radio, because it is one
/// choice and a path is always on one of them.
///
/// It used to be two checkboxes — `loop`, and a `zoom loop` row that greyed
/// itself out while the other was on. Two boxes for a four-way choice draws a
/// picture that isn't true: it says the two can be combined (they can't — see
/// `path::Loop`), and it leaves the ordinary case, "play once and stop", with
/// nothing on screen naming it at all. You could only tell you were on it by
/// noticing that neither box was ticked.
///
/// `zoom` is greyed rather than dropped when the scene has no scale symmetry
/// to close under, on the same reasoning the old row was: a mode that isn't
/// drawn can't tell you it exists, and the tooltip is where it says how to get
/// one. The period count beside it follows the same rule.
fn draw_loop(
    ui: &mut egui::Ui,
    app: &mut App,
    zoom: Option<(usize, f32)>,
    current: Option<crate::path::ZoomLoop>,
) {
    use crate::path::LoopKind;

    let zoom_note = match zoom {
        Some((map, octaves)) => format!(
            "Loop by zooming in until the picture repeats itself, instead of \
             flying back to the first keypoint. The animation plays as a zoom \
             that never ends.\n\n\
             The zoom centres on transform {}, and repeats every {:.2} octaves. \
             A single keypoint is enough, and gives a steady zoom with no seam.",
            map, octaves
        ),
        None => "Loop by zooming in until the picture repeats itself, so the \
                 animation plays as a zoom that never ends.\n\n\
                 This needs infinite zoom, which this scene doesn't use yet. Turn \
                 it on under infinite zoom in the Render window."
            .to_string(),
    };

    let kind = app.path_loop().kind();
    ui.horizontal(|ui| {
        ui.label("Loop");
        let chosen = super::radio::radio(&mut app.ui_state, "path_loop", kind)
            .option(
                LoopKind::Once,
                "once",
                "Fly from the first keypoint to the last, then stop. Starts and \
                 stops gently.",
                "click: play the path once and stop",
            )
            .option(
                LoopKind::PingPong,
                "ping-pong",
                "Fly out to the last keypoint and back again, forever. Good for \
                 a path whose ends are far apart. It slows down smoothly at each \
                 end, so the turnarounds don't jolt.",
                "click: loop out and back along the path",
            )
            .option(
                LoopKind::Closed,
                "loop",
                "Fly from the last keypoint back to the first, for a seamless \
                 loop at a steady speed. Choosing this on the default orbit makes \
                 it this scene's own path.",
                "click: loop back round to the first keypoint",
            )
            .option_enabled(
                zoom.is_some(),
                LoopKind::Zoom,
                "zoom",
                zoom_note,
                if zoom.is_some() {
                    "click: loop as an endless zoom"
                } else {
                    // No arrow: the UI font has no U+2192, and a missing glyph
                    // in the one line that tells you where to go is worse than
                    // a comma.
                    "needs infinite zoom: see the Render window"
                },
            )
            .show(ui);
        if let Some(kind) = chosen {
            app.set_path_loop(kind);
        }

        // The zoom loop's one parameter, on the same row as the option it
        // belongs to and greyed off it — a modifier of that segment, not a
        // fifth thing to pick. It also has to be *this* row: the Camera window
        // is already at its vertical budget, and a row of its own pushed the
        // keypoint list into the transport controls below it.
        let Some((_, octaves)) = zoom else { return };
        let mut periods = current.map_or(1, |z| z.periods);
        let suffix = if periods == 1 { " repeat" } else { " repeats" };
        let resp = ui.add_enabled(
            current.is_some(),
            num::Drag::new(&mut periods).speed(0.05).range(1..=64).suffix(suffix),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            format!(
                "How many times the picture repeats in one loop. Each repeat is \
                 {:.2} octaves, so {} of them zoom in by a factor of {:.0}. More \
                 makes a longer fall; the loop is seamless either way.",
                octaves,
                periods,
                2f32.powf(octaves * periods as f32),
            ),
            "drag: repeats per loop",
        );
        if resp.changed() {
            app.set_path_zoom_periods(periods);
        }
    });
}


// The four output buttons — Render job… / Screenshot / Save scene / Save as… —
// used to live here, in a row pinned to the bottom of this window. Three of the
// four had nothing to do with the camera; they were here because Camera was the
// window with a bottom panel free, which is an implementation reason and not a
// task one. They're in the toolbar's File menu now (`ui::toolbar`), where a
// person arriving from any other program will look for them first.
