//! `--set <path>=<value>` — dotted-path overrides applied to a scene's TOML
//! before it is parsed.
//!
//! This is the general form of what `--palette` and `--zoom` already do for two
//! subsystems: restyle a scene without editing it. It exists mainly so that
//! *sweeping a parameter* is a shell loop rather than a code-generation task —
//! before it, varying one number across five renders meant writing five TOML
//! files from a script, which is the single most expensive thing about
//! exploring a scene from the CLI (see CRAFT.md §8).
//!
//!     --set meta.color_falloff=0.3
//!     --set transform.facet-1.variations.absfold=0.15
//!     --set transform.2.weight=0.5
//!     --set zoom.edge_guard=1.5
//!
//! Because it rewrites the TOML text and hands the result to the ordinary
//! loader, everything downstream — `--info`, grids, animation, the mutation
//! sheet — sees a normal scene and needs no knowledge of this module.
//!
//! **A path that doesn't resolve is a hard error, never a silent no-op.** A
//! typo'd sweep that quietly rendered nine identical tiles would cost far more
//! than it saved, and it is exactly the failure mode that makes a fold applied
//! inside its own no-op region so expensive to notice.

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

/// One override, and the value it displaced.
///
/// Recorded so `--info` can say the command line moved something. A report
/// that shows `haze 0.90` without mentioning that the file says `0.50` answers
/// "what is this scene?" but not "did my edit land?", and the second question
/// is half of what the command is for.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    /// The dotted path, exactly as written on the command line
    pub path: String,
    /// The value that was there before, as TOML text. `None` when the key was
    /// not set at all — a `variations` table invented on the spot, mostly.
    pub from: Option<String>,
    /// The value now in its place, as TOML text
    pub to: String,
}

/// Apply `path=value` overrides to a scene's TOML source, returning the
/// rewritten source and what each override displaced. Formatting and comments
/// are preserved (toml_edit only rewrites the values that are touched).
///
/// The old value is read at the moment it is overwritten rather than diffed
/// afterwards, so two `--set`s on the same path report the chain honestly:
/// the second says it replaced the first, not the file.
pub fn apply_recording(
    src: &str,
    overrides: &[String],
) -> Result<(String, Vec<Applied>), String> {
    if overrides.is_empty() {
        return Ok((src.to_string(), Vec::new()));
    }
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e| format!("scene is not valid TOML: {e}"))?;
    let mut applied = Vec::with_capacity(overrides.len());
    for spec in overrides {
        applied.push(apply_one(&mut doc, spec)?);
    }
    Ok((doc.to_string(), applied))
}

/// A TOML value as the shortest text that means it — `0.5`, `"ember"`,
/// `[0.0, 0.5, 0.0]`. `Value`'s own `Display` carries the decor it was parsed
/// with, including the leading space in `key = value`, so it is trimmed.
fn shown(v: &Value) -> String {
    v.to_string().trim().to_string()
}

fn apply_one(doc: &mut DocumentMut, spec: &str) -> Result<Applied, String> {
    let (path, raw) = spec
        .split_once('=')
        .ok_or_else(|| format!("--set '{spec}': expected <path>=<value>"))?;
    let path = path.trim();
    validate_path(path).map_err(|e| format!("--set {e}"))?;
    let value = parse_value(raw.trim());
    let to = shown(&value);
    let record = |from: Option<String>| Applied { path: path.to_string(), from, to: to.clone() };

    // validate_path already confirmed segs.len() >= 2 and that section is one
    // of the five recognised ones, so the splits below can't fail.
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    let section = segs[0];
    let rest = &segs[1..];

    if section == "transform" {
        return set_in_transform(doc, rest, value, path).map(record);
    }

    let table = doc
        .get_mut(section)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            format!("--set '{path}': this scene has no [{section}] table to set into")
        })?;
    set_path(&mut TableRef::Table(table), rest, value, path).map(record)
}

/// Derive a struct's real field list, and *prove* it's the real one.
///
/// The `$ty { $($field),+ }` pattern below carries no `..`, and Rust's struct
/// patterns are exhaustive: that pattern fails to compile unless every field
/// of `$ty` is named in it. So the moment `scene.rs` or `palette/spec.rs`
/// gains a field on one of these structs and this macro's invocation isn't
/// updated to match, `cargo check` fails right here — "pattern does not
/// mention field `whatever`" — instead of the new field silently becoming a
/// `--set`/`--sweep` path nothing validates. That failure mode is exactly
/// what `validate_path` exists to close off in the first place; this closes
/// it one level up, with the compiler as the checker instead of a human
/// re-reading `scene.rs` on every change.
///
/// The generated function is never called — it exists purely to make the
/// compiler type-check the pattern — hence `#[allow(dead_code,
/// unused_variables)]` on it.
///
/// What this cannot see: `#[serde(alias = "...")]`. An alias is a second
/// TOML key accepted for an *existing* field, not a field of its own, so it
/// never appears in a destructuring pattern and there is nothing here for
/// the compiler to check it against. Where a struct has one (only
/// `SceneMeta` does, today), its derived list is unioned with a
/// hand-maintained alias list at the call site — see `META_FIELD_ALIASES`
/// below. That union is the one part of this mechanism that can still
/// silently drift; forgetting to update it only makes a legal old-style key
/// report as unknown, which is a far smaller failure than the one this file
/// guards against (a bad key being silently accepted).
macro_rules! struct_fields {
    ($assert_fn:ident, $ty:path, $($field:ident),+ $(,)?) => {{
        #[allow(dead_code, unused_variables)]
        fn $assert_fn(v: $ty) {
            let $ty { $($field),+ } = v;
        }
        const FIELDS: &[&str] = &[$(stringify!($field)),+];
        FIELDS
    }};
}

/// The section and field names a scene file actually understands. None of
/// `scene.rs`'s TOML structs use `deny_unknown_fields` — an unrecognised key
/// is not a parse error anywhere else in the program, so a misspelled
/// `--set`/`--sweep` path (`meta.wieght`, `camera.distanc`) would otherwise
/// add a key nothing reads and silently do nothing. This list is what turns
/// that into a hard error.
///
/// Each list below is derived from its struct by `struct_fields!` rather
/// than copied out by hand, so it cannot go stale: an added field either
/// gets listed here or the crate stops compiling. Variation names get the
/// same treatment a different way: `scene::VARIATION_NAMES` already *is* the
/// closed vocabulary `parse_variations` enforces at load time, so there is
/// nothing to derive — it's used directly.
const META_FIELDS: &[&str] = struct_fields!(
    _assert_scene_meta_exhaustive,
    crate::scene::SceneMeta,
    name,
    author,
    point_size,
    points_per_frame,
    decay,
    color_speed,
    color_falloff,
    color_contrast,
    color_mode,
    background,
    haze,
    exposure,
    point_count,
);
/// `#[serde(alias = ...)]` names on `SceneMeta`: real, accepted `[meta]` keys
/// that have no field of their own (`iters` for `points_per_frame`, `fog`
/// for `haze`), so `struct_fields!` cannot derive them — see the comment on
/// the macro above. Hand-maintained; when adding a field to `SceneMeta`,
/// check whether it comes with an alias, and if so add it here too.
const META_FIELD_ALIASES: &[&str] = &["iters", "fog"];
const CAMERA_FIELDS: &[&str] = struct_fields!(
    _assert_camera_def_exhaustive,
    crate::scene::CameraDef,
    focus,
    offset,
    distance,
    yaw,
    pitch,
    roll,
    rotvec,
    path_loop,
    path_zoom_periods,
    path_seconds,
    path_ease,
    path_closed,
    path_zoom_loop,
    path,
);
const ZOOM_FIELDS: &[&str] = struct_fields!(
    _assert_zoom_def_exhaustive,
    crate::scene::ZoomDef,
    map,
    radius,
    levels,
    octave_falloff,
    edge_guard,
    octave_fade,
);
const PALETTE_FIELDS: &[&str] = struct_fields!(
    _assert_palette_def_exhaustive,
    crate::palette::spec::PaletteDef,
    name,
    enabled,
    cyclic,
    interpolate,
    rotate,
    reverse,
    stops,
    cosine,
    entries,
);
const TRANSFORM_FIELDS: &[&str] = struct_fields!(
    _assert_transform_def_exhaustive,
    crate::scene::TransformDef,
    name,
    translation,
    scale,
    rotation,
    rotvec,
    color,
    weight,
    color_value,
    color_speed,
    variations,
    post_translation,
    post_scale,
    post_rotation,
);

/// Check that `path` names a real, settable parameter — independent of any
/// particular scene file, so `--sweep` can call this before a scene is even
/// chosen. What still needs the document (does this transform exist, is a
/// key already present) stays in `apply_one`/`set_in_transform`; this only
/// rules out the case that's never legitimate: a section, field, or
/// variation name that isn't a parameter at all.
pub fn validate_path(path: &str) -> Result<(), String> {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return Err(format!(
            "'{path}': path needs a section, e.g. meta.haze, camera.distance, \
             zoom.radius, palette.rotate, transform.<name-or-index>.weight"
        ));
    }
    let section = segs[0];
    if section == "transform" {
        if segs.len() < 3 {
            return Err(format!(
                "'{path}': expected a field after the transform, e.g. transform.{}.weight",
                segs[1]
            ));
        }
        check_field("transform field", &[TRANSFORM_FIELDS], segs[2], path)?;
        if segs[2] == "variations" {
            if let Some(&name) = segs.get(3) {
                let variation_names: &[&str] = &crate::scene::VARIATION_NAMES;
                check_field("variation", &[variation_names], name, path)?;
            }
        }
        return Ok(());
    }
    // A slice of lists rather than one flat list: only `meta` needs a second
    // source (its serde aliases, which `struct_fields!` cannot derive — see
    // `META_FIELD_ALIASES`), but every section is checked the same way so
    // that fact doesn't leak into the rest of this function.
    let known: &[&[&str]] = match section {
        "meta" => &[META_FIELDS, META_FIELD_ALIASES],
        "camera" => &[CAMERA_FIELDS],
        "zoom" => &[ZOOM_FIELDS],
        "palette" => &[PALETTE_FIELDS],
        _ => {
            return Err(format!(
                "'{path}': unknown section '{section}' (expected meta, camera, \
                 zoom, palette or transform)"
            ))
        }
    };
    check_field(&format!("{section} field"), known, segs[1], path)
}

fn check_field(kind: &str, known: &[&[&str]], key: &str, path: &str) -> Result<(), String> {
    if known.iter().any(|list| list.contains(&key)) {
        return Ok(());
    }
    let all: Vec<&str> = known.iter().flat_map(|list| list.iter().copied()).collect();
    match closest(key, &all) {
        Some(guess) => {
            Err(format!("'{path}': '{key}' is not a known {kind} (did you mean '{guess}'?)"))
        }
        None => Err(format!(
            "'{path}': '{key}' is not a known {kind}. Valid: {}",
            all.join(", ")
        )),
    }
}

/// The closest of `candidates` to `word`, if it's close enough to be worth
/// guessing — within an edit distance of 2, which catches a transposition or
/// a couple of typos (`wieght` -> `weight`) without matching two names that
/// just happen to share a few letters.
fn closest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|&c| (c, edit_distance(word, c)))
        .min_by_key(|&(_, d)| d)
        .filter(|&(_, d)| d <= 2)
        .map(|(c, _)| c)
}

/// Levenshtein distance. Field and variation names are short (well under 20
/// characters), so the plain O(n*m) table is cheap enough to run on every
/// `--set`/`--sweep` path with no need to memoize.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let tmp = row[j];
            row[j] = if a[i - 1] == b[j - 1] { prev } else { 1 + prev.min(row[j]).min(row[j - 1]) };
            prev = tmp;
        }
    }
    row[b.len()]
}

/// Resolve `transform.<name-or-index>.<rest>` against the `[[transform]]` array.
fn set_in_transform(
    doc: &mut DocumentMut,
    segs: &[&str],
    value: Value,
    path: &str,
) -> Result<Option<String>, String> {
    let (selector, rest) = segs
        .split_first()
        .ok_or_else(|| format!("--set '{path}': expected transform.<name-or-index>.<field>"))?;
    if rest.is_empty() {
        return Err(format!(
            "--set '{path}': expected a field after the transform, e.g. \
             transform.{selector}.weight"
        ));
    }

    let arr = doc
        .get_mut("transform")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| format!("--set '{path}': this scene has no [[transform]] entries"))?;

    // Index first, so a transform *named* "2" can still be reached by name.
    let idx = match selector.parse::<usize>() {
        Ok(i) => {
            if i >= arr.len() {
                return Err(format!(
                    "--set '{path}': transform index {i} is out of range (scene has {})",
                    arr.len()
                ));
            }
            i
        }
        Err(_) => {
            let found = arr.iter().position(|t| {
                t.get("name").and_then(|i| i.as_str()) == Some(*selector)
            });
            found.ok_or_else(|| {
                let names: Vec<String> = arr
                    .iter()
                    .enumerate()
                    .map(|(i, t)| match t.get("name").and_then(|v| v.as_str()) {
                        Some(n) => format!("{i}:{n}"),
                        None => format!("{i}:<unnamed>"),
                    })
                    .collect();
                format!(
                    "--set '{path}': no transform named '{selector}'. This scene has {}",
                    names.join(", ")
                )
            })?
        }
    };

    let table = arr.get_mut(idx).expect("index checked above");
    set_path(&mut TableRef::Table(table), rest, value, path)
}

/// A container we can descend through: either a real `[table]` or an inline
/// one (`variations = { ... }`). Arrays are handled at the leaf.
enum TableRef<'a> {
    Table(&'a mut Table),
    Inline(&'a mut InlineTable),
}

impl TableRef<'_> {
    fn get_value(&self, key: &str) -> Option<&Value> {
        match self {
            TableRef::Table(t) => t.get(key).and_then(Item::as_value),
            TableRef::Inline(t) => t.get(key),
        }
    }

    /// Replace a key, keeping the old value's decor (a trailing `# comment`),
    /// and hand back what was there.
    fn put(&mut self, key: &str, mut new: Value) -> Option<String> {
        let old = self.get_value(key).map(|old| {
            *new.decor_mut() = old.decor().clone();
            shown(old)
        });
        match self {
            TableRef::Table(t) => t[key] = Item::Value(new),
            TableRef::Inline(t) => {
                t.insert(key, new);
            }
        }
        old
    }
}

/// Walk `segs` from `container` and set the leaf, returning the value the leaf
/// held before — `None` if it held nothing.
fn set_path(
    container: &mut TableRef,
    segs: &[&str],
    value: Value,
    path: &str,
) -> Result<Option<String>, String> {
    let (key, rest) = segs.split_first().expect("non-empty by construction");

    if rest.is_empty() {
        return Ok(container.put(key, value));
    }

    // One more level. Two shapes actually occur in a scene file: an inline
    // table (`variations = { swirl = 0.35 }`) and a 3-array (`translation`,
    // `rotation`, `color`, per-axis `scale`).
    if rest.len() == 1 {
        // Creating `variations` on a transform that has none is a legitimate
        // edit, and the only container we invent — every other path must
        // already exist, or the user has mistyped something.
        if *key == "variations" && container.get_value(key).is_none() {
            container.put(key, Value::InlineTable(InlineTable::new()));
        }
    }

    let existing = container
        .get_value(key)
        .ok_or_else(|| format!("--set '{path}': '{key}' is not set on this scene"))?
        .clone();

    match existing {
        Value::InlineTable(mut it) => {
            let was = set_path(&mut TableRef::Inline(&mut it), rest, value, path)?;
            container.put(key, Value::InlineTable(it));
            Ok(was)
        }
        Value::Array(mut arr) => {
            if rest.len() != 1 {
                return Err(format!("--set '{path}': '{key}' is an array; index it once"));
            }
            let i = axis_index(rest[0]).ok_or_else(|| {
                format!(
                    "--set '{path}': '{}' is not an index into '{key}' (use x/y/z or 0/1/2)",
                    rest[0]
                )
            })?;
            if i >= arr.len() {
                return Err(format!(
                    "--set '{path}': index {i} is out of range for '{key}' (len {})",
                    arr.len()
                ));
            }
            let was = set_array_elem(&mut arr, i, value);
            container.put(key, Value::Array(arr));
            Ok(was)
        }
        other => Err(format!(
            "--set '{path}': '{key}' is a {} and cannot be indexed further",
            type_name(&other)
        )),
    }
}

fn set_array_elem(arr: &mut Array, i: usize, mut new: Value) -> Option<String> {
    let old = arr.get(i).map(|old| {
        *new.decor_mut() = old.decor().clone();
        shown(old)
    });
    arr.replace(i, new);
    old
}

/// `x`/`y`/`z` or a plain integer.
fn axis_index(s: &str) -> Option<usize> {
    match s {
        "x" | "r" => Some(0),
        "y" | "g" => Some(1),
        "z" | "b" => Some(2),
        _ => s.parse::<usize>().ok(),
    }
}

/// Parse as a TOML value; anything that isn't valid TOML is taken as a bare
/// string, so `--set palette.name=ember` works without shell quoting.
fn parse_value(raw: &str) -> Value {
    raw.parse::<Value>()
        .unwrap_or_else(|_| Value::from(raw.to_string()))
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::Boolean(_) => "boolean",
        Value::Datetime(_) => "datetime",
        Value::Array(_) => "array",
        Value::InlineTable(_) => "table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = r#"
[meta]
name = "T"
point_size = 0.002   # a trailing comment
haze = 0.1

[camera]
distance = 3.0

[zoom]
map = "a"
radius = 3.2

[[transform]]
name = "a"
translation = [0.0, 0.5, 0.0]
scale = 0.6
color = [1.0, 0.2, 0.2]
weight = 1.0

[[transform]]
name = "b"
translation = [0.1, 0.0, 0.0]
scale = 0.4
color = [0.2, 0.2, 1.0]
weight = 2.0
variations = { swirl = 0.35, linear = 0.65 }
"#;

    fn set(specs: &[&str]) -> String {
        let owned: Vec<String> = specs.iter().map(|s| s.to_string()).collect();
        apply_recording(SCENE, &owned).expect("should apply").0
    }

    fn err(spec: &str) -> String {
        apply_recording(SCENE, &[spec.to_string()]).unwrap_err()
    }

    #[test]
    fn sets_a_meta_scalar() {
        assert!(set(&["meta.haze=0.75"]).contains("haze = 0.75"));
    }

    #[test]
    fn keeps_a_trailing_comment_on_the_line_it_rewrites() {
        let out = set(&["meta.point_size=0.004"]);
        assert!(out.contains("point_size = 0.004"));
        assert!(out.contains("# a trailing comment"), "decor was dropped: {out}");
    }

    #[test]
    fn selects_a_transform_by_name_and_by_index() {
        assert!(set(&["transform.b.weight=9.0"]).contains("weight = 9.0"));
        let by_index = set(&["transform.0.weight=7.5"]);
        // transform 0 is "a"; its weight moved and b's did not
        let a = by_index.split("[[transform]]").nth(1).unwrap();
        assert!(a.contains("weight = 7.5"), "{a}");
    }

    #[test]
    fn sets_a_variation_inside_an_inline_table() {
        let out = set(&["transform.b.variations.swirl=0.9"]);
        assert!(out.contains("swirl = 0.9"), "{out}");
        // the sibling survives
        assert!(out.contains("linear = 0.65"), "{out}");
    }

    #[test]
    fn creates_a_variations_table_when_the_transform_has_none() {
        let out = set(&["transform.a.variations.absfold=0.15"]);
        assert!(out.contains("absfold = 0.15"), "{out}");
    }

    #[test]
    fn indexes_arrays_by_axis_and_by_number() {
        assert!(set(&["transform.a.translation.y=1.25"]).contains("0.0, 1.25, 0.0"));
        assert!(set(&["transform.a.translation.2=1.5"]).contains("0.0, 0.5, 1.5"));
    }

    #[test]
    fn takes_a_whole_array_at_once() {
        let out = set(&["transform.a.scale=[0.05, 0.6, 0.05]"]);
        assert!(out.contains("scale = [0.05, 0.6, 0.05]"), "{out}");
    }

    #[test]
    fn bare_words_become_strings() {
        assert!(set(&["zoom.map=descent"]).contains(r#"map = "descent""#));
    }

    #[test]
    fn applies_several_in_order() {
        let out = set(&["meta.haze=0.4", "meta.haze=0.6", "camera.distance=9.0"]);
        assert!(out.contains("haze = 0.6"));
        assert!(out.contains("distance = 9.0"));
    }

    #[test]
    fn the_result_still_parses_as_a_scene() {
        let out = set(&["transform.b.variations.swirl=0.9", "meta.haze=0.3"]);
        let doc: DocumentMut = out.parse().expect("still valid TOML");
        assert_eq!(doc["meta"]["haze"].as_float(), Some(0.3));
    }

    // --- the errors, which are the point: a typo must never be a silent no-op ---

    #[test]
    fn an_unknown_transform_name_lists_the_real_ones() {
        let e = err("transform.nosuch.weight=1.0");
        assert!(e.contains("no transform named 'nosuch'"), "{e}");
        assert!(e.contains("0:a") && e.contains("1:b"), "{e}");
    }

    #[test]
    fn an_out_of_range_index_is_an_error() {
        assert!(err("transform.9.weight=1.0").contains("out of range"));
    }

    #[test]
    fn an_unknown_section_is_an_error() {
        assert!(err("mesh.thing=1.0").contains("unknown section"));
    }

    #[test]
    fn a_missing_equals_is_an_error() {
        assert!(err("meta.haze").contains("expected <path>=<value>"));
    }

    #[test]
    fn a_bare_key_with_no_section_is_an_error() {
        assert!(err("haze=0.5").contains("needs a section"));
    }

    #[test]
    fn a_bad_axis_index_is_an_error() {
        assert!(err("transform.a.translation.w=1.0").contains("not an index"));
    }

    // --- misspelled fields, which used to be a silent no-op: the value was
    // written under the wrong key and nothing downstream ever read it ---

    #[test]
    fn a_misspelled_meta_field_is_an_error_with_a_suggestion() {
        let e = err("meta.exposre=1.0");
        assert!(e.contains("'exposre' is not a known meta field"), "{e}");
        assert!(e.contains("did you mean 'exposure'"), "{e}");
    }

    #[test]
    fn a_misspelled_camera_field_is_an_error() {
        let e = err("camera.distanc=5.0");
        assert!(e.contains("'distanc' is not a known camera field"), "{e}");
        assert!(e.contains("did you mean 'distance'"), "{e}");
    }

    #[test]
    fn a_misspelled_transform_field_is_an_error() {
        let e = err("transform.a.wieght=0.5");
        assert!(e.contains("'wieght' is not a known transform field"), "{e}");
        assert!(e.contains("did you mean 'weight'"), "{e}");
    }

    #[test]
    fn a_misspelled_variation_name_is_an_error() {
        let e = err("transform.a.variations.swril=0.5");
        assert!(e.contains("'swril' is not a known variation"), "{e}");
        assert!(e.contains("did you mean 'swirl'"), "{e}");
    }

    #[test]
    fn a_field_with_no_close_match_lists_the_valid_ones() {
        let e = err("meta.xyzzy=0.5");
        assert!(e.contains("Valid: name, author"), "{e}");
    }

    /// `struct_fields!` itself, exercised on a throwaway fixture rather than
    /// one of the real scene structs: the derived list is exactly the field
    /// names given, in order. The half of the mechanism this can't cover —
    /// that an *unlisted* field fails the build — was verified by hand (see
    /// the task report); it can't be a passing `#[test]`, because the whole
    /// point is that it doesn't compile.
    #[test]
    fn struct_fields_derives_the_real_field_list() {
        struct Fixture {
            pub a: i32,
            pub b: i32,
            pub c: i32,
        }
        const FIXTURE_FIELDS: &[&str] =
            struct_fields!(_assert_fixture_exhaustive, Fixture, a, b, c);
        assert_eq!(FIXTURE_FIELDS, &["a", "b", "c"]);
    }

    /// The distinction this file has to get right: `point_count` is a real
    /// `[meta]` field that this particular scene just doesn't set. That is
    /// legitimate — the whole point of `--set` is to add such a key — and
    /// must keep working even though the section-level check below now
    /// rejects field names that were never real.
    #[test]
    fn a_real_field_absent_from_this_scene_can_still_be_set() {
        assert!(set(&["meta.point_count=500"]).contains("point_count = 500"));
    }

    #[test]
    fn setting_into_a_missing_section_is_an_error() {
        let e = apply_recording("[meta]\nname = \"x\"\n", &["palette.rotate=0.5".into()])
            .unwrap_err();
        assert!(e.contains("no [palette] table"), "{e}");
    }

    #[test]
    fn no_overrides_is_the_identity() {
        assert_eq!(apply_recording(SCENE, &[]).unwrap().0, SCENE);
    }

    // --- what each override displaced, which is what `--info` reports ---

    fn record(specs: &[&str]) -> Vec<Applied> {
        let owned: Vec<String> = specs.iter().map(|s| s.to_string()).collect();
        apply_recording(SCENE, &owned).expect("should apply").1
    }

    #[test]
    fn records_what_each_override_replaced() {
        let a = record(&["meta.haze=0.75", "transform.b.weight=9.0"]);
        assert_eq!(a[0], Applied {
            path: "meta.haze".into(),
            from: Some("0.1".into()),
            to: "0.75".into(),
        });
        assert_eq!(a[1].path, "transform.b.weight");
        assert_eq!(a[1].from.as_deref(), Some("2.0"));
    }

    #[test]
    fn records_an_element_of_an_array_not_the_whole_array() {
        let a = record(&["transform.a.translation.y=1.25"]);
        assert_eq!(a[0].from.as_deref(), Some("0.5"));
        assert_eq!(a[0].to, "1.25");
    }

    #[test]
    fn a_key_that_did_not_exist_reports_no_previous_value() {
        let a = record(&["transform.a.variations.absfold=0.15"]);
        assert_eq!(a[0].from, None);
    }

    /// Two sets on one path are a chain, and the second replaced the first —
    /// which is what happened. Diffing against the file afterwards would lose
    /// the middle value and imply an edit nobody made.
    #[test]
    fn a_repeated_path_reports_the_value_it_actually_displaced() {
        let a = record(&["meta.haze=0.4", "meta.haze=0.6"]);
        assert_eq!(a[0].from.as_deref(), Some("0.1"));
        assert_eq!(a[1].from.as_deref(), Some("0.4"));
    }
}
