use glam::{Mat4, Quat, Vec3};
use serde::Deserialize;
use std::fs;
use std::path::Path;

/// 256-color gradient for Apophysis-style rendering
pub type Colormap = [[f32; 4]; 256];

/// Scene metadata from TOML
#[derive(Deserialize)]
pub struct SceneMeta {
    pub name: String,
    pub author: Option<String>,
    #[serde(default = "default_point_size")]
    pub point_size: f32,
    /// Points generated per frame by the chaos game
    #[serde(alias = "iters")] // backwards compat
    pub points_per_frame: usize,
    /// Temporal decay factor (0.0-1.0). Lower = sharper, higher = more accumulation
    #[serde(default = "default_decay")]
    pub decay: f32,
    #[serde(default = "default_color_speed")]
    pub color_speed: f32,
}

fn default_point_size() -> f32 {
    0.012
}

fn default_color_speed() -> f32 {
    0.5
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
    /// Overrides global scene color_speed if set
    #[serde(default)]
    pub color_speed: Option<f32>,
}

fn default_scale() -> f32 {
    1.0
}

fn default_weight() -> f32 {
    1.0
}

/// Full scene file structure
#[derive(Deserialize)]
pub struct SceneFile {
    pub meta: SceneMeta,
    #[serde(rename = "transform")]
    pub transforms: Vec<TransformDef>,
}

/// Loaded scene ready for use
pub struct Scene {
    pub name: String,
    pub author: String,
    pub point_size: f32,
    /// Points generated per frame by chaos game
    pub points_per_frame: usize,
    /// Temporal decay factor (0.0-1.0)
    pub decay: f32,
    pub color_speed: f32,
    /// Transforms: (matrix, color_value, weight, color_speed)
    /// color_value is 0.0-1.0 index into colormap
    pub transforms: Vec<(Mat4, f32, f32, f32)>,
    /// 256-color gradient for point coloring
    pub colormap: Colormap,
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

        let transforms: Vec<(Mat4, f32, f32, f32)> = scene_file
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

                // Use transform-specific speed or global default
                let speed = t.color_speed.unwrap_or(global_speed);

                (matrix, color_value, t.weight, speed)
            })
            .collect();

        // Generate colormap from transform colors (always cyclic)
        let colormap = generate_colormap(&transform_colors);

        Ok(Scene {
            name: scene_file.meta.name,
            author: scene_file.meta.author.unwrap_or_else(|| "Unknown".to_string()),
            point_size: scene_file.meta.point_size,
            points_per_frame: scene_file.meta.points_per_frame,
            decay: scene_file.meta.decay,
            color_speed: scene_file.meta.color_speed,
            transforms,
            colormap,
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
