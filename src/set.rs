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

/// Apply `path=value` overrides to a scene's TOML source, returning the
/// rewritten source. Formatting and comments are preserved (toml_edit only
/// rewrites the values that are touched).
pub fn apply(src: &str, overrides: &[String]) -> Result<String, String> {
    if overrides.is_empty() {
        return Ok(src.to_string());
    }
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e| format!("scene is not valid TOML: {e}"))?;
    for spec in overrides {
        apply_one(&mut doc, spec)?;
    }
    Ok(doc.to_string())
}

fn apply_one(doc: &mut DocumentMut, spec: &str) -> Result<(), String> {
    let (path, raw) = spec
        .split_once('=')
        .ok_or_else(|| format!("--set '{spec}': expected <path>=<value>"))?;
    let path = path.trim();
    let value = parse_value(raw.trim());

    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return Err(format!(
            "--set '{spec}': path needs a section, e.g. meta.haze, camera.distance, \
             zoom.radius, palette.rotate, transform.<name-or-index>.weight"
        ));
    }

    let section = segs[0];
    let rest = &segs[1..];

    if section == "transform" {
        return set_in_transform(doc, rest, value, path);
    }

    if !matches!(section, "meta" | "camera" | "zoom" | "palette") {
        return Err(format!(
            "--set '{path}': unknown section '{section}' (expected meta, camera, \
             zoom, palette or transform)"
        ));
    }
    let table = doc
        .get_mut(section)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            format!("--set '{path}': this scene has no [{section}] table to set into")
        })?;
    set_path(&mut TableRef::Table(table), rest, value, path)
}

/// Resolve `transform.<name-or-index>.<rest>` against the `[[transform]]` array.
fn set_in_transform(
    doc: &mut DocumentMut,
    segs: &[&str],
    value: Value,
    path: &str,
) -> Result<(), String> {
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

    /// Replace a key, keeping the old value's decor (a trailing `# comment`).
    fn put(&mut self, key: &str, mut new: Value) {
        if let Some(old) = self.get_value(key) {
            *new.decor_mut() = old.decor().clone();
        }
        match self {
            TableRef::Table(t) => t[key] = Item::Value(new),
            TableRef::Inline(t) => {
                t.insert(key, new);
            }
        }
    }
}

/// Walk `segs` from `container` and set the leaf.
fn set_path(
    container: &mut TableRef,
    segs: &[&str],
    value: Value,
    path: &str,
) -> Result<(), String> {
    let (key, rest) = segs.split_first().expect("non-empty by construction");

    if rest.is_empty() {
        container.put(key, value);
        return Ok(());
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
            set_path(&mut TableRef::Inline(&mut it), rest, value, path)?;
            container.put(key, Value::InlineTable(it));
            Ok(())
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
            set_array_elem(&mut arr, i, value);
            container.put(key, Value::Array(arr));
            Ok(())
        }
        other => Err(format!(
            "--set '{path}': '{key}' is a {} and cannot be indexed further",
            type_name(&other)
        )),
    }
}

fn set_array_elem(arr: &mut Array, i: usize, mut new: Value) {
    if let Some(old) = arr.get(i) {
        *new.decor_mut() = old.decor().clone();
    }
    arr.replace(i, new);
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
        apply(SCENE, &owned).expect("should apply")
    }

    fn err(spec: &str) -> String {
        apply(SCENE, &[spec.to_string()]).unwrap_err()
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

    #[test]
    fn setting_into_a_missing_section_is_an_error() {
        let e = apply("[meta]\nname = \"x\"\n", &["palette.rotate=0.5".into()]).unwrap_err();
        assert!(e.contains("no [palette] table"), "{e}");
    }

    #[test]
    fn no_overrides_is_the_identity() {
        assert_eq!(apply(SCENE, &[]).unwrap(), SCENE);
    }
}
