//! CPU prototype of the glass renderer (GLASS-RENDERER-PLAN.md). Test-only:
//! it exists to look at the idea before any WGSL is written.
//!
//! Dots are flat discs carrying a fixed share of the material's absorbance,
//! `tau_point = sigma * L^2 / N` (world area), spread over the larger of the
//! disc's area and the pixel's — which is both "a big dot is faint per pixel"
//! and "a sub-pixel dot covers part of its pixel" in one rule. Then three
//! composites of the same fragments:
//!
//! * `exact`  every pixel's fragments sorted, front to back;
//! * `front`  the plan: the K nearest exact, the tail ranked by a linear
//!            guess between the K-th and the farthest depth, the total
//!            opacity exact from the summed absorbance;
//! * `trunc`  the K nearest only, then the background (the first idea, kept
//!            to show what the tail is for).
//!
//! `cargo test --release -j 3 glass_proto -- --ignored --nocapture`, with
//! GLASS_SCENE, GLASS_SIZE (WxH), GLASS_POINTS, GLASS_SIGMA, GLASS_K (comma
//! list) to taste. Writes renders/glass-proto/.

use glam::{Vec3, Vec4Swizzles};
use rand::{Rng, SeedableRng};

use crate::camera::{CameraOverride, OrbitCamera};
use crate::scene::Scene;

#[derive(Clone, Copy)]
struct Frag {
    z: f32,
    tau: f32,
    rgb: Vec3,
}

pub struct Setup {
    pub w: usize,
    pub h: usize,
    pub n: usize,
    pub sigma: f32,
}

fn env<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

/// The chaos-game points, coloured through the scene's colormap.
fn points(scene: &Scene, n: usize) -> Vec<(Vec3, Vec3)> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    let enabled = vec![true; scene.transforms.len()];
    let walkers = 64.min(n.max(1));
    let per = n.div_ceil(walkers);
    let mut out = Vec::with_capacity(n);
    let cm = &scene.colormap;
    for _ in 0..walkers {
        let mut w = crate::trace::Walker::new(&scene.transforms, &enabled, &mut rng).unwrap();
        for _ in 0..crate::trace::BURN_IN_STEPS {
            w.step(&mut rng);
        }
        for _ in 0..per {
            if out.len() == n {
                break;
            }
            w.step(&mut rng);
            let f = w.color_val.clamp(0.0, 1.0);
            let s = (0.5 + (f - 0.5) * scene.color_contrast).rem_euclid(1.0);
            let c = cm[((s * 256.0) as usize).min(255)];
            out.push((w.pos, Vec3::new(c[0], c[1], c[2])));
        }
    }
    let _ = rng.r#gen::<u8>();
    out
}

/// Every fragment every dot makes, per pixel.
fn rasterize(scene: &Scene, cam: &OrbitCamera, s: &Setup) -> Vec<Vec<Frag>> {
    let pts = points(scene, s.n);
    let stats = crate::trace::measure(&scene.transforms, &vec![true; scene.transforms.len()]);
    let l = stats.map(|st| st.radius).unwrap_or(1.0).max(1e-3);
    let tau_point = s.sigma * l * l / s.n as f32; // world area x optical depth
    let view = cam.view_matrix();
    let th = cam.fov.tan_half();
    let (wf, hf) = (s.w as f32, s.h as f32);
    let aspect = wf / hf;
    let radius_world = 0.5 * scene.point_size * cam.fov.point_scale();
    let mut px: Vec<Vec<Frag>> = vec![Vec::new(); s.w * s.h];
    for (p, rgb) in pts {
        let v = (view * p.extend(1.0)).xyz();
        let z = -v.z;
        if z <= cam.near_plane() {
            continue;
        }
        // Pixels per world unit at this depth, and the pixel's world area.
        let ppw = hf / (2.0 * z * th);
        let pix_area = 1.0 / (ppw * ppw);
        let cx = (v.x / (z * th * aspect) * 0.5 + 0.5) * wf;
        let cy = (0.5 - v.y / (z * th) * 0.5) * hf;
        let r_px = radius_world * ppw;
        let disc_area = std::f32::consts::PI * radius_world * radius_world;
        let tau_px = tau_point / disc_area.max(pix_area);
        if r_px < 0.5 {
            // Sub-pixel: one pixel, its absorbance already shrunk by area.
            let (ix, iy) = (cx.floor() as i64, cy.floor() as i64);
            if ix >= 0 && iy >= 0 && (ix as usize) < s.w && (iy as usize) < s.h {
                px[iy as usize * s.w + ix as usize].push(Frag { z, tau: tau_px, rgb });
            }
            continue;
        }
        // A disc with a one-pixel feathered edge.
        let x0 = (cx - r_px - 1.0).floor().max(0.0) as usize;
        let x1 = ((cx + r_px + 1.0).ceil() as i64).min(s.w as i64 - 1);
        let y0 = (cy - r_px - 1.0).floor().max(0.0) as usize;
        let y1 = ((cy + r_px + 1.0).ceil() as i64).min(s.h as i64 - 1);
        if x1 < 0 || y1 < 0 {
            continue;
        }
        for iy in y0..=y1 as usize {
            for ix in x0..=x1 as usize {
                let d = ((ix as f32 + 0.5 - cx).powi(2) + (iy as f32 + 0.5 - cy).powi(2)).sqrt();
                let cover = (r_px + 0.5 - d).clamp(0.0, 1.0);
                if cover > 0.0 {
                    px[iy * s.w + ix].push(Frag { z, tau: tau_px * cover, rgb });
                }
            }
        }
    }
    px
}

fn exact(frags: &[Frag], bg: Vec3) -> Vec3 {
    let mut f = frags.to_vec();
    f.sort_by(|a, b| a.z.total_cmp(&b.z));
    let mut t = 1.0;
    let mut c = Vec3::ZERO;
    for fr in &f {
        let a = 1.0 - (-fr.tau).exp();
        c += t * a * fr.rgb;
        t *= 1.0 - a;
    }
    c + t * bg
}

fn truncated(frags: &[Frag], bg: Vec3, k: usize) -> Vec3 {
    let mut f = frags.to_vec();
    f.sort_by(|a, b| a.z.total_cmp(&b.z));
    f.truncate(k);
    exact(&f, bg)
}

/// What the GPU will do: two passes over the fragments and a resolve.
fn front_k(frags: &[Frag], bg: Vec3, k: usize) -> Vec3 {
    if frags.is_empty() {
        return bg;
    }
    // Pass 1: the K nearest (depth, tau), the summed tau and the far depth.
    let mut list: Vec<(f32, f32)> = Vec::with_capacity(k);
    let mut tau_total = 0.0;
    let mut z_max = f32::MIN;
    for fr in frags {
        tau_total += fr.tau;
        z_max = z_max.max(fr.z);
        if list.len() < k {
            list.push((fr.z, fr.tau));
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
        } else if fr.z < list[k - 1].0 {
            list[k - 1] = (fr.z, fr.tau);
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }
    let tau_front_all: f32 = list.iter().map(|e| e.1).sum();
    let z_k = list.last().unwrap().0;
    let tail_tau = (tau_total - tau_front_all).max(0.0);
    // Pass 2: each fragment's weight from the absorbance in front of it.
    let mut sum_c = Vec3::ZERO;
    let mut sum_w = 0.0;
    for fr in frags {
        let tau_front = if fr.z <= z_k {
            list.iter().take_while(|e| e.0 < fr.z).map(|e| e.1).sum::<f32>()
        } else {
            let span = (z_max - z_k).max(1e-9);
            tau_front_all + tail_tau * ((fr.z - z_k) / span)
        };
        let w = (-tau_front).exp() * (1.0 - (-fr.tau).exp());
        sum_c += w * fr.rgb;
        sum_w += w;
    }
    // Resolve: the total opacity is exact; only its sharing out is a guess.
    let t_total = (-tau_total).exp();
    let c = if sum_w > 0.0 { sum_c / sum_w } else { Vec3::ZERO };
    c * (1.0 - t_total) + bg * t_total
}

fn to_srgb8(c: Vec3) -> [u8; 3] {
    let e = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        let s = if x <= 0.0031308 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    [e(c.x), e(c.y), e(c.z)]
}

/// Lay images side by side, each scaled up by `zoom` (nearest neighbour).
fn sheet(images: &[Vec<Vec3>], w: usize, h: usize, zoom: usize) -> image::RgbImage {
    let gap = 4;
    let mut out = image::RgbImage::from_pixel(
        (images.len() * (w * zoom + gap)) as u32,
        (h * zoom) as u32,
        image::Rgb([40, 40, 40]),
    );
    for (i, img) in images.iter().enumerate() {
        for y in 0..h * zoom {
            for x in 0..w * zoom {
                let c = to_srgb8(img[(y / zoom) * w + x / zoom]);
                out.put_pixel((i * (w * zoom + gap) + x) as u32, y as u32, image::Rgb(c));
            }
        }
    }
    out
}

fn rmse(a: &[Vec3], b: &[Vec3]) -> f32 {
    let s: f32 = a.iter().zip(b).map(|(x, y)| (*x - *y).length_squared()).sum();
    (s / (3.0 * a.len() as f32)).sqrt()
}

#[test]
#[ignore = "a prototype to look at; writes renders/glass-proto/"]
fn glass_proto() {
    let name: String = env("GLASS_SCENE", "blossom".to_string());
    let size: String = env("GLASS_SIZE", "32x32".to_string());
    let (w, h) = size.split_once('x').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap())).unwrap();
    let s = Setup { w, h, n: env("GLASS_POINTS", 64), sigma: env("GLASS_SIGMA", 2.0) };
    let ks: Vec<usize> = env("GLASS_K", "2,4,8".to_string())
        .split(',')
        .map(|k| k.parse().unwrap())
        .collect();
    let zoom = (512 / w.max(h)).max(1);

    let scene = Scene::load(format!("scenes/{name}.toml")).unwrap();
    let cam = crate::offline::effective_camera(None, &scene, CameraOverride::default());
    let bg = scene.background;
    let t = std::time::Instant::now();
    let px = rasterize(&scene, &cam, &s);
    let frags: usize = px.iter().map(Vec::len).sum();
    let busiest = px.iter().map(Vec::len).max().unwrap_or(0);
    println!(
        "{name} {w}x{h} N={} sigma={}: {frags} fragments, busiest pixel {busiest}, raster {:.2}s",
        s.n,
        s.sigma,
        t.elapsed().as_secs_f32()
    );

    let ex: Vec<Vec3> = px.iter().map(|f| exact(f, bg)).collect();
    let mut row = vec![ex.clone()];
    for &k in &ks {
        let fk: Vec<Vec3> = px.iter().map(|f| front_k(f, bg, k)).collect();
        let tr: Vec<Vec3> = px.iter().map(|f| truncated(f, bg, k)).collect();
        println!("  K={k:3}  front+tail rmse {:.4}   truncated rmse {:.4}", rmse(&fk, &ex), rmse(&tr, &ex));
        row.push(fk);
        row.push(tr);
    }
    std::fs::create_dir_all("renders/glass-proto").unwrap();
    let out = format!("renders/glass-proto/{name}-{w}x{h}-n{}-s{}.png", s.n, s.sigma);
    sheet(&row, w, h, zoom).save(&out).unwrap();
    println!("  wrote {out}  (exact, then per K: front+tail, truncated)  {:.2}s", t.elapsed().as_secs_f32());
}
