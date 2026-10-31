//! Multiview (FEATURE-PLANS item 2): the viewport split into panes that are
//! all cameras on the one scene. A perspective pane, which is the ordinary
//! camera, and up to three **axis panes**, each looking straight down a world
//! axis, so that moving a transform along that axis can be seen not to move
//! it on screen, and dragging it in that pane moves it in that plane exactly.
//!
//! Every pane draws the same point buffer: the chaos game runs once a frame,
//! and each pane costs one render pass (`App::render_frame` draws them one
//! after another, each with its own camera uploaded and a viewport set on
//! every pass). So a pane is a rectangle and a camera and nothing else — no
//! scene of its own, which is what the mutation grid's panes are.
//!
//! **The axis panes share the main camera's focus and scale.** Scroll in any
//! pane and all of them zoom; pan in an axis pane and the perspective pane
//! pans with it. There is one place the camera is, seen four ways, rather
//! than four cameras that drift apart and have to be brought back together.
//! An axis pane has no orbit — its direction is the whole point of it — so a
//! left-drag there pans. Which axis it looks down is chosen with the axis
//! cross in its corner: click a ball and the pane looks down that one.
//!
//! **An axis pane is a long lens, not an orthographic camera.** It looks
//! through [`AXIS_LENS_DEGREES`] from far enough back to show exactly what
//! the perspective pane shows at its focus. Across an attractor framed in the
//! pane, perspective changes a dot's size by under a percent front to back,
//! which is orthographic to the eye. A true orthographic projection would have
//! been wrong in every shader that reads `clip.w` as the eye's distance: point
//! size, haze, the splat footprint and the volume renderer's world-space
//! absorbance all divide by it, and under orthographic projection it is 1
//! everywhere. A long lens keeps every one of them right without touching a
//! shader, and picking, which is a ray from the eye, works as it always has.
//! Its near and far planes are its own, half its distance either side of the
//! focus, because at that distance the main camera's planes would spend
//! nearly all of the depth buffer's precision in front of the scene.

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::camera::{Fov, OrbitCamera};
use crate::rot::Orientation;

/// The axis panes' lens, in degrees. One degree puts the eye 115 frame
/// heights from the focus, where an attractor one frame deep changes a dot's
/// size by about 0.9% front to back.
pub const AXIS_LENS_DEGREES: f32 = 1.0;

/// How the viewport is divided. A preference, like panel geometry: it is how
/// the person looks at scenes, not part of any one scene.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// The one perspective view, as Fracturize has always been.
    #[default]
    Single,
    /// Two by two: the perspective view top-left, where the eye lands first
    /// (Hazel's call, 2026-10-31), top above front down the right, and right
    /// beside front along the bottom — so the top and front views still share
    /// their x and the right and front views their y.
    Quad,
    /// The perspective view large on the left, the three axis views stacked
    /// down the right.
    OneAndThree,
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::Single, Layout::Quad, Layout::OneAndThree];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Layout::Single => "One view",
            Layout::Quad => "Four views",
            Layout::OneAndThree => "One big, three small",
        }
    }
}

/// A direction an axis pane looks from: the camera sits on this side of the
/// focus and looks back along it, as clicking a ball on the axis cross does.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    #[serde(rename = "+x")]
    PosX,
    #[serde(rename = "-x")]
    NegX,
    #[serde(rename = "+y")]
    PosY,
    #[serde(rename = "-y")]
    NegY,
    #[serde(rename = "+z")]
    PosZ,
    #[serde(rename = "-z")]
    NegZ,
}

/// The axis panes' directions before anyone has picked one: top, front and
/// right. Front is +z because that is where the default camera stands.
pub const DEFAULT_AXES: [Axis; 3] = [Axis::PosY, Axis::PosZ, Axis::PosX];

impl Axis {
    pub const ALL: [Axis; 6] = [Axis::PosX, Axis::NegX, Axis::PosY, Axis::NegY, Axis::PosZ, Axis::NegZ];

    pub fn dir(self) -> Vec3 {
        match self {
            Axis::PosX => Vec3::X,
            Axis::NegX => -Vec3::X,
            Axis::PosY => Vec3::Y,
            Axis::NegY => -Vec3::Y,
            Axis::PosZ => Vec3::Z,
            Axis::NegZ => -Vec3::Z,
        }
    }

    /// What the view is called, from where the camera stands.
    pub fn name(self) -> &'static str {
        match self {
            Axis::PosX => "Right",
            Axis::NegX => "Left",
            Axis::PosY => "Top",
            Axis::NegY => "Bottom",
            Axis::PosZ => "Front",
            Axis::NegZ => "Back",
        }
    }

    /// The axis it looks from, signed. ASCII minus: U+2212 is in none of the
    /// app's fonts.
    pub fn signed(self) -> &'static str {
        match self {
            Axis::PosX => "+X",
            Axis::NegX => "-X",
            Axis::PosY => "+Y",
            Axis::NegY => "-Y",
            Axis::PosZ => "+Z",
            Axis::NegZ => "-Z",
        }
    }

    /// The axis nearest a direction.
    pub fn nearest(v: Vec3) -> Axis {
        *Self::ALL
            .iter()
            .max_by(|a, b| a.dir().dot(v).total_cmp(&b.dir().dot(v)))
            .expect("six of them")
    }
}

/// The framing that looks down `dir` at the focus.
///
/// World up is screen up, except looking along y, where it can't be. There
/// the screen's up is whichever z end puts +x on the right, so the top view
/// drawn above the front view lines up with it column for column, and the
/// bottom view likewise.
pub fn axis_orientation(dir: Vec3) -> Orientation {
    let z = dir.normalize();
    let up = if z.y > 0.99 {
        -Vec3::Z
    } else if z.y < -0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    Orientation::from_basis(x, y, z)
}

/// The camera an axis pane looks through, and its near and far planes.
///
/// Same focus as the main camera, and far enough back down `axis` that its
/// long lens shows the same height of world at the focus as the main camera
/// does. `frame` is the infinite zoom's carried orientation
/// (`App::zoom_frame`), so a pane keeps looking down the axis the world's own
/// axis cross shows as the zoom twists; identity otherwise.
pub fn axis_camera(main: &OrbitCamera, axis: Axis, frame: Orientation) -> (OrbitCamera, (f32, f32)) {
    let lens = Fov::axis_lens(AXIS_LENS_DEGREES);
    let half_height = main.distance * main.fov.tan_half();
    let distance = half_height / lens.tan_half();
    let o = axis_orientation(axis.dir());
    let orientation = Orientation::from_basis(
        frame.rotate(o.rotate(Vec3::X)),
        frame.rotate(o.rotate(Vec3::Y)),
        frame.rotate(o.rotate(Vec3::Z)),
    );
    let camera = OrbitCamera { orientation, distance, focus: main.focus, fov: lens };
    (camera, (distance * 0.5, distance * 1.5))
}

/// A pane's rectangle on the surface, in physical pixels: what a viewport and
/// a scissor take, and what picking measures the cursor against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x as f32 && p.1 >= self.y as f32 && p.0 < (self.x + self.w) as f32 && p.1 < (self.y + self.h) as f32
    }

    /// `p` measured from this rectangle's corner.
    pub fn local(&self, p: (f32, f32)) -> (f32, f32) {
        (p.0 - self.x as f32, p.1 - self.y as f32)
    }

    pub fn aspect(&self) -> f32 {
        self.w.max(1) as f32 / self.h.max(1) as f32
    }

    pub fn viewport(&self) -> crate::gpu::Viewport {
        crate::gpu::Viewport { x: self.x, y: self.y, w: self.w, h: self.h }
    }
}

/// What a pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Perspective,
    /// Axis pane `k` of three, looking down whichever axis it has been given.
    Axis(usize),
}

/// One pane, ready to draw or to pick in.
#[derive(Clone, Copy, Debug)]
pub struct Pane {
    pub rect: Rect,
    pub slot: Slot,
    pub camera: OrbitCamera,
    /// Near and far planes of its own, for an axis pane; `None` is the
    /// camera's own, which is what keeps the one-pane layout exactly the view
    /// it always was.
    pub planes: Option<(f32, f32)>,
}

impl Pane {
    pub fn view_proj(&self) -> Mat4 {
        match self.planes {
            None => self.camera.view_proj(self.rect.aspect()),
            Some((near, far)) => {
                Mat4::perspective_rh(self.camera.fov.radians(), self.rect.aspect(), near, far)
                    * self.camera.view_matrix()
            }
        }
    }

    pub fn is_axis(&self) -> bool {
        matches!(self.slot, Slot::Axis(_))
    }
}

/// Where each pane of `layout` sits in `area`, `gap` pixels apart. The
/// perspective pane comes first, always: it is the main view, the one the
/// xaos edges and the selection glow are drawn in.
pub fn rects(layout: Layout, area: Rect, gap: u32) -> Vec<(Rect, Slot)> {
    // Split `len` at `x` into two runs `gap` apart: the first `first` long.
    let split = |start: u32, len: u32, first: u32| -> ((u32, u32), (u32, u32)) {
        let first = first.min(len.saturating_sub(gap));
        ((start, first), (start + first + gap, len.saturating_sub(first + gap)))
    };
    let r = |(x, w): (u32, u32), (y, h): (u32, u32)| Rect { x, y, w, h };
    match layout {
        Layout::Single => vec![(area, Slot::Perspective)],
        Layout::Quad => {
            let half_w = area.w.saturating_sub(gap) / 2;
            let half_h = area.h.saturating_sub(gap) / 2;
            let (left, right) = split(area.x, area.w, half_w);
            let (top, bottom) = split(area.y, area.h, half_h);
            vec![
                (r(left, top), Slot::Perspective),
                (r(right, top), Slot::Axis(0)),
                (r(right, bottom), Slot::Axis(1)),
                (r(left, bottom), Slot::Axis(2)),
            ]
        }
        Layout::OneAndThree => {
            let big = (area.w.saturating_sub(gap) as f32 * (2.0 / 3.0)).round() as u32;
            let (left, right) = split(area.x, area.w, big);
            let third = area.h.saturating_sub(2 * gap) / 3;
            let (a, rest) = split(area.y, area.h, third);
            let (b, c) = split(rest.0, rest.1, third);
            vec![
                (r(left, (area.y, area.h)), Slot::Perspective),
                (r(right, a), Slot::Axis(0)),
                (r(right, b), Slot::Axis(1)),
                (r(right, c), Slot::Axis(2)),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_camera() -> OrbitCamera {
        OrbitCamera::from_chart(0.7, 0.3, 0.1, 3.0, Vec3::new(0.2, -0.1, 0.4))
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    /// Every layout tiles its area: inside it, no two panes overlapping, and
    /// the perspective pane first. Odd sizes included, where a careless split
    /// loses a pixel or runs one past the edge.
    #[test]
    fn panes_tile_the_area() {
        for (w, h) in [(1920, 1080), (1281, 721), (640, 359), (37, 29)] {
            let area = Rect { x: 0, y: 30, w, h };
            for layout in Layout::ALL {
                let panes = rects(layout, area, 2);
                assert_eq!(panes[0].1, Slot::Perspective, "{layout:?}");
                for (i, (a, _)) in panes.iter().enumerate() {
                    assert!(a.x >= area.x && a.y >= area.y, "{layout:?} {a:?}");
                    assert!(a.x + a.w <= area.x + area.w && a.y + a.h <= area.y + area.h, "{layout:?} {a:?} in {area:?}");
                    for (b, _) in &panes[i + 1..] {
                        assert!(!overlaps(a, b), "{layout:?}: {a:?} and {b:?}");
                    }
                }
            }
        }
    }

    /// The drafting layout: top above front, right beside front, the
    /// perspective view in the corner left over.
    #[test]
    fn the_quad_puts_perspective_top_left_and_keeps_the_views_aligned() {
        let p = rects(Layout::Quad, Rect { x: 0, y: 0, w: 1000, h: 800 }, 2);
        let at = |s: Slot| p.iter().find(|(_, k)| *k == s).unwrap().0;
        let (top, front, right, persp) = (at(Slot::Axis(0)), at(Slot::Axis(1)), at(Slot::Axis(2)), at(Slot::Perspective));
        assert_eq!((persp.x, persp.y), (0, 0));
        assert_eq!(top.x, front.x);
        assert!(top.y < front.y);
        assert_eq!(right.y, front.y);
        assert_eq!(right.x, persp.x);
    }

    #[test]
    fn layouts_cycle_both_ways() {
        for l in Layout::ALL {
            assert_eq!(l.next().prev(), l);
        }
        assert_eq!(Layout::Single.next(), Layout::Quad);
        assert_eq!(Layout::Single.prev(), Layout::OneAndThree);
    }

    /// An axis pane looks back down its axis at the main camera's focus, and
    /// the drafting views agree: top and front share +x to the right, front
    /// and right share +y up.
    #[test]
    fn an_axis_pane_looks_down_its_axis() {
        let main = main_camera();
        for axis in Axis::ALL {
            let (cam, _) = axis_camera(&main, axis, Orientation::IDENTITY);
            assert!((cam.forward() + axis.dir()).length() < 1e-5, "{axis:?}: {:?}", cam.forward());
            assert!((cam.focus - main.focus).length() < 1e-6);
            assert_eq!(Axis::nearest(-cam.forward()), axis);
        }
        let right = |a| axis_camera(&main, a, Orientation::IDENTITY).0.right();
        let up = |a| axis_camera(&main, a, Orientation::IDENTITY).0.up();
        assert!((right(Axis::PosY) - Vec3::X).length() < 1e-5);
        assert!((right(Axis::PosZ) - Vec3::X).length() < 1e-5);
        assert!((up(Axis::PosZ) - Vec3::Y).length() < 1e-5);
        assert!((up(Axis::PosX) - Vec3::Y).length() < 1e-5);
        // Bottom: +x stays on the right there too.
        assert!((right(Axis::NegY) - Vec3::X).length() < 1e-5);
    }

    /// The scale follows the main camera: at the focus a world unit spans as
    /// many pixels in an axis pane as in the perspective pane of the same
    /// height. That is what makes the panes show the same region.
    #[test]
    fn an_axis_pane_shows_what_the_main_camera_shows_at_the_focus() {
        let main = main_camera();
        let (cam, _) = axis_camera(&main, Axis::PosZ, Orientation::IDENTITY);
        let ppu = |c: &OrbitCamera| OrbitCamera::pixels_per_world_unit(c.distance, 1000.0, c.fov);
        assert!((ppu(&cam) / ppu(&main) - 1.0).abs() < 1e-4, "{} vs {}", ppu(&cam), ppu(&main));
    }

    /// Near enough orthographic: a point a whole frame-height in front of the
    /// focus and one the same distance behind land within a percent of each
    /// other's screen offset from the centre.
    #[test]
    fn an_axis_pane_is_orthographic_to_within_a_percent() {
        let main = main_camera();
        let (cam, planes) = axis_camera(&main, Axis::PosZ, Orientation::IDENTITY);
        let pane = Pane { rect: Rect { x: 0, y: 0, w: 800, h: 800 }, slot: Slot::Axis(0), camera: cam, planes: Some(planes) };
        let vp = pane.view_proj();
        let half = main.distance * main.fov.tan_half();
        let ndc = |p: Vec3| {
            let c = vp * p.extend(1.0);
            c.truncate() / c.w
        };
        let side = cam.right() * half * 0.8;
        let front = ndc(main.focus + side + cam.orientation.rotate(Vec3::Z) * 2.0 * half);
        let back = ndc(main.focus + side - cam.orientation.rotate(Vec3::Z) * 2.0 * half);
        assert!((front.x / back.x - 1.0).abs() < 0.04, "{front:?} {back:?}");
        assert!(front.z > 0.0 && front.z < 1.0 && back.z > 0.0 && back.z < 1.0, "inside the planes: {front:?} {back:?}");
    }

    #[test]
    fn axes_are_written_as_signed_letters() {
        #[derive(Serialize, Deserialize)]
        struct T {
            a: [Axis; 3],
            l: Layout,
        }
        let s = toml::to_string(&T { a: DEFAULT_AXES, l: Layout::OneAndThree }).unwrap();
        assert!(s.contains(r#"a = ["+y", "+z", "+x"]"#), "{s}");
        assert!(s.contains(r#"l = "one-and-three""#), "{s}");
        let back: T = toml::from_str(&s).unwrap();
        assert_eq!(back.a, DEFAULT_AXES);
    }
}
