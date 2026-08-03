//! The gradient strip: the colour source, drawn rather than described.
//!
//! Before this you could not see the colormap you were getting — only the
//! per-transform swatches it was built from, which is a different thing once
//! the ring interpolation has been between them. So the strip is drawn in
//! **both** modes; in `transforms` mode it is read-only, and it is worth
//! having on its own.
//!
//! Two strips, not one, whenever `color_contrast` is doing something. The
//! contrast stretch is a cyclic rescale applied at lookup time in the shader,
//! so a designed palette can render as a small arc of itself with no
//! indication why. The second strip is what the fractal actually indexes
//! into, drawn directly under the palette it came from, with the control that
//! caused it immediately below — see the fifth weakness in PALETTE-PLAN.md §3.
//!
//! Everything here paints **sRGB**: the stored colours are linear (the render
//! surface encodes for itself), so they go through `palette::to_srgb8` on the
//! way to a `Color32` or the strip would read far too dark.

use glam::Vec3;

use crate::app::App;
use crate::palette::{to_srgb8, Body, Colormap, Interpolate};
use crate::scene::ColorMode;

use super::hints::hinted;

/// Height of the main gradient bar.
const STRIP_H: f32 = 26.0;
/// Height of the "what the fractal actually indexes" bar.
const STRETCHED_H: f32 = 10.0;
/// Height of the control-point handle row.
const HANDLE_H: f32 = 12.0;

fn color32(c: Vec3) -> egui::Color32 {
    let [r, g, b] = to_srgb8(c);
    egui::Color32::from_rgb(r, g, b)
}

fn entry(map: &Colormap, i: usize) -> egui::Color32 {
    let e = map[i & 0xFF];
    color32(Vec3::new(e[0], e[1], e[2]))
}

/// Where the shader's cyclic contrast stretch sends colour index `t`.
/// Mirrors `points/render.wgsl` and `points/splat.wgsl` — keep in sync.
fn stretched_index(t: f32, contrast: f32) -> f32 {
    (0.5 + (t - 0.5) * contrast).rem_euclid(1.0)
}

/// Paint a colormap across `rect` as a smooth bar.
///
/// A mesh rather than 256 rectangles: one shape, and the GPU interpolates
/// between entries so the strip doesn't stair-step at panel widths under
/// 256px (which is most of them).
fn paint(painter: &egui::Painter, rect: egui::Rect, mut sample: impl FnMut(f32) -> egui::Color32) {
    const N: usize = 96;
    let mut mesh = egui::Mesh::default();
    for i in 0..=N {
        let t = i as f32 / N as f32;
        let x = rect.left() + rect.width() * t;
        let c = sample(t);
        mesh.colored_vertex(egui::pos2(x, rect.top()), c);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), c);
        if i > 0 {
            let b = (i as u32 - 1) * 2;
            mesh.add_triangle(b, b + 1, b + 2);
            mesh.add_triangle(b + 1, b + 2, b + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A bordered gradient bar filling the available width.
fn bar(ui: &mut egui::Ui, height: f32, map: &Colormap, transform: impl Fn(f32) -> f32) -> egui::Response {
    let width = ui.available_width().max(64.0);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, |t| entry(map, (transform(t) * 256.0) as usize));
        ui.painter().rect_stroke(
            rect,
            2.0,
            ui.visuals().widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    resp
}

/// The Render window's whole colour block: mode, source, gradient, handles.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    draw_mode_row(ui, app);

    // Mix mode has no colormap to draw: colour is a 3-vector carried through
    // the walk and written straight into the point, so there is no 1-D strip
    // it could be indexing into. Its "palette" is the set of transform
    // colours, so show those.
    if app.scene.color_mode == ColorMode::Mix {
        draw_mix(ui, app);
        return;
    }

    let map = app.scene.colormap;
    let resp = bar(ui, STRIP_H, &map, |t| t);
    let strip_rect = resp.rect;

    // Where each transform's colour lands in the gradient. This is the one
    // piece of UI that makes the Apophysis model click: `color_value` stops
    // being an abstract 0-1 and becomes a place you can point at.
    draw_transform_marks(ui, app, strip_rect);

    let resp = hinted(
        resp,
        &mut app.ui_state,
        if app.scene.color_mode == ColorMode::Palette {
            "The gradient this scene renders through. Double-click to add a control point."
        } else {
            "The colormap built from the per-transform colours. Read-only here — edit the \
             colours in the Transforms window, or switch to palette mode."
        },
        if app.scene.color_mode == ColorMode::Palette {
            "double-click: add a control point"
        } else {
            "the colormap the transform colours produce"
        },
    );

    if app.scene.color_mode == ColorMode::Palette && resp.double_clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - strip_rect.left()) / strip_rect.width()).clamp(0.0, 0.999);
            if app.scene.palette.as_ref().and_then(|p| p.stops()).is_some() {
                let added = app.add_palette_stop(t);
                app.ui_state.palette_stop = added;
            }
        }
    }

    // Handles go directly under the strip — a control point has to sit under
    // the colour it controls, or it isn't a handle on anything.
    if app.scene.color_mode == ColorMode::Palette {
        draw_handles(ui, app, strip_rect);
    }

    draw_contrast(ui, app, &map);

    if app.scene.color_mode == ColorMode::Palette {
        draw_palette_controls(ui, app);
    }
}

/// `transforms | palette`, the library dropdown, and the dice.
fn draw_mode_row(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Color");
        let mode = app.scene.color_mode;

        let resp = ui.selectable_label(mode == ColorMode::Transforms, "transforms");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Colour from the per-transform RGBs, spread evenly around the gradient. \
             Keeps each transform's identity — the mode for reading IFS structure. \
             Note the spacing is 1/N, so adding a transform recolours them all.",
            "click: colour from the transform colours",
        );
        if resp.clicked() {
            app.set_color_mode(ColorMode::Transforms);
        }

        let resp = ui.selectable_label(mode == ColorMode::Palette, "palette");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Colour through an independent gradient, Apophysis-style. Doesn't depend \
             on the transform count, and the gradient is a portable asset you can \
             restyle a finished flame with.",
            "click: colour through a palette",
        );
        if resp.clicked() {
            app.set_color_mode(ColorMode::Palette);
        }

        let resp = ui.selectable_label(mode == ColorMode::Mix, "mix");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Carry the per-transform colours through the walk as a 3-vector instead of a \
             gradient position, so they genuinely blend: a walker that came via a red map and \
             then a blue one is purple, and tells apart from one that came via two magenta \
             maps. Distinct transform *combinations* get distinct colours — the thing a 1-D \
             index cannot do. No colormap, so color contrast doesn't apply.",
            "click: mix the transform colours through the walk",
        );
        if resp.clicked() {
            app.set_color_mode(ColorMode::Mix);
        }

        if mode == ColorMode::Palette {
            let current = app
                .scene
                .palette
                .as_ref()
                .and_then(|p| p.name.clone())
                .unwrap_or_else(|| "custom".to_string());
            egui::ComboBox::from_id_salt("palette_library")
                .selected_text(current)
                .width(110.0)
                .show_ui(ui, |ui| {
                    for name in crate::palette::library::names() {
                        let selected =
                            app.scene.palette.as_ref().and_then(|p| p.name.as_deref()) == Some(name);
                        let resp = ui.selectable_label(selected, name);
                        if let Some(blurb) = crate::palette::library::blurb(name) {
                            resp.clone().on_hover_text(blurb);
                        }
                        if resp.clicked() {
                            if let Some(p) = crate::palette::library::get(name) {
                                app.set_palette(p, "Palette from library");
                            }
                        }
                    }
                });

            let resp = ui.button("roll");
            let resp = hinted(
                resp,
                &mut app.ui_state,
                "Roll a random gradient — a cosine palette, a colour-harmony scheme, or one \
                 from the library. Every roll sweeps luminance and has no seam at index 0, \
                 because the renderer has no lights and the palette is the shading.",
                "click: roll a random palette",
            );
            if resp.clicked() {
                let described = app.randomize_palette(None);
                log::info!("Random palette: {}", described);
            }
        }
    });
}

/// Mix mode's stand-in for the strip: the transform colours themselves, which
/// is what the walk is actually blending. Clicking one selects that transform,
/// so the swatch row doubles as a way into the colour picker that edits it.
fn draw_mix(ui: &mut egui::Ui, app: &mut App) {
    let colors: Vec<Vec3> = app.scene.colors.clone();
    let selected = app.selection();
    let mut clicked = None;

    ui.horizontal_wrapped(|ui| {
        for (i, &c) in colors.iter().enumerate() {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(STRIP_H, STRIP_H), egui::Sense::click());
            if ui.is_rect_visible(rect) {
                let stroke = if selected == Some(i) {
                    egui::Stroke::new(2.0, ui.visuals().widgets.active.fg_stroke.color)
                } else {
                    egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color)
                };
                ui.painter().rect(rect, 2.0, color32(c), stroke, egui::StrokeKind::Inside);
            }
            let name = app
                .scene
                .transform_names
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_else(|| format!("T{i}"));
            if resp.on_hover_text(name).clicked() {
                clicked = Some(i);
            }
        }
    });
    if let Some(i) = clicked {
        app.select_transform(Some(i));
    }

    ui.label(
        egui::RichText::new(
            "Colour is mixed through the walk, not looked up: no colormap, and \
             color contrast does not apply. Edit the colours in the Transforms window.",
        )
        .weak()
        .small(),
    );
}

/// Small ticks along the strip showing each transform's `color_value`.
fn draw_transform_marks(ui: &mut egui::Ui, app: &App, rect: egui::Rect) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let selected = app.selection();
    for (i, t) in app.scene.transforms.iter().enumerate() {
        let x = rect.left() + rect.width() * t.color_value.clamp(0.0, 1.0);
        let is_sel = selected == Some(i);
        // Two strokes, dark under light: a single-colour tick disappears into
        // whichever end of the gradient it happens to land on.
        let h = if is_sel { rect.height() } else { rect.height() * 0.45 };
        let top = egui::pos2(x, rect.bottom() - h);
        let bottom = egui::pos2(x, rect.bottom());
        painter.line_segment(
            [top, bottom],
            egui::Stroke::new(if is_sel { 3.5 } else { 2.5 }, egui::Color32::from_black_alpha(160)),
        );
        painter.line_segment(
            [top, bottom],
            egui::Stroke::new(if is_sel { 1.5 } else { 1.0 }, egui::Color32::WHITE),
        );
    }
}

/// The stretched strip plus the control that causes it.
fn draw_contrast(ui: &mut egui::Ui, app: &mut App, map: &Colormap) {
    let contrast = app.color_contrast;
    if (contrast - 1.0).abs() > 0.01 {
        let resp = bar(ui, STRETCHED_H, map, move |t| stretched_index(t, contrast));
        hinted(
            resp,
            &mut app.ui_state,
            "What the fractal actually indexes into: the gradient above after the cyclic \
             contrast stretch. If your palette looks like only part of itself, this is why.",
            "the colormap after the contrast stretch",
        );
    }

    let mut contrast = app.color_contrast;
    let resp = ui.add(
        egui::Slider::new(&mut contrast, 0.25..=16.0)
            .logarithmic(true)
            .text("color contrast"),
    );
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Cyclic stretch of the colour index around its centre. Above 1 it spreads a \
         compressed range back across the gradient — which is what you want after a low \
         color falloff, and what makes a designed palette look like only an arc of itself \
         if you leave it high (C / Shift+C)",
        "drag: adjust color contrast",
    );
    if resp.changed() {
        app.set_color_contrast(contrast);
    }
}

/// Draggable control points under the strip, and the editor for the selected
/// one.
fn draw_handles(ui: &mut egui::Ui, app: &mut App, strip: egui::Rect) {
    let Some(palette) = app.scene.palette.as_ref() else { return };

    // Procedural and imported palettes have no control points to drag. Rather
    // than show a dead row, offer the conversion — which keeps the colours and
    // makes them editable.
    let stops: Vec<crate::palette::Stop> = match palette.stops() {
        Some(s) => s.to_vec(),
        None => {
            let kind = match &palette.body {
                Body::Cosine(_) => "a cosine formula",
                _ => "256 imported entries",
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{kind} — no handles")).weak());
                let resp = ui.button("convert to stops");
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Freeze this gradient into draggable control points, keeping the colours \
                     it has now. The formula is replaced by the samples.",
                    "click: make this palette editable",
                );
                if resp.clicked() {
                    app.convert_palette_to_stops(12);
                }
            });
            return;
        }
    };

    let (row, _) = ui.allocate_exact_size(
        egui::vec2(strip.width(), HANDLE_H),
        egui::Sense::hover(),
    );
    // The handle row is allocated in the panel's layout but drawn against the
    // strip's x-extent, so a handle sits under the colour it controls.
    let row = egui::Rect::from_min_max(
        egui::pos2(strip.left(), row.top()),
        egui::pos2(strip.right(), row.bottom()),
    );

    let mut selected = app.ui_state.palette_stop.filter(|&i| i < stops.len());
    let mut dragged_to: Option<f32> = None;
    let mut deleted: Option<usize> = None;

    for (i, stop) in stops.iter().enumerate() {
        let x = row.left() + row.width() * stop.at.clamp(0.0, 1.0);
        let handle = egui::Rect::from_center_size(
            egui::pos2(x, row.center().y),
            egui::vec2(HANDLE_H, HANDLE_H),
        );
        let resp = ui.interact(
            handle,
            ui.id().with(("palette_stop", i)),
            egui::Sense::click_and_drag(),
        );

        let is_sel = selected == Some(i);
        let outline = if is_sel || resp.hovered() {
            ui.visuals().widgets.active.fg_stroke.color
        } else {
            ui.visuals().widgets.inactive.bg_stroke.color
        };
        ui.painter()
            .circle(handle.center(), HANDLE_H * 0.42, color32(stop.color), egui::Stroke::new(1.5, outline));

        if resp.clicked() {
            selected = Some(i);
        }
        // The press decides *what* is being dragged; from then on the
        // response only reports that a drag is still happening, because the
        // index under it may no longer be this stop (see `palette_drag`).
        if resp.drag_started() {
            app.ui_state.palette_drag = Some(i);
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                dragged_to = Some(pos.x);
            }
        }
        if resp.drag_stopped() {
            app.ui_state.palette_drag = None;
        }
        resp.context_menu(|ui| {
            if ui.button("Delete stop").clicked() {
                deleted = Some(i);
                ui.close();
            }
        });
    }

    if let (Some(x), Some(active)) = (dragged_to, app.ui_state.palette_drag) {
        let at = (x - row.left()) / row.width();
        let landed = app.set_palette_stop_at(active, at);
        app.ui_state.palette_drag = Some(landed);
        selected = Some(landed);
    }
    if let Some(i) = deleted {
        app.remove_palette_stop(i);
        selected = None;
    }
    app.ui_state.palette_stop = selected;

    draw_selected_stop(ui, app, selected);
}

/// Colour, position and delete for the selected control point.
fn draw_selected_stop(ui: &mut egui::Ui, app: &mut App, selected: Option<usize>) {
    let Some(idx) = selected else {
        ui.label(
            egui::RichText::new("click a handle to edit a stop · double-click the strip to add")
                .weak()
                .small(),
        );
        return;
    };
    let Some(stop) = app.scene.palette.as_ref().and_then(|p| p.stops()).and_then(|s| s.get(idx))
    else {
        return;
    };
    let (mut at, color) = (stop.at, stop.color);
    let mut srgb = to_srgb8(color);
    let count = app.scene.palette.as_ref().and_then(|p| p.stops()).map_or(0, |s| s.len());

    ui.horizontal(|ui| {
        ui.label(format!("stop {}", idx));
        let resp = ui.color_edit_button_srgb(&mut srgb);
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "This control point's colour. Picked in sRGB — the same value a colour picker \
             anywhere else shows — and stored linear for the renderer.",
            "click: pick this stop's colour",
        );
        if resp.changed() {
            app.set_palette_stop_color(idx, crate::palette::from_srgb8(srgb));
        }

        let resp = ui.add(egui::DragValue::new(&mut at).speed(0.002).range(0.0..=0.999));
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Where this stop sits in the gradient (0-1)",
            "drag: move this stop · click: type an exact position",
        );
        if resp.changed() {
            app.ui_state.palette_stop = Some(app.set_palette_stop_at(idx, at));
        }

        // Two stops is the floor: one is a flat colour, none is white.
        ui.add_enabled_ui(count > 2, |ui| {
            let resp = ui.button("delete");
            let resp = hinted(
                resp,
                &mut app.ui_state,
                if count > 2 {
                    "Remove this control point"
                } else {
                    "A gradient needs at least two control points"
                },
                "click: delete this stop",
            );
            if resp.clicked() {
                app.remove_palette_stop(idx);
                app.ui_state.palette_stop = None;
            }
        });
    });
}

/// Rotate, reverse, interpolation space — the tweaks that let a library
/// palette be adjusted without forking it.
fn draw_palette_controls(ui: &mut egui::Ui, app: &mut App) {
    let Some(p) = app.scene.palette.as_ref() else { return };
    let (mut rotate, mut reverse, mut interpolate, mut cyclic) =
        (p.rotate, p.reverse, p.interpolate, p.cyclic);

    let resp = ui.add(egui::Slider::new(&mut rotate, 0.0..=1.0).text("rotate"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Shift the whole gradient along the colour index. Which transform gets which \
         colour is an arbitrary choice, so this is the cheapest way to try another one.",
        "drag: rotate the gradient",
    );
    if resp.changed() {
        app.edit_palette("Rotate palette", Some("pal:rotate"), |p| {
            p.rotate = rotate.rem_euclid(1.0)
        });
    }

    ui.horizontal(|ui| {
        let resp = ui.checkbox(&mut reverse, "reverse");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Run the gradient backwards. Applied before rotate.",
            "click: reverse the gradient",
        );
        if resp.changed() {
            app.edit_palette("Reverse palette", None, |p| p.reverse = reverse);
        }

        let resp = ui.checkbox(&mut cyclic, "cyclic");
        let resp = hinted(
            resp,
            &mut app.ui_state,
            "Whether index 255 wraps back to 0. The renderer's lookup wraps and the contrast \
             stretch assumes it, so cyclic is the norm; imported flam3 palettes are authored \
             for a clamped 0-255 range and may have a seam.",
            "click: toggle cyclic wrap",
        );
        if resp.changed() {
            app.edit_palette("Palette wrap", None, |p| p.cyclic = cyclic);
        }

        ui.label("blend");
        for space in Interpolate::ALL {
            let resp = ui.selectable_label(interpolate == space, space.name());
            let resp = hinted(
                resp,
                &mut app.ui_state,
                match space {
                    Interpolate::Rgb => "Blend control points in linear RGB — what flam3 and \
                                         the transform ring do. Complementary stops pass \
                                         through grey.",
                    Interpolate::Oklab => "Blend in Oklab: perceptually even, and no muddy \
                                           midpoint between opposite hues.",
                },
                "click: change the blend space",
            );
            if resp.clicked() && interpolate != space {
                interpolate = space;
                app.edit_palette("Palette blend space", None, |p| p.interpolate = space);
            }
        }
    });
}

/// The Transforms window's version: the strip with *this* transform's
/// `color_value` marked and draggable on it.
///
/// In palette mode a per-transform RGB swatch is misleading — nothing renders
/// it — and `color_value` is the control that matters. Drawing it on the
/// gradient is the Apophysis idiom: you set where a transform lands in the
/// palette by pointing at the colour you want.
pub fn transform_color_value(ui: &mut egui::Ui, app: &mut App, idx: usize) {
    let map = app.scene.colormap;
    let mut value = app.scene.transforms[idx].color_value;

    let resp = bar(ui, STRIP_H, &map, |t| t);
    let rect = resp.rect;

    if ui.is_rect_visible(rect) {
        let x = rect.left() + rect.width() * value.clamp(0.0, 1.0);
        let painter = ui.painter();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(4.0, egui::Color32::from_black_alpha(180)),
        );
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
    }

    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Where this transform sits in the palette. The walker's colour index eases toward \
         this value each time the transform is applied, at a rate set by color speed / \
         falloff — so it is a target, not the colour the transform paints.",
        "drag: move this transform along the gradient",
    );
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            value = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            app.set_transform_color_value(idx, value);
        }
    }

    let mut cv = app.scene.transforms[idx].color_value;
    let resp = ui.add(egui::Slider::new(&mut cv, 0.0..=1.0).text("color value"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "This transform's position in the gradient (0-1 around the cyclic colormap)",
        "drag: adjust colormap index",
    );
    if resp.changed() {
        app.set_transform_color_value(idx, cv);
    }
}
