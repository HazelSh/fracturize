use glam::{Mat4, Quat, Vec3};
use serde::Deserialize;
use std::fs;
use std::path::Path;

/// Scene metadata from TOML
#[derive(Deserialize)]
pub struct SceneMeta {
    pub name: String,
    pub author: Option<String>,
    #[serde(default = "default_point_size")]
    pub point_size: f32,
    pub iters: usize,
    pub max_points: usize,

}

fn default_point_size() -> f32 {
    0.012
}

/// Transform definition in TOML (human-readable format)
#[derive(Deserialize)]
pub struct TransformDef {
    pub translation: [f32; 3],
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub rotation: [f32; 3], // Euler angles in degrees (pitch, yaw, roll)
    pub color: [f32; 3],
    #[serde(default = "default_weight")]
    pub weight: f32,
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
    pub iters: usize,
    pub max_points: usize,
    pub transforms: Vec<(Mat4, Vec3, f32)>, // (matrix, color, weight)
}

impl Scene {
    /// Load a scene from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read scene file: {}", e))?;

        let scene_file: SceneFile = toml::from_str(&content)
            .map_err(|e| format!("Failed to parse scene file: {}", e))?;

        let transforms = scene_file
            .transforms
            .iter()
            .map(|t| {
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

                let color = Vec3::from(t.color);

                (matrix, color, t.weight)
            })
            .collect();

        Ok(Scene {
            name: scene_file.meta.name,
            author: scene_file.meta.author.unwrap_or_else(|| "Unknown".to_string()),
            point_size: scene_file.meta.point_size,
            iters: scene_file.meta.iters,
            max_points: scene_file.meta.max_points,
            transforms,
        })
    }
}
