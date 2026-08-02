//! `--info`: say everything about a scene, in text, without rendering it
//!
//! Written for the agent side of this tool, and by one. Reading a scene
//! previously meant `cat`ting its TOML and doing arithmetic in your head:
//! weights are unnormalized so you can't see what fraction of the chaos game a
//! transform actually gets, `scale` is authored per-axis but contraction is
//! the cube root of the determinant, and whether a map could serve as a zoom
//! symmetry is four separate conditions. All of that is already computed
//! somewhere in the codebase for its own reasons; this prints it.
//!
//! It also reports what the scene *measures* — where the attractor lands and
//! how big it is, from the same CPU walkers `randomize.rs` gates on — which
//! the file cannot tell you at all, and which is the thing you want before
//! choosing a camera distance or a point size.

use glam::Vec3;

use crate::renorm::{Renorm, ZoomSpec};
use crate::scene::Scene;

/// Everything `--info` prints, as one string
pub fn report(scene: &Scene, source: &str) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };

    line(format!("{}  —  {}", scene.name, source));
    if !scene.author.is_empty() && scene.author != "Unknown" {
        line(format!("author: {}", scene.author));
    }
    line(String::new());

    // ---- what the chaos game will do -------------------------------------
    let enabled = vec![true; scene.transforms.len()];
    let total: f32 = scene.transforms.iter().map(|t| t.weight).sum();
    // Rotations are re-derived from the matrix, so a transform authored as
    // (-26, 138, 0) can come back as (154, 42, -180) — the same rotation on
    // the other euler branch. Say so rather than have someone diff it against
    // the file and conclude the loader is wrong.
    line(format!(
        "{} transforms (rotations re-derived from the matrix; euler branch may differ from the file)",
        scene.transforms.len()
    ));
    for (i, t) in scene.transforms.iter().enumerate() {
        let name = scene
            .transform_names
            .get(i)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("#{}", i));
        let (scale, rot, trans) = t.matrix.to_scale_rotation_translation();
        let (rx, ry, rz) = rot.to_euler(glam::EulerRot::XYZ);
        let share = if total > 0.0 { t.weight / total * 100.0 } else { 0.0 };
        line(format!(
            "  [{}] {:<14} {:>5.1}% of the walk   contraction {:.3}",
            i,
            name,
            share,
            t.contraction()
        ));
        line(format!(
            "       scale {}  rot ({:.0}, {:.0}, {:.0})°  translate ({:.3}, {:.3}, {:.3})",
            fmt_scale(scale),
            rx.to_degrees(),
            ry.to_degrees(),
            rz.to_degrees(),
            trans.x,
            trans.y,
            trans.z,
        ));
        let vars = t.variation_summary();
        if vars != "linear" {
            line(format!("       {}", vars));
        }
    }
    line(String::new());

    // ---- what it actually looks like -------------------------------------
    // Nothing above this line needed to run the fractal; this does, and it's
    // the part a TOML file can't answer.
    match crate::trace::measure(&scene.transforms, &enabled) {
        Some(s) => {
            line(format!(
                "measured: centre ({:.3}, {:.3}, {:.3}), radius {:.3} (95th pct), \
                 spread ({:.2}, {:.2}, {:.2}), occupancy {:.1}%",
                s.center.x,
                s.center.y,
                s.center.z,
                s.radius,
                s.spread.x,
                s.spread.y,
                s.spread.z,
                s.occupancy * 100.0
            ));
            // The two numbers most often wrong in a hand-authored scene, and
            // the reason they're wrong is that both depend on this measurement
            // rather than on anything visible in the file.
            let framed = (s.radius * 2.4).clamp(0.8, 12.0);
            line(format!(
                "  suggests: camera distance ~{:.2} (fills the frame), \
                 point_size ≤ {:.4} (stays on the crisp 1px path at 1080p)",
                framed,
                1.5 * framed / 1080.0
            ));
            if (scene.camera_distance / framed).max(framed / scene.camera_distance) > 2.0 {
                line(format!(
                    "  NOTE: the scene frames at distance {:.2}, {:.1}x that",
                    scene.camera_distance,
                    scene.camera_distance / framed
                ));
            }
        }
        None => line(
            "measured: the chaos game does not converge (all weights zero, or it diverges)"
                .to_string(),
        ),
    }
    line(String::new());

    // ---- render properties ------------------------------------------------
    line(format!(
        "point_size {}   point_count {}   haze {:.2}   background [{:.3}, {:.3}, {:.3}]",
        scene.point_size, scene.point_count, scene.haze,
        scene.background.x, scene.background.y, scene.background.z
    ));
    line(format!(
        "color: speed {:.2}, falloff {:.2}, contrast {:.2}",
        scene.color_speed, scene.color_falloff, scene.color_contrast
    ));
    line(format!(
        "camera: yaw {:.4} pitch {:.4} distance {:.3} focus ({:.3}, {:.3}, {:.3}){}",
        scene.camera_yaw,
        scene.camera_pitch,
        scene.camera_distance,
        scene.camera_focus.x,
        scene.camera_focus.y,
        scene.camera_focus.z,
        if scene.camera_roll == 0.0 {
            String::new()
        } else {
            format!(" roll {:.4}", scene.camera_roll)
        }
    ));
    match &scene.camera_path {
        Some(p) if p.keys.len() >= 2 => line(format!(
            "path: {} keypoints, {}, {:.1}s",
            p.keys.len(),
            if p.closed { "closed loop" } else { "open" },
            p.duration()
        )),
        _ => line("path: none authored (the default full-turn turntable applies)".to_string()),
    }
    line(String::new());

    // ---- infinite zoom ----------------------------------------------------
    // Eligibility is four conditions spread over two files; checking it by
    // eye is exactly the sort of thing this command exists to stop.
    line("infinite zoom:".to_string());
    match &scene.zoom {
        Some(spec) => match Renorm::build(spec, &scene.transforms, scene.camera_distance) {
            Ok(r) => line(format!(
                "  ON — {}",
                r.summary(scene.transform_names.get(r.map).cloned().flatten().as_deref())
            )),
            Err(e) => line(format!("  BROKEN — {}", e)),
        },
        None => line("  off; eligible maps:".to_string()),
    }
    if scene.zoom.is_none() {
        let mut any = false;
        for i in 0..scene.transforms.len() {
            let name = scene
                .transform_names
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_else(|| i.to_string());
            match Renorm::build(&ZoomSpec { map: i, ..Default::default() }, &scene.transforms, scene.camera_distance)
            {
                Ok(r) => {
                    any = true;
                    line(format!(
                        "    --zoom {:<12} {:.2} octaves/period, {:.0}° twist, fixed point \
                         ({:.3}, {:.3}, {:.3}){}",
                        name,
                        r.log_scale / std::f32::consts::LN_2,
                        r.angle.to_degrees(),
                        r.fixed_point.x,
                        r.fixed_point.y,
                        r.fixed_point.z,
                        match (r.defect > 0.02, r.band_covers_the_view()) {
                            (true, _) => "  [not a similarity — seam]",
                            (_, false) => "  [band too short — see renorm::MIN_RADIUS]",
                            _ => "",
                        },
                    ));
                }
                Err(e) => line(format!("    [{}] {:<12} no: {}", i, name, strip_prefix(&e))),
            }
        }
        if !any {
            line("    (none — infinite zoom needs a pure affine map that contracts on all three axes)".to_string());
        }
    }

    out
}

/// `zoom map 3 uses variations (...)` reads badly in a per-map list that has
/// already said which map it's talking about
fn strip_prefix(e: &str) -> String {
    match e.split_once("; ") {
        Some((head, _)) => head
            .split_once(" uses ")
            .map(|(_, rest)| format!("uses {}", rest))
            .unwrap_or_else(|| head.to_string()),
        None => e.to_string(),
    }
}

fn fmt_scale(s: Vec3) -> String {
    if (s.x - s.y).abs() < 1e-4 && (s.x - s.z).abs() < 1e-4 {
        format!("{:.3}", s.x)
    } else {
        format!("[{:.3}, {:.3}, {:.3}]", s.x, s.y, s.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_every_transform_and_the_zoom_verdict() {
        let scene = Scene::blank();
        let text = report(&scene, "blank");
        assert!(text.contains("2 transforms"));
        // blank()'s two maps are plain half-scales: both eligible
        assert!(text.contains("--zoom 0"), "{}", text);
        assert!(text.contains("--zoom 1"), "{}", text);
        // and the measurement ran
        assert!(text.contains("measured: centre"), "{}", text);
    }

    #[test]
    fn weights_are_reported_as_shares_of_the_walk() {
        let mut scene = Scene::blank();
        scene.transforms[0].weight = 3.0;
        scene.transforms[1].weight = 1.0;
        let text = report(&scene, "test");
        assert!(text.contains("75.0%"), "{}", text);
        assert!(text.contains("25.0%"), "{}", text);
    }
}
