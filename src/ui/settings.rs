//! Settings: the person's own preferences. Each follows the person from scene
//! to scene and is never saved in one, so nothing here needs a heading saying
//! where it lives.
//!
//! These used to be split between the top of the Controls window and the
//! Camera window. Controls is a reference now (the keys and the mouse), and
//! the Camera window is gone.
//!
//! Machine settings that belong to one dialog (the Render job's GPU share and
//! threads, the agent backend) stay in that dialog, where they're read.

use crate::app::App;
use crate::camera::{Fov, OrbitStyle};

use super::hints::hinted;
use super::num;

/// Budget for the horizontal-FoV readout beside the FoV slider: `150°h`.
const FOV_READOUT_CHARS: usize = 5;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.settings_open;
    if !open {
        return;
    }
    super::window(ctx, app, super::WindowKey::Settings, "Settings")
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("The mouse").strong());
            draw_input_prefs(ui, app);
            ui.add_space(6.0);
            ui.label(egui::RichText::new("The panels").strong());
            draw_panel_scale(ui, app);
            draw_selection_colour(ui, app);
        });
    super::remember(ctx, app, super::WindowKey::Settings);
    app.ui_state.panels.settings_open = open;
}

/// Orbit style, the lens and invert pitch. None of them moves the camera:
/// they change how the next drag is read, or how wide the lens is for every
/// scene that doesn't choose its own.
fn draw_input_prefs(ui: &mut egui::Ui, app: &mut App) {
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
                 level button beside the axis cross puts it back.",
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

/// How big the panels and text are.
fn draw_panel_scale(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        // Applied on *release*, not while dragging.
        //
        // This is a slider that sets how big sliders are, which makes
        // live application a feedback loop: every frame it re-scales
        // the widget you are holding, so the handle moves out from
        // under the pointer, which moves the handle again. It flickers,
        // it jumps, and it is close to unusable. Dragging shows the
        // pending number and the panel only re-lays-out once you let
        // go — the one control in the app where live application is
        // actively wrong.
        let mut scale = app.ui_state.pending_ui_scale.unwrap_or(app.ui_scale());
        let resp = ui.add(
            egui::Slider::new(&mut scale, 0.7..=2.0)
                .fixed_decimals(2)
                .text("panel scale"),
        );
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "How big the panels and text are. Takes effect when you let \
             go. The picture itself isn't affected.",
            "drag: resize the panels (applies on release)",
        );
        if resp.dragged() {
            app.ui_state.pending_ui_scale = Some(scale);
        } else if resp.changed() {
            // A click on the track, or an arrow key: no drag to wait for.
            app.ui_state.pending_ui_scale = None;
            app.set_ui_scale(scale);
        }
        if resp.drag_stopped() {
            app.ui_state.pending_ui_scale = None;
            app.set_ui_scale(scale);
        }
    });
}

/// The highlight colour: one swatch, live, and a way back to the default.
/// A preference, so not undoable; Default is the undo.
fn draw_selection_colour(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let current = app.selection_colour();
        let [r, g, b] = current.unwrap_or({
            let c = super::theme::DEFAULT_SELECTION;
            [c.r(), c.g(), c.b()]
        });
        let mut srgb = [r, g, b];
        let resp = ui.color_edit_button_srgb(&mut srgb);
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "The colour that marks what's selected: the chosen option, a window that's \
             open, the selected transform or track, a render's progress. Click to pick another.",
            "click: pick the highlight colour",
        );
        if resp.changed() {
            app.set_selection_colour(Some(srgb));
        }
        ui.label("highlight colour");
        let resp = ui.add_enabled(current.is_some(), egui::Button::new("Default"));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            if current.is_some() {
                "Go back to the standard blue"
            } else {
                "This is the standard blue already"
            },
            "click: use the standard highlight colour",
        );
        if resp.clicked() {
            app.set_selection_colour(None);
        }
    });
}
