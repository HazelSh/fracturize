use glam::{Mat4, Vec3, Vec4};
use miniquad::*;
use rand::prelude::*;
use rand_xoshiro::Xoshiro256PlusPlus;

/// A single IFS transform with affine matrix and color
#[derive(Clone, Debug)]
struct Transform {
    /// 4x4 affine transform matrix (3D)
    matrix: Mat4,
    /// Color contribution (RGB + intensity)
    color: Vec4,
    /// Weight for random selection
    weight: f32,
}

impl Transform {
    fn linear(matrix: Mat4, color: Vec3, weight: f32) -> Self {
        Self {
            matrix,
            color: color.extend(1.0),
            weight,
        }
    }

    fn apply(&self, point: Vec3) -> Vec3 {
        (self.matrix * point.extend(1.0)).truncate()
    }
}

/// The IFS fractal system
struct IfsSystem {
    transforms: Vec<Transform>,
    cumulative_weights: Vec<f32>,
}

impl IfsSystem {
    fn new(transforms: Vec<Transform>) -> Self {
        let mut cumulative = Vec::with_capacity(transforms.len());
        let mut sum = 0.0;
        for t in &transforms {
            sum += t.weight;
            cumulative.push(sum);
        }
        // Normalize
        for w in &mut cumulative {
            *w /= sum;
        }
        Self {
            transforms,
            cumulative_weights: cumulative,
        }
    }

    fn select_transform(&self, rng: &mut impl Rng) -> usize {
        let r: f32 = rng.r#gen();
        self.cumulative_weights
            .iter()
            .position(|&w| r < w)
            .unwrap_or(self.transforms.len() - 1)
    }
}

/// A point with position and accumulated color
#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Point {
    pos: Vec3,
    color: Vec4,
}

/// Vertex for rendering (with UV for circle shader)
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

unsafe impl bytemuck::Pod for Vertex {}
unsafe impl bytemuck::Zeroable for Vertex {}

const POINT_SIZE: f32 = 0.012; // Billboard size in world units

/// Main application state
struct Stage {
    ctx: Box<dyn RenderingBackend>,
    pipeline: Pipeline,
    bindings: Bindings,
    ifs: IfsSystem,
    rng: Xoshiro256PlusPlus,
    current_point: Vec3,
    current_color: Vec4,
    points: Vec<Point>,
    max_points: usize,
    iterations_per_frame: usize,
    camera_distance: f32,
    rotation: f32,
}

impl Stage {
    fn new() -> Self {
        let mut ctx: Box<dyn RenderingBackend> = window::new_rendering_backend();

        // Create a classic 3D Sierpinski tetrahedron
        let transforms = vec![
            Transform::linear(
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.0, 0.0, 0.5),
                ),
                Vec3::new(1.0, 0.2, 0.2), // Red
                1.0,
            ),
            Transform::linear(
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.0, 0.47, -0.17),
                ),
                Vec3::new(0.2, 1.0, 0.2), // Green
                1.0,
            ),
            Transform::linear(
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(-0.41, -0.24, -0.17),
                ),
                Vec3::new(0.2, 0.2, 1.0), // Blue
                1.0,
            ),
            Transform::linear(
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5),
                    glam::Quat::IDENTITY,
                    Vec3::new(0.41, -0.24, -0.17),
                ),
                Vec3::new(1.0, 1.0, 0.2), // Yellow
                1.0,
            ),
        ];

        let ifs = IfsSystem::new(transforms);

        // Shader for point rendering
        let shader = ctx
            .new_shader(
                ShaderSource::Glsl {
                    vertex: VERTEX_SHADER,
                    fragment: FRAGMENT_SHADER,
                },
                ShaderMeta {
                    images: vec![],
                    uniforms: UniformBlockLayout {
                        uniforms: vec![UniformDesc::new("mvp", UniformType::Mat4)],
                    },
                },
            )
            .unwrap();

        let pipeline = ctx.new_pipeline(
            &[BufferLayout::default()],
            &[
                VertexAttribute::new("pos", VertexFormat::Float3),
                VertexAttribute::new("uv", VertexFormat::Float2),
                VertexAttribute::new("color", VertexFormat::Float4),
            ],
            shader,
            PipelineParams {
                primitive_type: PrimitiveType::Triangles,
                cull_face: CullFace::Nothing,
                ..Default::default()
            },
        );

        let max_points = 500_000;

        // Create vertex buffer (will be updated each frame)
        // 6 vertices per point (2 triangles = 1 quad) + some extra for debug geometry
        let vertex_buffer = ctx.new_buffer(
            BufferType::VertexBuffer,
            BufferUsage::Stream,
            BufferSource::empty::<Vertex>(max_points * 6 + 1000),
        );

        // Index buffer - generate indices for all quads (6 indices per quad = 2 triangles)
        // Each quad uses vertices [i*4, i*4+1, i*4+2, i*4+3] but we use 6 verts per quad
        // So indices are just sequential: 0,1,2,3,4,5, 6,7,8,9,10,11, ...
        let indices: Vec<u32> = (0..((max_points * 6) as u32)).collect();
        let index_buffer = ctx.new_buffer(
            BufferType::IndexBuffer,
            BufferUsage::Immutable,
            BufferSource::slice(&indices),
        );

        let bindings = Bindings {
            vertex_buffers: vec![vertex_buffer],
            index_buffer,
            images: vec![],
        };

        let mut rng = Xoshiro256PlusPlus::seed_from_u64(42);
        let current_point = Vec3::new(
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        );

        Self {
            ctx,
            pipeline,
            bindings,
            ifs,
            rng,
            current_point,
            current_color: Vec4::new(0.5, 0.5, 0.5, 1.0),
            points: Vec::with_capacity(max_points),
            max_points,
            iterations_per_frame: 50_000,
            camera_distance: 3.0,
            rotation: 0.0,
        }
    }

    fn iterate(&mut self) {
        // Skip first few iterations to let the point settle into the attractor
        let skip = if self.points.is_empty() { 20 } else { 0 };

        for i in 0..self.iterations_per_frame {
            let idx = self.ifs.select_transform(&mut self.rng);
            let transform = &self.ifs.transforms[idx];

            self.current_point = transform.apply(self.current_point);
            // Blend colors (Apophysis-style color accumulation)
            self.current_color = self.current_color * 0.5 + transform.color * 0.5;

            if i >= skip {
                if self.points.len() < self.max_points {
                    self.points.push(Point {
                        pos: self.current_point,
                        color: self.current_color,
                    });
                } else {
                    // Ring buffer behavior - overwrite old points
                    let write_idx = self.rng.gen_range(0..self.max_points);
                    self.points[write_idx] = Point {
                        pos: self.current_point,
                        color: self.current_color,
                    };
                }
            }
        }
    }
}

impl EventHandler for Stage {
    fn update(&mut self) {
        self.iterate();
        self.rotation += 0.005;
    }

    fn draw(&mut self) {
        if self.points.is_empty() {
            return;
        }

        // Camera setup
        let (width, height) = window::screen_size();
        let aspect = width / height;
        let cam_pos = Vec3::new(
            self.rotation.sin() * self.camera_distance,
            1.0,
            self.rotation.cos() * self.camera_distance,
        );
        let cam_target = Vec3::ZERO;

        let projection = Mat4::perspective_rh_gl(45.0_f32.to_radians(), aspect, 0.1, 100.0);
        let view = Mat4::look_at_rh(cam_pos, cam_target, Vec3::Y);
        let mvp = projection * view;

        // Billboard basis vectors
        let forward = (cam_target - cam_pos).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);

        // Build quad vertices (6 per point = 2 triangles)
        let size = POINT_SIZE;
        let mut vertices: Vec<Vertex> = Vec::with_capacity(self.points.len() * 6);

        for p in &self.points {
            let c = p.pos;
            let col = p.color.to_array();
            let r = right * size;
            let u = up * size;

            // Two triangles forming a quad with UVs for circle shader
            vertices.push(Vertex { pos: (c - r + u).to_array(), uv: [-1.0, 1.0], color: col });
            vertices.push(Vertex { pos: (c - r - u).to_array(), uv: [-1.0, -1.0], color: col });
            vertices.push(Vertex { pos: (c + r + u).to_array(), uv: [1.0, 1.0], color: col });
            vertices.push(Vertex { pos: (c + r + u).to_array(), uv: [1.0, 1.0], color: col });
            vertices.push(Vertex { pos: (c - r - u).to_array(), uv: [-1.0, -1.0], color: col });
            vertices.push(Vertex { pos: (c + r - u).to_array(), uv: [1.0, -1.0], color: col });
        }

        self.ctx.buffer_update(
            self.bindings.vertex_buffers[0],
            BufferSource::slice(&vertices),
        );

        self.ctx.begin_default_pass(PassAction::clear_color(0.02, 0.02, 0.05, 1.0));
        self.ctx.apply_pipeline(&self.pipeline);
        self.ctx.apply_bindings(&self.bindings);

        #[repr(C)]
        struct Uniforms {
            mvp: [[f32; 4]; 4],
        }
        let uniforms = Uniforms {
            mvp: mvp.to_cols_array_2d(),
        };

        self.ctx.apply_uniforms(UniformsSource::table(&uniforms));
        self.ctx.draw(0, vertices.len() as i32, 1);
        self.ctx.end_render_pass();
        self.ctx.commit_frame();
    }

    fn key_down_event(&mut self, keycode: KeyCode, _mods: KeyMods, _repeat: bool) {
        match keycode {
            KeyCode::Escape => window::request_quit(),
            KeyCode::Space => {
                // Reset
                self.points.clear();
                self.rng = Xoshiro256PlusPlus::seed_from_u64(42);
            }
            KeyCode::Up => self.camera_distance *= 0.9,
            KeyCode::Down => self.camera_distance *= 1.1,
            _ => {}
        }
    }
}

const VERTEX_SHADER: &str = r#"
    #version 100
    attribute vec3 pos;
    attribute vec2 uv;
    attribute vec4 color;

    uniform mat4 mvp;

    varying lowp vec4 v_color;
    varying lowp vec2 v_uv;

    void main() {
        gl_Position = mvp * vec4(pos, 1.0);
        v_color = color;
        v_uv = uv;
    }
"#;

const FRAGMENT_SHADER: &str = r#"
    #version 100
    precision mediump float;
    varying lowp vec4 v_color;
    varying lowp vec2 v_uv;

    void main() {
        // Circle with soft edge
        float dist = length(v_uv);
        if (dist > 1.0) discard;
        float alpha = 1.0 - smoothstep(0.7, 1.0, dist);
        gl_FragColor = vec4(v_color.rgb, v_color.a * alpha);
    }
"#;

fn main() {
    miniquad::start(
        conf::Conf {
            window_title: "Fracturize - 3D IFS Fractal Renderer".to_string(),
            window_width: 1280,
            window_height: 720,
            high_dpi: true,
            ..Default::default()
        },
        || Box::new(Stage::new()),
    );
}
