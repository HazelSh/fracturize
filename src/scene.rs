use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::path::{CameraPath, PathKey};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// 256-color gradient for Apophysis-style rendering
pub type Colormap = [[f32; 4]; 256];

/// Number of variation slots per transform (must match chaos.wgsl)
pub const NUM_VARIATIONS: usize = 20;

/// Variation names, in GPU slot order (must match apply_variations in chaos.wgsl)
pub const VARIATION_NAMES: [&str; NUM_VARIATIONS] = [
    "linear",      // 0: identity
    "sinusoidal",  // 1: sin() per component
    "spherical",   // 2: p / r^2 (inversion)
    "swirl",       // 3: rotate xy by r^2
    "horseshoe",   // 4: complex square-ish fold
    "polar",       // 5: (theta/pi, r-1)
    "disc",        // 6: (theta/pi)*(sin(pi r), cos(pi r))
    "spiral",      // 7: (cos+sin r, sin-cos r)/r
    "hyperbolic",  // 8: (sin(theta)/r, r cos(theta))
    "diamond",     // 9: (sin t cos r, cos t sin r)
    "julia",       // 10: sqrt-r half-angle with random branch
    "bent",        // 11: piecewise fold of negative x/y
    "fisheye",     // 12: 2p/(r+1) (eyefish)
    "bubble",      // 13: 4p/(r^2+4)
    "cylinder",    // 14: (sin x, y, z)
    "tangent",     // 15: (sin x / cos y, tan y, z)
    "absfold",     // 16: abs(p) — KIFS kaleidoscope fold (pair with rotations)
    "boxfold",     // 17: 2*clamp(p,±1)-p — Mandelbox box fold
    "spherefold",  // 18: Mandelbox sphere fold (minR2 0.25, fixR2 1)
    "bulb",        // 19: power-8 mandelbulb angle map, radius-preserving
];

/// A single IFS transform, fully resolved for use by the app and GPU
#[derive(Clone)]
pub struct TransformSpec {
    /// Affine part (applied before variations)
    pub matrix: Mat4,
    /// Colormap index (0.0-1.0)
    pub color_value: f32,
    /// Selection weight
    pub weight: f32,
    /// Effective color blending speed (0.0-1.0), resolved by resolve_color_speeds
    pub color_speed: f32,
    /// Explicit per-transform color_speed from the scene file, if any.
    /// Always wins over global color_speed and color_falloff.
    pub explicit_color_speed: Option<f32>,
    /// Variation blend weights, by slot (see VARIATION_NAMES)
    pub variations: [f32; NUM_VARIATIONS],
}

/// Resolve each transform's effective color_speed.
///
/// With color_falloff = 0, transforms use their explicit color_speed or the
/// global one (classic fixed-rate EMA). With color_falloff > 0, the EMA
/// retain-factor per step is tied to the transform's spatial contraction:
///
///     retained = contraction^falloff    (speed = 1 - retained)
///
/// so the color weight of the transform applied k steps ago equals the
/// spatial scale that step controls, raised to `falloff`. Color variation
/// amplitude then follows a pure power law of feature scale — detail at
/// every scale with no resonant size. Lower falloff = flatter spectrum
/// (more fine detail, but colors compress toward the mean; compensate with
/// color_contrast at render time).
pub fn resolve_color_speeds(transforms: &mut [TransformSpec], global_speed: f32, falloff: f32) {
    for t in transforms {
        t.color_speed = match t.explicit_color_speed {
            Some(s) => s,
            None if falloff > 0.0 => 1.0 - t.contraction().powf(falloff),
            None => global_speed,
        };
    }
}

impl TransformSpec {
    /// Spatial contraction factor of the affine part (cube root of the
    /// determinant), clamped away from 0 and 1 so falloff-derived speeds
    /// stay sane for degenerate or expanding transforms.
    pub fn contraction(&self) -> f32 {
        self.matrix.determinant().abs().powf(1.0 / 3.0).clamp(0.05, 0.95)
    }

    /// Weights for a pure-linear (classic affine) transform
    pub fn linear_variations() -> [f32; NUM_VARIATIONS] {
        let mut v = [0.0; NUM_VARIATIONS];
        v[0] = 1.0;
        v
    }

    /// Short summary of variation weights, e.g. "spherical 0.70 + linear 0.30"
    pub fn variation_summary(&self) -> String {
        let mut parts: Vec<(usize, f32)> = self
            .variations
            .iter()
            .enumerate()
            .filter(|&(_, &w)| w != 0.0)
            .map(|(i, &w)| (i, w))
            .collect();
        if parts.is_empty() {
            return "none".to_string();
        }
        if parts.len() == 1 && parts[0].0 == 0 {
            return "linear".to_string();
        }
        parts.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap());
        parts
            .iter()
            .map(|(i, w)| format!("{} {:.2}", VARIATION_NAMES[*i], w))
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// Parse a TOML `variations` table into slot weights
fn parse_variations(table: &BTreeMap<String, f64>) -> Result<[f32; NUM_VARIATIONS], String> {
    let mut weights = [0.0f32; NUM_VARIATIONS];
    for (name, &weight) in table {
        let slot = VARIATION_NAMES
            .iter()
            .position(|&n| n == name)
            .ok_or_else(|| {
                format!(
                    "Unknown variation '{}'. Available: {}",
                    name,
                    VARIATION_NAMES.join(", ")
                )
            })?;
        weights[slot] = weight as f32;
    }
    Ok(weights)
}

/// Scene metadata from TOML
#[derive(Deserialize, Serialize)]
pub struct SceneMeta {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default = "default_point_size")]
    pub point_size: f64,
    /// Points generated per frame by the chaos game (legacy, unused by point renderer)
    #[serde(alias = "iters", default)] // backwards compat
    pub points_per_frame: usize,
    /// Temporal decay factor (0.0-1.0). Lower = sharper, higher = more accumulation
    #[serde(default = "default_decay")]
    pub decay: f64,
    #[serde(default = "default_color_speed")]
    pub color_speed: f64,
    /// Scale-aware color accumulation exponent (see resolve_color_speeds).
    /// 0 = classic fixed-rate EMA using color_speed. > 0 ties each EMA step's
    /// retain-factor to the transform's contraction^falloff, giving color
    /// detail at every spatial scale (power-law, no resonant size).
    /// ~1.0 is neutral; lower = more fine detail (raise color_contrast too).
    #[serde(default)]
    pub color_falloff: f64,
    /// Render-time contrast stretch of the colormap index around its center,
    /// wrapping cyclically. Compensates the wash-out from low color_falloff.
    #[serde(default = "default_color_contrast")]
    pub color_contrast: f64,
    /// Total point buffer size for the simple point renderer.
    /// If unset, defaults to 500k.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point_count: Option<usize>,
}

fn default_point_size() -> f64 {
    0.012
}

fn default_color_speed() -> f64 {
    0.5
}

fn default_color_contrast() -> f64 {
    1.0
}

fn default_decay() -> f64 {
    0.8 // ~10 frame persistence
}

/// Transform definition in TOML (human-readable format)
#[derive(Deserialize, Serialize)]
pub struct TransformDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub translation: [f64; 3],
    #[serde(default = "default_scale")]
    pub scale: ScaleDef,
    #[serde(default)]
    pub rotation: [f64; 3], // Euler angles in degrees (pitch, yaw, roll)
    pub color: [f64; 3],
    #[serde(default = "default_weight")]
    pub weight: f64,
    /// Color value for Apophysis-style colormap indexing (0.0-1.0)
    /// If not specified, auto-assigned based on transform index
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_value: Option<f64>,
    /// Per-transform color blending speed (0.0-1.0)
    /// Overrides global color_speed and color_falloff if set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_speed: Option<f64>,
    /// Variation blend weights by name, e.g. { spherical = 0.7, linear = 0.3 }
    /// Defaults to { linear = 1.0 } (classic affine IFS)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variations: Option<BTreeMap<String, f64>>,
}

fn default_scale() -> ScaleDef {
    ScaleDef::Uniform(1.0)
}

/// Uniform (`scale = 0.5`) or per-axis (`scale = [0.05, 0.6, 0.05]`) scale.
/// Per-axis scale is what makes L-system-style maps (squash onto a trunk
/// segment, long thin branches) expressible in the TOML format.
#[derive(Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(untagged)]
pub enum ScaleDef {
    Uniform(f64),
    PerAxis([f64; 3]),
}

impl ScaleDef {
    pub fn to_vec3(self) -> Vec3 {
        match self {
            ScaleDef::Uniform(s) => Vec3::splat(s as f32),
            ScaleDef::PerAxis(a) => Vec3::from(a.map(|v| v as f32)),
        }
    }
}

fn default_weight() -> f64 {
    1.0
}

/// Camera configuration from TOML
#[derive(Deserialize, Serialize, Default)]
pub struct CameraDef {
    /// Orbit center / look-at point
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<[f64; 3]>,
    /// Legacy: eye displacement off the orbit sphere. Folded into
    /// yaw/pitch/distance at load; never written back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f64; 3]>,
    /// Orbit radius
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    /// Orbit angle around Y in radians
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    /// Orbit elevation in radians (positive = above the focus)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    /// Camera path: loop back to the first keypoint (seamless loops)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_closed: Option<bool>,
    /// Camera path: playback/render duration in seconds (default 3s/segment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_seconds: Option<f64>,
    /// Camera path: ease in/out (default: open paths ease, closed don't)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_ease: Option<bool>,
    /// Camera path spline keypoints ([[camera.path]]). Omitted fields
    /// default to the base camera's values. Must be last: TOML requires
    /// scalar keys before sub-tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<PathKeyDef>>,
}

/// One [[camera.path]] spline keypoint. Every field is optional and falls
/// back to the base [camera] framing, so a key only states what changes.
#[derive(Deserialize, Serialize, Clone, Copy, Default)]
pub struct PathKeyDef {
    /// Orbit angle in radians. Unbounded: successive keys spanning more than
    /// a full turn author spirals; nothing is wrapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<[f64; 3]>,
}

/// Full scene file structure
#[derive(Deserialize, Serialize)]
pub struct SceneFile {
    pub meta: SceneMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraDef>,
    #[serde(rename = "transform")]
    pub transforms: Vec<TransformDef>,
}

/// Default point buffer size for the simple point renderer
const DEFAULT_POINT_COUNT: usize = 500_000;

/// Loaded scene ready for use
#[derive(Clone)]
pub struct Scene {
    pub name: String,
    pub author: String,
    pub point_size: f32,
    /// Points generated per frame by the density renderer's chaos game
    #[allow(dead_code)]
    pub points_per_frame: usize,
    /// Total point buffer size for the simple point renderer
    pub point_count: usize,
    /// Temporal decay factor (0.0-1.0)
    #[allow(dead_code)]
    pub decay: f32,
    pub color_speed: f32,
    /// Scale-aware color accumulation exponent (0 = classic fixed-rate EMA)
    pub color_falloff: f32,
    /// Render-time cyclic contrast stretch of the colormap index
    pub color_contrast: f32,
    /// IFS transforms (affine matrix + variation blend weights)
    pub transforms: Vec<TransformSpec>,
    /// Human-readable name per transform (from scene file)
    pub transform_names: Vec<Option<String>>,
    /// Per-transform gradient color (source data for the colormap)
    pub colors: Vec<Vec3>,
    /// 256-color gradient for point coloring
    pub colormap: Colormap,
    /// Camera orbit center / look-at point
    pub camera_focus: Vec3,
    /// Camera orbit radius
    pub camera_distance: f32,
    /// Camera orbit angle around Y (radians)
    pub camera_yaw: f32,
    /// Camera orbit elevation (radians)
    pub camera_pitch: f32,
    /// Optional spline camera path ([[camera.path]] keypoints)
    pub camera_path: Option<CameraPath>,
}

impl Scene {
    /// Load a scene from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read scene file: {}", e))?;

        let scene_file: SceneFile = toml::from_str(&content)
            .map_err(|e| format!("Failed to parse scene file: {}", e))?;

        let num_transforms = scene_file.transforms.len();

        // Collect transform colors for colormap generation
        let transform_colors: Vec<Vec3> = scene_file
            .transforms
            .iter()
            .map(|t| Vec3::from(t.color.map(|v| v as f32)))
            .collect();

        let global_speed = scene_file.meta.color_speed as f32;

        let transform_names: Vec<Option<String>> = scene_file
            .transforms
            .iter()
            .map(|t| t.name.clone())
            .collect();

        let transforms: Vec<TransformSpec> = scene_file
            .transforms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                // Convert euler angles (degrees) to quaternion
                let rotation = Quat::from_euler(
                    glam::EulerRot::XYZ,
                    (t.rotation[0] as f32).to_radians(),
                    (t.rotation[1] as f32).to_radians(),
                    (t.rotation[2] as f32).to_radians(),
                );

                let matrix = Mat4::from_scale_rotation_translation(
                    t.scale.to_vec3(),
                    rotation,
                    Vec3::from(t.translation.map(|v| v as f32)),
                );

                // Color value: use explicit if provided, otherwise distribute evenly
                let color_value = t.color_value.map(|v| v as f32).unwrap_or_else(|| {
                    if num_transforms == 1 {
                        0.5
                    } else {
                        i as f32 / (num_transforms - 1) as f32
                    }
                });

                // Placeholder; resolve_color_speeds computes the effective value
                let speed = t.color_speed.map(|v| v as f32).unwrap_or(global_speed);

                let variations = match &t.variations {
                    Some(table) => parse_variations(table)
                        .map_err(|e| format!("Transform {}: {}", i, e))?,
                    None => TransformSpec::linear_variations(),
                };

                Ok(TransformSpec {
                    matrix,
                    color_value,
                    weight: t.weight as f32,
                    color_speed: speed,
                    explicit_color_speed: t.color_speed.map(|v| v as f32),
                    variations,
                })
            })
            .collect::<Result<_, String>>()?;

        let mut transforms = transforms;
        resolve_color_speeds(&mut transforms, global_speed, scene_file.meta.color_falloff as f32);

        // Generate colormap from transform colors (always cyclic)
        let colormap = generate_colormap(&transform_colors);

        let cam = scene_file.camera.unwrap_or_default();
        let camera_focus = cam.focus.map(|f| Vec3::from(f.map(|v| v as f32))).unwrap_or(Vec3::ZERO);
        // Fully-legacy camera blocks (no yaw/pitch) default to the historical
        // slightly-elevated eye; files that specify yaw/pitch get no offset
        let default_offset = if cam.yaw.is_none() && cam.pitch.is_none() {
            Vec3::new(0.0, 1.0, 0.0)
        } else {
            Vec3::ZERO
        };
        let camera_offset = cam.offset.map(|f| Vec3::from(f.map(|v| v as f32))).unwrap_or(default_offset);
        let camera_distance = cam.distance.unwrap_or(3.0) as f32;
        // Fold the legacy eye offset into on-sphere yaw/pitch/distance
        let folded = crate::camera::OrbitCamera::from_legacy(
            camera_focus,
            camera_offset,
            camera_distance,
            cam.yaw.unwrap_or(0.0) as f32,
            cam.pitch.unwrap_or(0.0) as f32,
        );

        // Resolve [[camera.path]] keypoints; omitted fields inherit the base
        // (folded) camera framing
        let camera_path = match &cam.path {
            Some(defs) if !defs.is_empty() => {
                if defs.len() < 2 {
                    return Err("camera.path needs at least 2 keypoints".to_string());
                }
                Some(CameraPath {
                    keys: defs
                        .iter()
                        .map(|k| PathKey {
                            yaw: k.yaw.map(|v| v as f32).unwrap_or(folded.yaw),
                            pitch: k.pitch.map(|v| v as f32).unwrap_or(folded.pitch),
                            distance: k.distance.map(|v| v as f32).unwrap_or(folded.distance),
                            focus: k
                                .focus
                                .map(|f| Vec3::from(f.map(|v| v as f32)))
                                .unwrap_or(camera_focus),
                        })
                        .collect(),
                    closed: cam.path_closed.unwrap_or(false),
                    ease: cam.path_ease,
                    seconds: cam.path_seconds.map(|v| v as f32),
                })
            }
            _ => None,
        };

        Ok(Scene {
            name: scene_file.meta.name,
            author: scene_file.meta.author.unwrap_or_else(|| "Unknown".to_string()),
            point_size: scene_file.meta.point_size as f32,
            points_per_frame: scene_file.meta.points_per_frame,
            point_count: scene_file.meta.point_count.unwrap_or(DEFAULT_POINT_COUNT),
            decay: scene_file.meta.decay as f32,
            color_speed: scene_file.meta.color_speed as f32,
            color_falloff: (scene_file.meta.color_falloff as f32).max(0.0),
            color_contrast: (scene_file.meta.color_contrast as f32).max(0.0),
            transforms,
            transform_names,
            colors: transform_colors,
            colormap,
            camera_focus,
            camera_distance: folded.distance,
            camera_yaw: folded.yaw,
            camera_pitch: folded.pitch,
            camera_path,
        })
    }

    /// Rebuild the colormap from the per-transform colors (after color edits
    /// or transform add/remove)
    pub fn regenerate_colormap(&mut self) {
        self.colormap = generate_colormap(&self.colors);
    }

    /// Save the scene back to a TOML file, decomposing each transform matrix
    /// into translation / uniform scale / XYZ euler rotation (the format the
    /// loader understands). Only exact for matrices built that way — which is
    /// everything the loader and the in-app gizmo editing produce.
    /// Build the serialization structs for the current scene state
    fn to_scene_file(&self) -> SceneFile {
        let transforms: Vec<TransformDef> = self
            .transforms
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let (scale, rot, trans) = spec.matrix.to_scale_rotation_translation();
                let (rx, ry, rz) = rot.to_euler(EulerRot::XYZ);

                let variations: BTreeMap<String, f64> = spec
                    .variations
                    .iter()
                    .enumerate()
                    .filter(|&(_, &w)| w != 0.0)
                    .map(|(slot, &w)| (VARIATION_NAMES[slot].to_string(), tidy(w)))
                    .collect();
                let is_pure_linear =
                    variations.len() == 1 && variations.get("linear") == Some(&1.0);

                let color = self.colors.get(i).copied().unwrap_or(Vec3::ONE);
                let scale = if approx(scale.x as f64, scale.y as f64)
                    && approx(scale.x as f64, scale.z as f64)
                {
                    ScaleDef::Uniform(tidy(scale.x))
                } else {
                    ScaleDef::PerAxis(scale.to_array().map(tidy))
                };

                TransformDef {
                    name: self.transform_names.get(i).cloned().flatten(),
                    translation: trans.to_array().map(tidy),
                    scale,
                    rotation: [rx, ry, rz].map(|r| tidy(r.to_degrees())),
                    color: color.to_array().map(tidy),
                    weight: tidy(spec.weight),
                    color_value: Some(tidy(spec.color_value)),
                    color_speed: spec.explicit_color_speed.map(tidy),
                    variations: if is_pure_linear || variations.is_empty() {
                        None
                    } else {
                        Some(variations)
                    },
                }
            })
            .collect();

        SceneFile {
            meta: SceneMeta {
                name: self.name.clone(),
                author: Some(self.author.clone()),
                point_size: tidy(self.point_size),
                points_per_frame: self.points_per_frame,
                decay: tidy(self.decay),
                color_speed: tidy(self.color_speed),
                color_falloff: tidy(self.color_falloff),
                color_contrast: tidy(self.color_contrast),
                point_count: Some(self.point_count),
            },
            camera: Some({
                // A 1-key path is a transient in-app authoring state; the
                // loader requires 2+ keys, so don't write it out
                let path = self.camera_path.as_ref().filter(|p| p.keys.len() >= 2);
                CameraDef {
                    focus: Some(self.camera_focus.to_array().map(tidy)),
                    offset: None,
                    distance: Some(tidy(self.camera_distance)),
                    yaw: Some(tidy(self.camera_yaw)),
                    pitch: Some(tidy(self.camera_pitch)),
                    path_closed: path.and_then(|p| p.closed.then_some(true)),
                    path_seconds: path.and_then(|p| p.seconds.map(tidy)),
                    path_ease: path.and_then(|p| p.ease),
                    path: path.map(|p| {
                        p.keys
                            .iter()
                            .map(|k| PathKeyDef {
                                yaw: Some(tidy(k.yaw)),
                                pitch: Some(tidy(k.pitch)),
                                distance: Some(tidy(k.distance)),
                                focus: Some(k.focus.to_array().map(tidy)),
                            })
                            .collect()
                    }),
                }
            }),
            transforms,
        }
    }

    /// Save the scene back to a TOML file, decomposing each transform matrix
    /// into translation / uniform scale / XYZ euler rotation (the format the
    /// loader understands). Only exact for matrices built that way — which is
    /// everything the loader and the in-app gizmo editing produce.
    ///
    /// When the target file already exists, the existing document is edited
    /// in place: comments and formatting are preserved, and only values that
    /// actually changed are rewritten. The one structural exception is the
    /// transform list — if transforms were added or removed, the whole
    /// [[transform]] array is rebuilt (per-transform comments are lost, but
    /// the header/meta/camera comments survive).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let file = self.to_scene_file();

        // Fresh serialization: used for new files, and as the fallback (and
        // transform-array donor) for the comment-preserving merge
        let fresh = toml::to_string(&file)
            .map_err(|e| format!("Failed to serialize scene: {}", e))?;
        let fresh = inline_variation_tables(&fresh).unwrap_or(fresh);

        let content = fs::read_to_string(path.as_ref())
            .ok()
            .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|doc| merge_scene_into_document(doc, &file, &fresh))
            .unwrap_or(fresh);

        if let Some(dir) = path.as_ref().parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
        }
        fs::write(path.as_ref(), content)
            .map_err(|e| format!("Failed to write scene file: {}", e))
    }
}

/// Round in f64 space: trims both decomposition noise and the f32->f64
/// representation error that would otherwise litter files with values like
/// 0.0020000000949949026. `+ 0.0` normalizes -0.0.
fn tidy(v: f32) -> f64 {
    (v as f64 * 10_000.0).round() / 10_000.0 + 0.0
}

// === Comment-preserving merge helpers ===

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 5e-5
}

fn value_as_f64(v: &toml_edit::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// Replace a key's value, keeping the old value's decor (e.g. a trailing
/// `# comment` on the same line)
fn set_value(table: &mut toml_edit::Table, key: &str, new: toml_edit::Value) {
    let mut new = new;
    if let Some(old) = table.get(key).and_then(|i| i.as_value()) {
        *new.decor_mut() = old.decor().clone();
    }
    table[key] = toml_edit::Item::Value(new);
}

/// Set a float key unless it already holds (approximately) this value.
/// `default`: skip writing entirely when the key is absent and the value
/// equals the loader's default — keeps minimal files minimal.
fn set_f64(table: &mut toml_edit::Table, key: &str, v: f64, default: Option<f64>) {
    match table.get(key).and_then(|i| i.as_value()).and_then(value_as_f64) {
        Some(old) if approx(old, v) => {}
        Some(_) => set_value(table, key, v.into()),
        None => {
            if !default.is_some_and(|d| approx(d, v)) {
                set_value(table, key, v.into());
            }
        }
    }
}

fn set_i64(table: &mut toml_edit::Table, key: &str, v: i64, default: Option<i64>) {
    match table.get(key).and_then(|i| i.as_integer()) {
        Some(old) if old == v => {}
        Some(_) => set_value(table, key, v.into()),
        None => {
            if default != Some(v) {
                set_value(table, key, v.into());
            }
        }
    }
}

fn set_str(table: &mut toml_edit::Table, key: &str, v: &str) {
    if table.get(key).and_then(|i| i.as_str()) != Some(v) {
        set_value(table, key, v.into());
    }
}

fn set_bool(table: &mut toml_edit::Table, key: &str, v: bool) {
    if table.get(key).and_then(|i| i.as_bool()) != Some(v) {
        set_value(table, key, v.into());
    }
}

fn set_arr3(table: &mut toml_edit::Table, key: &str, v: [f64; 3], default: Option<[f64; 3]>) {
    if let Some(old) = table.get(key).and_then(|i| i.as_array()) {
        let matches = old.len() == 3
            && old
                .iter()
                .zip(v)
                .all(|(o, n)| value_as_f64(o).is_some_and(|x| approx(x, n)));
        if matches {
            return;
        }
    } else if default.is_some_and(|d| d.iter().zip(v).all(|(a, b)| approx(*a, b))) {
        return;
    }
    let mut arr = toml_edit::Array::new();
    for x in v {
        arr.push(x);
    }
    set_value(table, key, toml_edit::Value::Array(arr));
}

fn set_variations(table: &mut toml_edit::Table, vars: &Option<BTreeMap<String, f64>>) {
    let Some(map) = vars else {
        table.remove("variations");
        return;
    };
    // Compare against the existing table (inline or section form)
    let old: Option<BTreeMap<String, f64>> = match table.get("variations") {
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => Some(
            t.iter()
                .filter_map(|(k, v)| value_as_f64(v).map(|f| (k.to_string(), f)))
                .collect(),
        ),
        Some(toml_edit::Item::Table(t)) => Some(
            t.iter()
                .filter_map(|(k, i)| {
                    i.as_value().and_then(value_as_f64).map(|f| (k.to_string(), f))
                })
                .collect(),
        ),
        _ => None,
    };
    if let Some(old) = &old {
        let same = old.len() == map.len()
            && old
                .iter()
                .zip(map.iter())
                .all(|((ka, va), (kb, vb))| ka == kb && approx(*va, *vb));
        if same {
            return;
        }
    }
    let mut it = toml_edit::InlineTable::new();
    for (k, v) in map {
        it.insert(k, (*v).into());
    }
    it.fmt();
    let had = table.remove("variations").is_some();
    let _ = had;
    table["variations"] = toml_edit::value(it);
}

/// Edit an existing scene document in place so comments and formatting
/// survive a save. Returns None (falling back to `fresh`) on any structural
/// surprise.
fn merge_scene_into_document(
    mut doc: toml_edit::DocumentMut,
    file: &SceneFile,
    fresh: &str,
) -> Option<String> {
    {
        let meta = doc
            .entry("meta")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()?;
        set_str(meta, "name", &file.meta.name);
        if let Some(a) = &file.meta.author {
            set_str(meta, "author", a);
        }
        set_f64(meta, "point_size", file.meta.point_size, Some(0.012));
        set_i64(meta, "points_per_frame", file.meta.points_per_frame as i64, Some(0));
        set_f64(meta, "decay", file.meta.decay, Some(0.8));
        set_f64(meta, "color_speed", file.meta.color_speed, Some(0.5));
        set_f64(meta, "color_falloff", file.meta.color_falloff, Some(0.0));
        set_f64(meta, "color_contrast", file.meta.color_contrast, Some(1.0));
        if let Some(pc) = file.meta.point_count {
            set_i64(meta, "point_count", pc as i64, None);
        }
    }

    {
        let cam = file.camera.as_ref()?;
        let camera = doc
            .entry("camera")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()?;
        set_arr3(camera, "focus", cam.focus?, Some([0.0; 3]));
        set_f64(camera, "distance", cam.distance?, None);
        set_f64(camera, "yaw", cam.yaw?, Some(0.0));
        set_f64(camera, "pitch", cam.pitch?, Some(0.0));
        // Folded into yaw/pitch/distance at load; leaving it would
        // double-apply on the next load
        camera.remove("offset");

        match cam.path_closed {
            Some(b) => set_bool(camera, "path_closed", b),
            None => {
                camera.remove("path_closed");
            }
        }
        match cam.path_seconds {
            Some(s) => set_f64(camera, "path_seconds", s, None),
            None => {
                camera.remove("path_seconds");
            }
        }
        match cam.path_ease {
            Some(b) => set_bool(camera, "path_ease", b),
            None => {
                camera.remove("path_ease");
            }
        }
        match &cam.path {
            None => {
                camera.remove("path");
            }
            Some(keys) => {
                let existing = camera
                    .get("path")
                    .and_then(|i| i.as_array_of_tables())
                    .map(|a| a.len());
                if existing == Some(keys.len()) {
                    // Same key count: update values in place (keeps comments)
                    let arr = camera.get_mut("path")?.as_array_of_tables_mut()?;
                    for (t, def) in arr.iter_mut().zip(keys) {
                        if let Some(y) = def.yaw {
                            set_f64(t, "yaw", y, None);
                        }
                        if let Some(p) = def.pitch {
                            set_f64(t, "pitch", p, None);
                        }
                        if let Some(d) = def.distance {
                            set_f64(t, "distance", d, None);
                        }
                        if let Some(f) = def.focus {
                            set_arr3(t, "focus", f, None);
                        }
                    }
                } else {
                    // Key count changed: rebuild from the fresh serialization
                    let fresh_doc: toml_edit::DocumentMut = fresh.parse().ok()?;
                    camera["path"] = fresh_doc
                        .get("camera")
                        .and_then(|c| c.as_table())
                        .and_then(|c| c.get("path"))?
                        .clone();
                }
            }
        }
    }

    let existing_len = doc
        .get("transform")
        .and_then(|t| t.as_array_of_tables())
        .map(|a| a.len());
    if existing_len == Some(file.transforms.len()) {
        let arr = doc.get_mut("transform")?.as_array_of_tables_mut()?;
        for (i, (t, def)) in arr.iter_mut().zip(&file.transforms).enumerate() {
            match &def.name {
                Some(n) => set_str(t, "name", n),
                None => {
                    t.remove("name");
                }
            }
            set_arr3(t, "translation", def.translation, None);
            match def.scale {
                ScaleDef::Uniform(s) => {
                    // An existing array form won't approx-match a float; clear
                    // it so set_f64 writes the scalar cleanly
                    if t.get("scale").is_some_and(|i| i.as_array().is_some()) {
                        t.remove("scale");
                    }
                    set_f64(t, "scale", s, Some(1.0));
                }
                ScaleDef::PerAxis(a) => set_arr3(t, "scale", a, None),
            }
            set_arr3(t, "rotation", def.rotation, Some([0.0; 3]));
            set_arr3(t, "color", def.color, None);
            set_f64(t, "weight", def.weight, Some(1.0));
            if let Some(cv) = def.color_value {
                // The loader auto-assigns i/(n-1); don't add an explicit key
                // when the value still matches that
                let n = file.transforms.len();
                let auto = if n <= 1 { 0.5 } else { i as f64 / (n - 1) as f64 };
                set_f64(t, "color_value", cv, Some(tidy(auto as f32)));
            }
            match def.color_speed {
                Some(cs) => set_f64(t, "color_speed", cs, None),
                None => {
                    t.remove("color_speed");
                }
            }
            set_variations(t, &def.variations);
        }
    } else {
        // Transform count changed: rebuild the array from the fresh
        // serialization (loses per-transform comments only)
        let fresh_doc: toml_edit::DocumentMut = fresh.parse().ok()?;
        doc["transform"] = fresh_doc.get("transform")?.clone();
    }

    Some(doc.to_string())
}

/// Rewrite each `[transform.variations]` sub-table as an inline
/// `variations = { ... }` on the transform itself
fn inline_variation_tables(content: &str) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = content.parse().ok()?;
    let transforms = doc.get_mut("transform")?.as_array_of_tables_mut()?;
    for t in transforms.iter_mut() {
        if let Some(toml_edit::Item::Table(vars)) = t.get("variations") {
            let mut inline = vars.clone().into_inline_table();
            inline.fmt();
            // Remove first so the fresh key gets clean `key = value` decor
            t.remove("variations");
            t["variations"] = toml_edit::value(inline);
        }
    }
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loader's rotation convention, pinned as a test: EulerRot::XYZ
    /// composes as Rx * Ry * Rz on column vectors. External generators
    /// (tools/lsystem_to_ifs.py) decompose matrices assuming exactly this.
    #[test]
    fn euler_xyz_is_rx_ry_rz() {
        let (a, b, c) = (0.3f32, -0.7, 1.1);
        let q = Quat::from_euler(glam::EulerRot::XYZ, a, b, c);
        let m = glam::Mat3::from_rotation_x(a)
            * glam::Mat3::from_rotation_y(b)
            * glam::Mat3::from_rotation_z(c);
        let diff = (glam::Mat3::from_quat(q) - m).to_cols_array();
        let max = diff.iter().fold(0.0f32, |acc, v| acc.max(v.abs()));
        assert!(max < 1e-5, "EulerRot::XYZ convention drift: {}", max);
    }

    #[test]
    fn variations_serialize_inline() {
        let scene = {
            let dir = std::env::temp_dir().join("fracturize_inline_test");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("v.toml");
            std::fs::write(
                &path,
                r#"
[meta]
name = "Inline"

[[transform]]
translation = [0.0, 0.0, 0.5]
scale = 0.5
color = [1.0, 0.2, 0.2]
variations = { swirl = 0.35, linear = 0.65 }
"#,
            )
            .unwrap();
            Scene::load(&path).unwrap()
        };
        let dir = std::env::temp_dir().join("fracturize_inline_test");
        let out = dir.join("saved.toml");
        scene.save(&out).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("variations = {"),
            "expected inline variations, got:\n{}",
            content
        );
        assert!(!content.contains("[transform.variations]"), "{}", content);
        Scene::load(&out).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn merge_save_preserves_comments() {
        let src = r#"# A scene with a story in its comments.
# This header block must survive saving.

[meta]
name = "Commented"
author = "test"
point_size = 0.0015
point_count = 6_000_000
color_speed = 0.65

[camera]
distance = 3.0
offset = [0.0, 0.0, 0.0]
focus = [0.0, 0.0, 0.0]

# The ocean: automatic processing. This comment matters.
[[transform]]
name = "ocean"
translation = [0.55, 0.1, -0.3]
scale = 0.7
rotation = [8.0, 35.0, 18.0]
color = [0.22, 0.30, 0.52] # cool blue
weight = 2.4
variations = { swirl = 0.3, linear = 0.7 }

# The workspace: small, hot, dense.
[[transform]]
name = "workspace"
translation = [-0.85, 0.05, 0.15]
scale = 0.32
color = [1.0, 0.58, 0.22]
weight = 1.2
"#;
        let dir = std::env::temp_dir().join("fracturize_merge_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commented.toml");
        std::fs::write(&path, src).unwrap();

        let mut scene = Scene::load(&path).unwrap();
        // Edit only transform 1's weight (the probability lever)
        scene.transforms[1].weight = 1.9;
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();

        // Comments survive
        assert!(out.contains("# A scene with a story in its comments."), "{}", out);
        assert!(out.contains("# The ocean: automatic processing. This comment matters."), "{}", out);
        assert!(out.contains("# The workspace: small, hot, dense."), "{}", out);
        assert!(out.contains("# cool blue"), "{}", out);
        // Untouched values keep their exact formatting
        assert!(out.contains("translation = [0.55, 0.1, -0.3]"), "{}", out);
        assert!(out.contains("point_count = 6_000_000"), "{}", out);
        assert!(out.contains("variations = { swirl = 0.3, linear = 0.7 }"), "{}", out);
        // The legacy offset is folded away, and the edit landed
        assert!(!out.contains("offset"), "{}", out);
        assert!(out.contains("weight = 1.9"), "{}", out);
        // And the merged file still loads to the same state
        let reloaded = Scene::load(&path).unwrap();
        assert!((reloaded.transforms[1].weight - 1.9).abs() < 1e-4);
        let diff = (reloaded.transforms[0].matrix - scene.transforms[0].matrix).to_cols_array();
        assert!(diff.iter().all(|v| v.abs() < 1e-4));

        // Add a transform: the array is rebuilt, header comments survive
        scene.transforms.push(scene.transforms[0].clone());
        scene.transform_names.push(None);
        scene.colors.push(Vec3::ONE);
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# A scene with a story in its comments."), "{}", out);
        assert_eq!(Scene::load(&path).unwrap().transforms.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scene_save_load_roundtrip() {
        let src = r#"
[meta]
name = "Roundtrip"
author = "test"
point_size = 0.002
color_speed = 0.4
color_falloff = 0.8
color_contrast = 2.0
point_count = 123456

[camera]
focus = [0.0, 1.0, 0.0]
offset = [0.0, 1.5, 0.0]
distance = 3.5

[[transform]]
name = "Spine"
translation = [0.1, 0.15, -0.2]
scale = 0.95
rotation = [10.0, 30.0, -45.0]
color = [1.0, 0.0, 0.8]
weight = 3.0
color_speed = 0.1
variations = { spherical = 0.7, linear = 0.3 }

[[transform]]
translation = [0.8, 0.0, 0.0]
scale = 0.3
color = [0.0, 1.0, 1.0]
"#;
        let dir = std::env::temp_dir().join("fracturize_scene_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src.toml");
        std::fs::write(&src_path, src).unwrap();

        let scene = Scene::load(&src_path).unwrap();
        let saved_path = dir.join("saved.toml");
        scene.save(&saved_path).unwrap();
        let reloaded = Scene::load(&saved_path).unwrap();

        assert_eq!(reloaded.name, scene.name);
        assert_eq!(reloaded.point_count, scene.point_count);
        assert_eq!(reloaded.color_falloff, scene.color_falloff);
        assert_eq!(reloaded.transform_names, scene.transform_names);
        assert_eq!(reloaded.transforms.len(), scene.transforms.len());
        for (a, b) in scene.transforms.iter().zip(reloaded.transforms.iter()) {
            let diff = (a.matrix - b.matrix).to_cols_array();
            let max = diff.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(max < 1e-3, "matrix drift {} exceeds tolerance", max);
            assert!((a.weight - b.weight).abs() < 1e-4);
            assert!((a.color_value - b.color_value).abs() < 1e-4);
            assert_eq!(a.explicit_color_speed.is_some(), b.explicit_color_speed.is_some());
            for (wa, wb) in a.variations.iter().zip(b.variations.iter()) {
                assert!((wa - wb).abs() < 1e-4);
            }
        }
        for (ca, cb) in scene.colors.iter().zip(reloaded.colors.iter()) {
            assert!((*ca - *cb).length() < 1e-3);
        }
        assert!((scene.camera_distance - reloaded.camera_distance).abs() < 1e-4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn camera_path_roundtrip() {
        let src = r#"
[meta]
name = "Pathed"

[camera]
focus = [0.0, 0.2, 0.0]
distance = 3.0
yaw = 0.5
pitch = 0.3
path_closed = true
path_seconds = 8.0

# approach from afar
[[camera.path]]
yaw = 0.0
distance = 6.0

[[camera.path]]
yaw = 3.14
pitch = 0.6
focus = [0.0, 0.5, 0.0]

[[camera.path]]
yaw = 6.28
distance = 1.5

[[transform]]
translation = [0.0, 0.0, 0.5]
scale = 0.5
color = [1.0, 0.2, 0.2]
"#;
        let dir = std::env::temp_dir().join("fracturize_path_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pathed.toml");
        std::fs::write(&path, src).unwrap();

        let scene = Scene::load(&path).unwrap();
        let p = scene.camera_path.as_ref().expect("path parsed");
        assert_eq!(p.keys.len(), 3);
        assert!(p.closed);
        assert_eq!(p.seconds, Some(8.0));
        // Omitted fields inherit the base camera
        assert!((p.keys[0].pitch - 0.3).abs() < 1e-6, "pitch inherits base");
        assert!((p.keys[0].focus - Vec3::new(0.0, 0.2, 0.0)).length() < 1e-6);
        assert!((p.keys[1].distance - 3.0).abs() < 1e-6, "distance inherits base");
        assert!((p.keys[1].focus - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-6);

        // Merge-save keeps the path (and its comment); values survive a reload
        scene.save(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# approach from afar"), "{}", out);
        assert!(out.contains("[[camera.path]]"), "{}", out);
        let reloaded = Scene::load(&path).unwrap();
        let q = reloaded.camera_path.as_ref().unwrap();
        assert_eq!(q.keys.len(), 3);
        assert!(q.closed);
        for (a, b) in p.keys.iter().zip(&q.keys) {
            assert!((a.yaw - b.yaw).abs() < 1e-3);
            assert!((a.pitch - b.pitch).abs() < 1e-3);
            assert!((a.distance - b.distance).abs() < 1e-3);
            assert!((a.focus - b.focus).length() < 1e-3);
        }

        // Fresh save (new file) round-trips too
        let fresh_path = dir.join("fresh.toml");
        scene.save(&fresh_path).unwrap();
        let fresh = Scene::load(&fresh_path).unwrap();
        assert_eq!(fresh.camera_path.as_ref().unwrap().keys.len(), 3);

        // A scene without a path stays path-free after save
        let no_path = {
            let p2 = dir.join("nopath.toml");
            std::fs::write(&p2, "[meta]\nname = \"n\"\n\n[[transform]]\ntranslation = [0.0, 0.0, 0.5]\nscale = 0.5\ncolor = [1.0, 1.0, 1.0]\n").unwrap();
            let s = Scene::load(&p2).unwrap();
            s.save(&p2).unwrap();
            Scene::load(&p2).unwrap()
        };
        assert!(no_path.camera_path.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn per_axis_scale_roundtrip() {
        let src = r#"
[meta]
name = "Anisotropic"

[[transform]]
translation = [0.0, 0.5, 0.0]
scale = [0.05, 0.6, 0.05]
rotation = [0.0, 20.0, 5.0]
color = [0.4, 0.8, 0.3]

[[transform]]
translation = [0.3, 0.0, 0.0]
scale = 0.7
color = [0.9, 0.5, 0.2]
"#;
        let dir = std::env::temp_dir().join("fracturize_scale_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src.toml");
        std::fs::write(&src_path, src).unwrap();

        let scene = Scene::load(&src_path).unwrap();
        let (s0, _, _) = scene.transforms[0].matrix.to_scale_rotation_translation();
        assert!((s0 - Vec3::new(0.05, 0.6, 0.05)).length() < 1e-4);

        // Round-trip through both save paths: in-place merge (existing file)
        // and fresh serialization (new file)
        for path in [&src_path, &dir.join("fresh.toml")] {
            scene.save(path).unwrap();
            let reloaded = Scene::load(path).unwrap();
            for (a, b) in scene.transforms.iter().zip(reloaded.transforms.iter()) {
                let diff = (a.matrix - b.matrix).to_cols_array();
                let max = diff.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(max < 1e-3, "matrix drift {} exceeds tolerance", max);
            }
        }
        // Uniform transform must stay scalar in the file
        let saved = std::fs::read_to_string(&src_path).unwrap();
        assert!(saved.contains("scale = 0.7"), "uniform scale kept scalar:\n{saved}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Generate a 256-color gradient from transform colors
/// Creates smooth interpolation between colors spaced evenly across the gradient
/// Always cyclic (last color blends to first)
fn generate_colormap(colors: &[Vec3]) -> Colormap {
    let mut colormap = [[0.0f32; 4]; 256];

    if colors.is_empty() {
        // Default: white
        for entry in &mut colormap {
            *entry = [1.0, 1.0, 1.0, 1.0];
        }
        return colormap;
    }

    if colors.len() == 1 {
        // Single color: fill entire map
        let c = colors[0];
        for entry in &mut colormap {
            *entry = [c.x, c.y, c.z, 1.0];
        }
        return colormap;
    }

    // Multiple colors: interpolate between them
    // Cyclic: treat the sequence as wrapping around (last connects to first)
    let n = colors.len();
    
    for i in 0..256 {
        let t = i as f32 / 256.0; // 0.0 to <1.0 for cyclic
        
        let scaled = t * n as f32;
        let idx0 = scaled.floor() as usize;
        let idx1 = (idx0 + 1) % colors.len();
        let local_t = scaled - idx0 as f32;

        // Linear interpolation
        let c0 = colors[idx0];
        let c1 = colors[idx1];
        let c = c0 * (1.0 - local_t) + c1 * local_t;

        colormap[i] = [c.x, c.y, c.z, 1.0];
    }

    colormap
}
