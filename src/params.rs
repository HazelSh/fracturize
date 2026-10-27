//! What in a scene can be set, named for people.
//!
//! `-S` paths are the address language for agents and the command line
//! (`transform.whorl.rotation.y`); nobody at the GUI should have to know them.
//! This module lists a scene's settable parameters as a tree a person can
//! browse — *Transforms › whorl › Rotation › y* — each leaf carrying the path
//! that sets it, the value it has now, and the range it may take. The
//! Timeline's "animate a parameter" uses it (through `ui::param_picker`), and
//! anything else that needs to point at "a value in this scene" can.
//!
//! **Every path listed is checked against the scene itself**, by applying it
//! to the scene's own text the way `-S` would. So the tree never offers a
//! rotation the scene writes as a `rotvec`, a palette field on a scene with no
//! palette, or a map a subscene borrows from another file — the same
//! no-silent-no-ops rule `set.rs` keeps. And a value the file leaves out (a
//! variation a map doesn't use yet, an xaos edge left at its default) is
//! listed with the value it implies.
//!
//! [`range_of`] is the other half: the values a path may take. Tracks are
//! clamped to it every frame (`track::Animator`), because a spline overshoots
//! and a cycle swings past zero, and the loader refuses some of what comes out
//! (a negative xaos multiplier, a subscene scaled through zero) and quietly
//! draws nonsense from more (a negative weight or point size).

use crate::scene::Scene;

/// One settable value.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    /// The `-S` path
    pub path: String,
    /// Where it sits in the tree, outermost first: `["Transforms", "whorl",
    /// "Rotation"]`
    pub group: Vec<String>,
    /// The leaf's own name: `"y"`, `"Haze"`
    pub label: String,
    /// What the scene says now, as it would be saved (or what it implies)
    pub value: toml::Value,
}

impl Param {
    /// The whole name, for tooltips and for filtering.
    pub fn breadcrumb(&self) -> String {
        let mut parts = self.group.clone();
        parts.push(self.label.clone());
        parts.join(" › ")
    }

    /// Whether every word of `filter` appears in the name or the path,
    /// ignoring case. Empty matches everything.
    pub fn matches(&self, filter: &str) -> bool {
        let hay = format!("{} {}", self.breadcrumb(), self.path).to_lowercase();
        filter.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
    }

    pub fn is_number(&self) -> bool {
        self.value.as_float().is_some() || self.value.as_integer().is_some()
    }
}

/// The values `path` may take, inclusive, where something refuses or breaks
/// outside them. `None` for a value any number suits.
pub fn range_of(path: &str) -> Option<(f64, f64)> {
    let segs: Vec<&str> = path.split('.').collect();
    let at = |i: usize| segs.get(i).copied().unwrap_or("");
    const ANY_POSITIVE: f64 = f64::MAX;
    match (at(0), at(1)) {
        ("meta", "haze" | "fog") => Some((0.0, 1.0)),
        ("meta", "exposure") => Some((0.01, 100.0)),
        ("meta", "point_size") => Some((1e-6, ANY_POSITIVE)),
        ("meta", "background") => Some((0.0, 1.0)),
        ("meta", "color_speed") => Some((0.0, 1.0)),
        ("meta", "color_falloff") => Some((0.0, 4.0)),
        ("meta", "color_contrast") => Some((0.0, 16.0)),
        ("meta", "share") => Some((0.0, ANY_POSITIVE)),
        ("zoom", "radius") => Some((0.1, ANY_POSITIVE)),
        ("zoom", "levels") => Some((1.0, 64.0)),
        ("zoom", "edge_guard" | "octave_falloff") => Some((0.0, ANY_POSITIVE)),
        ("transform", _) => transform_range(&segs[2.min(segs.len())..]),
        ("subscene", _) => match at(2) {
            "share" => Some((0.0, ANY_POSITIVE)),
            "scale" => Some((1e-4, ANY_POSITIVE)),
            "color_speed" => Some((0.0, 1.0)),
            "color_falloff" => Some((0.0, 4.0)),
            "color_contrast" => Some((0.0, 16.0)),
            "transform" => transform_range(&segs[4.min(segs.len())..]),
            _ => None,
        },
        _ => None,
    }
}

/// A map's fields, from the field name on.
fn transform_range(field: &[&str]) -> Option<(f64, f64)> {
    match field.first().copied().unwrap_or("") {
        "weight" | "xaos" => Some((0.0, f64::MAX)),
        "xaos_p" => Some((0.0, 1.0)),
        "color" | "color_value" | "color_speed" => Some((0.0, 1.0)),
        _ => None,
    }
}

/// `v` brought into `path`'s range, if it has one.
pub fn clamp(path: &str, v: f64) -> f64 {
    match range_of(path) {
        Some((lo, hi)) => v.clamp(lo, hi),
        None => v,
    }
}

/// Every parameter of `scene` a person might set, in the order the tree shows
/// them. Maps past `MAX_MAPS` in a layer are left out: a 40,000-map L-system
/// has nothing to gain from a 40,000-branch menu.
pub fn catalogue(scene: &Scene) -> Vec<Param> {
    const MAX_MAPS: usize = 64;
    let Ok(text) = scene.to_toml_string() else { return Vec::new() };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return Vec::new() };
    let mut out = Vec::new();
    // Try the path on the scene's own text: listed only if `-S` takes it, with
    // the value it displaced or, where the file says nothing, `implied`.
    let mut offer = |group: &[&str], label: &str, path: String, implied: Option<toml::Value>| {
        // A whole rotation is the one array offered: a track keys it as poses.
        let whole = crate::track::takes_rotations(&path);
        let probe = if whole { "[0.0, 0.0, 0.0]" } else { "0.0" };
        let Ok(applied) = crate::set::apply_to_doc(&mut doc.clone(), &format!("{path}={probe}")) else {
            return;
        };
        let value = applied
            .from
            .and_then(|f| toml::from_str::<toml::Table>(&format!("v = {f}")).ok())
            .and_then(|t| t.get("v").cloned())
            .or(implied);
        // Arrays and tables are reached through their elements.
        let Some(value) = value.filter(|v| match v {
            toml::Value::Array(a) => whole && a.len() == 3,
            toml::Value::Table(_) => false,
            _ => !whole,
        })
        else {
            return;
        };
        out.push(Param {
            path,
            group: group.iter().map(|s| s.to_string()).collect(),
            label: label.to_string(),
            value,
        });
    };
    let f = |x: f64| Some(toml::Value::Float(x));

    // The look of the whole picture.
    for (key, label) in [
        ("point_size", "Point size"),
        ("exposure", "Exposure"),
        ("haze", "Haze"),
        ("color_speed", "Colour speed"),
        ("color_falloff", "Colour falloff"),
        ("color_contrast", "Colour contrast"),
    ] {
        offer(&["Scene"], label, format!("meta.{key}"), None);
    }
    for (axis, label) in [("r", "red"), ("g", "green"), ("b", "blue")] {
        offer(&["Scene", "Background"], label, format!("meta.background.{axis}"), None);
    }
    if scene.is_composite() {
        offer(&["Scene"], "Share of the points", "meta.share".into(), f(1.0));
    }

    if scene.palette.is_some() {
        offer(&["Palette"], "Rotate", "palette.rotate".into(), f(0.0));
        offer(&["Palette"], "Reverse", "palette.reverse".into(), Some(toml::Value::Boolean(false)));
    }

    if scene.zoom.is_some() {
        for (key, label) in [
            ("radius", "Radius"),
            ("levels", "Levels"),
            ("edge_guard", "Edge guard"),
            ("octave_falloff", "Octave falloff"),
        ] {
            offer(&["Infinite zoom"], label, format!("zoom.{key}"), None);
        }
    }

    // The scene's own maps.
    let (maps, names, _) = scene.layer_maps(0);
    let refs: Vec<String> =
        names.iter().enumerate().map(|(i, n)| n.clone().unwrap_or_else(|| i.to_string())).collect();
    let shown: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| n.clone().unwrap_or_else(|| format!("map {i}")))
        .collect();
    for (pos, r) in refs.iter().enumerate().take(MAX_MAPS) {
        map_params(&mut offer, &["Transforms", &shown[pos]], &format!("transform.{r}"), &refs, &shown, &maps[pos]);
    }

    // Subscenes: placement and share; an inline one's maps too.
    for (k, sub) in scene.subscenes.iter().enumerate() {
        let name = sub.name.clone();
        let base = format!("subscene.{name}");
        let top = ["Subscenes", name.as_str()];
        offer(&top, "Share of the points", format!("{base}.share"), f(1.0));
        offer(&top, "Visible", format!("{base}.visible"), Some(toml::Value::Boolean(true)));
        offer(&top, "Scale", format!("{base}.scale"), f(1.0));
        let zero = toml::Value::Array(vec![toml::Value::Float(0.0); 3]);
        offer(&["Subscenes", &name, "Rotation"], "whole (poses)", format!("{base}.rotation"), Some(zero));
        for (axis, l) in [("x", "x"), ("y", "y"), ("z", "z")] {
            offer(&["Subscenes", &name, "Position"], l, format!("{base}.translation.{axis}"), f(0.0));
            offer(&["Subscenes", &name, "Rotation"], l, format!("{base}.rotation.{axis}"), f(0.0));
        }
        if sub.file.is_none() {
            let (maps, names, _) = scene.layer_maps(k + 1);
            let refs: Vec<String> =
                names.iter().enumerate().map(|(i, n)| n.clone().unwrap_or_else(|| i.to_string())).collect();
            let shown: Vec<String> = names
                .iter()
                .enumerate()
                .map(|(i, n)| n.clone().unwrap_or_else(|| format!("map {i}")))
                .collect();
            for (pos, r) in refs.iter().enumerate().take(MAX_MAPS) {
                map_params(
                    &mut offer,
                    &["Subscenes", &name, &shown[pos]],
                    &format!("{base}.transform.{r}"),
                    &refs,
                    &shown,
                    &maps[pos],
                );
            }
        }
    }
    out
}

/// One map's parameters under `group`, its path starting `base`.
fn map_params(
    offer: &mut impl FnMut(&[&str], &str, String, Option<toml::Value>),
    group: &[&str],
    base: &str,
    refs: &[String],
    shown: &[String],
    spec: &crate::scene::TransformSpec,
) {
    let f = |x: f64| Some(toml::Value::Float(x));
    fn sub<'a>(group: &[&'a str], extra: &'a str) -> Vec<&'a str> {
        let mut g: Vec<&str> = group.to_vec();
        g.push(extra);
        g
    }
    offer(group, "Weight", format!("{base}.weight"), f(1.0));
    offer(group, "Colour index", format!("{base}.color_value"), None);
    offer(group, "Colour speed", format!("{base}.color_speed"), None);
    for (key, label, implied) in [
        ("translation", "Position", 0.0),
        ("rotation", "Rotation", 0.0),
        ("scale", "Scale", 1.0),
        ("post_translation", "Post position", 0.0),
        ("post_rotation", "Post rotation", 0.0),
        ("post_scale", "Post scale", 1.0),
    ] {
        if key == "rotation" {
            // The whole pose first: keyed as rotations, it turns smoothly the
            // way round each key says, which one angle of three can't.
            let zero = toml::Value::Array(vec![toml::Value::Float(0.0); 3]);
            offer(&sub(group, label), "whole (poses)", format!("{base}.rotation"), Some(zero));
        }
        for axis in ["x", "y", "z"] {
            offer(&sub(group, label), axis, format!("{base}.{key}.{axis}"), f(implied));
        }
    }
    for (axis, label) in [("r", "red"), ("g", "green"), ("b", "blue")] {
        offer(&sub(group, "Colour"), label, format!("{base}.color.{axis}"), None);
    }
    // What the map blends now, which is what a file that leaves the table out
    // means too (all `linear`).
    for (v, w) in crate::scene::VARIATION_NAMES.iter().zip(spec.variations) {
        offer(&sub(group, "Variations"), v, format!("{base}.variations.{v}"), f(w as f64));
    }
    for (r, s) in refs.iter().zip(shown) {
        offer(&sub(group, "Xaos: odds of going to"), s, format!("{base}.xaos.{r}"), f(1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_scene() -> Vec<(std::path::PathBuf, Scene)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes");
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .filter_map(|p| Scene::load(&p).ok().map(|s| (p, s)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Everything the tree offers has to be something a track can start on:
    /// a number, string or boolean `-S` takes on this very scene. A leaf that
    /// failed would be a menu item that does nothing.
    #[test]
    fn every_offered_parameter_can_be_animated() {
        let mut checked = 0;
        for (path, scene) in every_scene() {
            let cat = catalogue(&scene);
            assert!(!cat.is_empty(), "{}: nothing offered", path.display());
            let mut scene = scene;
            for p in cat.iter().take(40) {
                scene.tracks = vec![crate::track::one_key(&p.path, 0.0, p.value.clone())
                    .unwrap_or_else(|e| panic!("{}: {}: {e}", path.display(), p.path))];
                let anim = crate::track::Animator::for_scene(&scene).unwrap().unwrap();
                assert!(anim.broken.is_empty(), "{}: {}: {:?}", path.display(), p.path, anim.broken);
                anim.at(0.0).unwrap_or_else(|e| panic!("{}: {}: {e}", path.display(), p.path));
                checked += 1;
            }
        }
        assert!(checked > 200, "only {checked}");
    }

    #[test]
    fn the_tree_names_things_for_people() {
        let (_, scene) = every_scene().into_iter().find(|(p, _)| p.ends_with("parasol.toml")).unwrap();
        let cat = catalogue(&scene);
        let rot = cat.iter().find(|p| p.path == "transform.petal.rotation.y").expect("listed");
        assert_eq!(rot.breadcrumb(), "Transforms › petal › Rotation › y");
        assert_eq!(rot.value.as_float(), Some(12.0));
        let x = cat.iter().find(|p| p.path == "transform.spine.xaos.petal").expect("listed");
        assert_eq!(x.value.as_float().or(x.value.as_integer().map(|i| i as f64)), Some(0.0));
        // A variation the map doesn't use yet starts from nothing.
        let v = cat.iter().find(|p| p.path == "transform.petal.variations.swirl").expect("listed");
        assert_eq!(v.value.as_float(), Some(0.0));
        assert!(rot.matches("petal rot"));
        assert!(!rot.matches("spine"));
    }

    #[test]
    fn ranges_cover_what_the_loader_refuses() {
        assert_eq!(clamp("transform.spine.xaos.petal", -0.2), 0.0);
        assert_eq!(clamp("subscene.smoke.scale", -1.0), 1e-4);
        assert_eq!(clamp("subscene.smoke.share", -1.0), 0.0);
        assert_eq!(clamp("meta.haze", 1.3), 1.0);
        assert_eq!(clamp("transform.a.rotation.y", 400.0), 400.0);
    }
}
