use glam::{Mat4, Quat, Vec3};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// 256-color gradient for Apophysis-style rendering
pub type Colormap = [[f32; 4]; 256];

/// Number of variation slots per transform (must match chaos.wgsl)
pub const NUM_VARIATIONS: usize = 16;

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
fn parse_variations(table: &BTreeMap<String, f32>) -> Result<[f32; NUM_VARIATIONS], String> {
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
        weights[slot] = weight;
    }
    Ok(weights)
}

/// Scene metadata from TOML
#[derive(Deserialize)]
pub struct SceneMeta {
    pub name: String,
    pub author: Option<String>,
    #[serde(default = "default_point_size")]
    pub point_size: f32,
    /// Points generated per frame by the chaos game (legacy, unused by point renderer)
    #[serde(alias = "iters", default)] // backwards compat
    pub points_per_frame: usize,
    /// Temporal decay factor (0.0-1.0). Lower = sharper, higher = more accumulation
    #[serde(default = "default_decay")]
    pub decay: f32,
    #[serde(default = "default_color_speed")]
    pub color_speed: f32,
    /// Scale-aware color accumulation exponent (see resolve_color_speeds).
    /// 0 = classic fixed-rate EMA using color_speed. > 0 ties each EMA step's
    /// retain-factor to the transform's contraction^falloff, giving color
    /// detail at every spatial scale (power-law, no resonant size).
    /// ~1.0 is neutral; lower = more fine detail (raise color_contrast too).
    #[serde(default)]
    pub color_falloff: f32,
    /// Render-time contrast stretch of the colormap index around its center,
    /// wrapping cyclically. Compensates the wash-out from low color_falloff.
    #[serde(default = "default_color_contrast")]
    pub color_contrast: f32,
    /// Total point buffer size for the simple point renderer.
    /// If unset, defaults to 500k.
    pub point_count: Option<usize>,
}

fn default_point_size() -> f32 {
    0.012
}

fn default_color_speed() -> f32 {
    0.5
}

fn default_color_contrast() -> f32 {
    1.0
}

fn default_decay() -> f32 {
    0.8 // ~10 frame persistence
}

/// Transform definition in TOML (human-readable format)
#[derive(Deserialize)]
pub struct TransformDef {
    pub name: Option<String>,
    pub translation: [f32; 3],
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub rotation: [f32; 3], // Euler angles in degrees (pitch, yaw, roll)
    pub color: [f32; 3],
    #[serde(default = "default_weight")]
    pub weight: f32,
    /// Color value for Apophysis-style colormap indexing (0.0-1.0)
    /// If not specified, auto-assigned based on transform index
    #[serde(default)]
    pub color_value: Option<f32>,
    /// Per-transform color blending speed (0.0-1.0)
    /// Overrides global color_speed and color_falloff if set
    #[serde(default)]
    pub color_speed: Option<f32>,
    /// Variation blend weights by name, e.g. { spherical = 0.7, linear = 0.3 }
    /// Defaults to { linear = 1.0 } (classic affine IFS)
    #[serde(default)]
    pub variations: Option<BTreeMap<String, f32>>,
}

fn default_scale() -> f32 {
    1.0
}

fn default_weight() -> f32 {
    1.0
}

/// Camera configuration from TOML
#[derive(Deserialize, Default)]
pub struct CameraDef {
    /// Orbit center / look-at point
    pub focus: Option<[f32; 3]>,
    /// Added to orbital camera position
    pub offset: Option<[f32; 3]>,
    /// Orbit radius
    pub distance: Option<f32>,
}

/// Full scene file structure
#[derive(Deserialize)]
pub struct SceneFile {
    pub meta: SceneMeta,
    pub camera: Option<CameraDef>,
    #[serde(rename = "transform")]
    pub transforms: Vec<TransformDef>,
}

/// Default point buffer size for the simple point renderer
const DEFAULT_POINT_COUNT: usize = 500_000;

/// Loaded scene ready for use
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
    /// 256-color gradient for point coloring
    pub colormap: Colormap,
    /// Camera orbit center / look-at point
    pub camera_focus: Vec3,
    /// Offset added to orbital camera position
    pub camera_offset: Vec3,
    /// Camera orbit radius
    pub camera_distance: f32,
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
            .map(|t| Vec3::from(t.color))
            .collect();

        let global_speed = scene_file.meta.color_speed;

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
                    t.rotation[0].to_radians(),
                    t.rotation[1].to_radians(),
                    t.rotation[2].to_radians(),
                );

                let matrix = Mat4::from_scale_rotation_translation(
                    Vec3::splat(t.scale),
                    rotation,
                    Vec3::from(t.translation),
                );

                // Color value: use explicit if provided, otherwise distribute evenly
                let color_value = t.color_value.unwrap_or_else(|| {
                    if num_transforms == 1 {
                        0.5
                    } else {
                        i as f32 / (num_transforms - 1) as f32
                    }
                });

                // Placeholder; resolve_color_speeds computes the effective value
                let speed = t.color_speed.unwrap_or(global_speed);

                let variations = match &t.variations {
                    Some(table) => parse_variations(table)
                        .map_err(|e| format!("Transform {}: {}", i, e))?,
                    None => TransformSpec::linear_variations(),
                };

                Ok(TransformSpec {
                    matrix,
                    color_value,
                    weight: t.weight,
                    color_speed: speed,
                    explicit_color_speed: t.color_speed,
                    variations,
                })
            })
            .collect::<Result<_, String>>()?;

        let mut transforms = transforms;
        resolve_color_speeds(&mut transforms, global_speed, scene_file.meta.color_falloff);

        // Generate colormap from transform colors (always cyclic)
        let colormap = generate_colormap(&transform_colors);

        let cam = scene_file.camera.unwrap_or_default();
        let camera_focus = cam.focus.map(Vec3::from).unwrap_or(Vec3::ZERO);
        let camera_offset = cam.offset.map(Vec3::from).unwrap_or(Vec3::new(0.0, 1.0, 0.0));
        let camera_distance = cam.distance.unwrap_or(3.0);

        Ok(Scene {
            name: scene_file.meta.name,
            author: scene_file.meta.author.unwrap_or_else(|| "Unknown".to_string()),
            point_size: scene_file.meta.point_size,
            points_per_frame: scene_file.meta.points_per_frame,
            point_count: scene_file.meta.point_count.unwrap_or(DEFAULT_POINT_COUNT),
            decay: scene_file.meta.decay,
            color_speed: scene_file.meta.color_speed,
            color_falloff: scene_file.meta.color_falloff.max(0.0),
            color_contrast: scene_file.meta.color_contrast.max(0.0),
            transforms,
            transform_names,
            colormap,
            camera_focus,
            camera_offset,
            camera_distance,
        })
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
