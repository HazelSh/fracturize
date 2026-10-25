//! What changed between two scenes, and a three-way merge of two sets of
//! changes. The shared core of agent collaboration (AGENT-COLLAB-PLAN.md §3):
//! a proposal card's summary, a tweak's diff against the option it tweaks,
//! and Apply, which lays a proposal's changes over the live scene rather than
//! writing the proposal over it.
//!
//!     fracturize --diff a.toml b.toml
//!     fracturize --diff mine.toml theirs.toml --base base.toml
//!
//! **The comparison is on the canonical scene, not the text.** Both sides go
//! through the loader and `Scene::to_toml_string`, so reformatting, comments
//! and spellings the loader normalises (`fog` for `haze`, `C5` for `Cyc5`)
//! are not changes.
//!
//! **A change is a path `--set` accepts**, wherever one exists, so the diff
//! doubles as a recipe: applying its set-ops to A with `-S` gives B
//! (`set_ops_rebuild_the_other_scene` holds that over every scene in
//! `scenes/`). What `--set` can't express is listed as structural: a
//! transform added, removed or reordered, a key going back to its default, a
//! block (symmetry, a camera path, an xaos matrix) that changed as a whole.
//!
//! Transforms and subscenes are matched by name. Unnamed or duplicate-named
//! ones are matched by position, which is right until one is inserted before
//! them; the loader names nothing, so that is the honest limit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use toml::Value;

use crate::scene::Scene;

/// A place in a scene: `["meta", "haze"]`,
/// `["transform", "petal", "variations", "swirl"]`. Containers (a transform,
/// a subscene, a `[zoom]` or `[palette]` table) also have a *marker* path,
/// `["@transform", "petal"]`, which is present exactly when the container is.
type Path_ = Vec<String>;

/// The top-level arrays whose elements are matched by identity.
const KEYED: &[&str] = &["transform", "subscene"];
/// The top-level tables that always mean "all defaults" when absent, so they
/// carry no marker. Every other table is a switch (`[zoom]` turns zoom on).
const IMPLICIT: &[&str] = &["meta", "camera"];

/// A scene flattened: every leaf value, plus which containers exist, and the
/// order of each keyed array.
#[derive(Clone, Debug, Default, PartialEq)]
struct Flat {
    leaves: BTreeMap<Path_, Value>,
    order: BTreeMap<String, Vec<String>>,
}

/// The scene as a file says it, read back and said again.
///
/// One pass is not enough: a scene in memory can hold a rotation the writer
/// spells on one euler branch, and the same matrix read back from that text
/// spells on the other. The second pass is the text a loaded scene settles
/// on, which is what every side of a diff or merge has to be compared in.
pub fn canonical_text(scene: &Scene) -> Result<String, String> {
    let once = scene.to_toml_string()?;
    let dir = std::env::temp_dir();
    match Scene::from_toml_str(&once, &dir) {
        Ok(back) => back.to_toml_string(),
        // `file =` references that only resolve beside the original: the
        // first pass is the best there is.
        Err(_) => Ok(once),
    }
}

fn canonical(scene: &Scene) -> Result<toml::Table, String> {
    let text = canonical_text(scene)?;
    toml::from_str(&text).map_err(|e| format!("canonical scene does not reparse: {e}"))
}

/// Element identities for a keyed array: the name where it is unique, else
/// `#<index>`.
fn ids(elems: &[Value]) -> Vec<String> {
    let name = |v: &Value| v.get("name").and_then(Value::as_str).map(str::to_string);
    let names: Vec<Option<String>> = elems.iter().map(name).collect();
    names
        .iter()
        .enumerate()
        .map(|(i, n)| match n {
            Some(n) if names.iter().filter(|m| m.as_deref() == Some(n)).count() == 1 => n.clone(),
            _ => format!("#{i}"),
        })
        .collect()
}

fn flatten(doc: &toml::Table) -> Flat {
    let mut flat = Flat::default();
    let p = |segs: &[&str]| segs.iter().map(|s| s.to_string()).collect::<Path_>();
    for (key, value) in doc {
        match value {
            Value::Array(elems) if KEYED.contains(&key.as_str()) => {
                let ids = ids(elems);
                for (id, elem) in ids.iter().zip(elems) {
                    flat.leaves.insert(p(&[&format!("@{key}"), id]), Value::Boolean(true));
                    let Some(t) = elem.as_table() else { continue };
                    for (k, v) in t {
                        match v {
                            // variations, xaos rows, a subscene's palette: one
                            // level deeper, so a single weight is one change.
                            Value::Table(inner) => {
                                for (k2, v2) in inner {
                                    flat.leaves.insert(p(&[key, id, k, k2]), v2.clone());
                                }
                            }
                            _ => {
                                flat.leaves.insert(p(&[key, id, k]), v.clone());
                            }
                        }
                    }
                }
                flat.order.insert(key.clone(), ids);
            }
            Value::Table(t) => {
                if !IMPLICIT.contains(&key.as_str()) {
                    flat.leaves.insert(p(&[&format!("@{key}")]), Value::Boolean(true));
                }
                for (k, v) in t {
                    flat.leaves.insert(p(&[key, k]), v.clone());
                }
            }
            // symmetry blocks, and anything else at the top: one value.
            _ => {
                flat.leaves.insert(p(&[key]), value.clone());
            }
        }
    }
    flat
}

/// The marker path of the container a leaf lives in, if it lives in one.
fn container_of(path: &Path_) -> Option<Path_> {
    if path[0].starts_with('@') {
        return None;
    }
    if KEYED.contains(&path[0].as_str()) {
        return Some(vec![format!("@{}", path[0]), path[1].clone()]);
    }
    if path.len() >= 2 && !IMPLICIT.contains(&path[0].as_str()) {
        return Some(vec![format!("@{}", path[0])]);
    }
    None
}

fn unflatten(flat: &Flat) -> toml::Table {
    let mut doc = toml::Table::new();
    // Keyed arrays are assembled per element, then laid out in order.
    let mut elems: BTreeMap<(String, String), toml::Table> = BTreeMap::new();
    for (path, value) in &flat.leaves {
        let head = path[0].as_str();
        if let Some(stripped) = head.strip_prefix('@') {
            if KEYED.contains(&stripped) {
                elems.entry((stripped.to_string(), path[1].clone())).or_default();
            } else {
                doc.entry(stripped.to_string()).or_insert_with(|| Value::Table(Default::default()));
            }
            continue;
        }
        if KEYED.contains(&head) {
            let elem = elems.entry((head.to_string(), path[1].clone())).or_default();
            if path.len() == 4 {
                let inner = elem
                    .entry(path[2].clone())
                    .or_insert_with(|| Value::Table(Default::default()));
                if let Value::Table(t) = inner {
                    t.insert(path[3].clone(), value.clone());
                }
            } else {
                elem.insert(path[2].clone(), value.clone());
            }
            continue;
        }
        if path.len() == 1 {
            doc.insert(head.to_string(), value.clone());
        } else {
            let t = doc.entry(head.to_string()).or_insert_with(|| Value::Table(Default::default()));
            if let Value::Table(t) = t {
                t.insert(path[1].clone(), value.clone());
            }
        }
    }
    for key in KEYED {
        let mut present: Vec<String> =
            elems.keys().filter(|(k, _)| k == key).map(|(_, id)| id.clone()).collect();
        if present.is_empty() {
            continue;
        }
        let order = flat.order.get(*key).cloned().unwrap_or_default();
        present.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
        let arr = present
            .into_iter()
            .map(|id| Value::Table(elems.remove(&(key.to_string(), id)).unwrap()))
            .collect();
        doc.insert(key.to_string(), Value::Array(arr));
    }
    doc
}

// ---------------------------------------------------------------- the diff

/// One line of a diff.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A valid `--set` argument: `transform.petal.weight=0.62`
    Set(String),
    /// A transform or subscene that only the new scene has
    Added { kind: String, id: String },
    /// One that only the old scene has
    Removed { kind: String, id: String },
    /// A `[zoom]` or `[palette]` table switched on or off
    Switched { table: String, on: bool },
    /// A key the new scene leaves to its default
    Unset(String),
    /// A block `--set` can't reach, changed as a whole
    Replaced(String),
    /// The same elements of a keyed array, in a different order
    Reordered { kind: String, order: Vec<String> },
}

impl Change {
    pub fn is_structural(&self) -> bool {
        !matches!(self, Change::Set(_))
    }
}

/// A float as the shortest text that gives back the same `f32`, which is
/// what the scene stores: `0.3`, not `0.30000001192092896`.
fn shown(v: &Value) -> String {
    match v {
        Value::Float(f) => {
            let s = (*f as f32).to_string();
            if s.contains(['.', 'e', 'i', 'N']) { s } else { format!("{s}.0") }
        }
        Value::Array(a) => format!("[{}]", a.iter().map(shown).collect::<Vec<_>>().join(", ")),
        Value::Table(t) => format!(
            "{{ {} }}",
            t.iter().map(|(k, v)| format!("{} = {}", key(k), shown(v))).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

fn key(k: &str) -> String {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        k.to_string()
    } else {
        format!("{k:?}")
    }
}

/// How a path is spelled on the command line. A positional id `#3` is the
/// index `3`, which `--set` takes as-is.
fn spelled(path: &Path_) -> String {
    path.iter().map(|s| s.strip_prefix('#').unwrap_or(s)).collect::<Vec<_>>().join(".")
}

/// Can `--set` write this leaf? Blocks it can't reach are whole values.
fn settable(path: &Path_) -> bool {
    match path[0].as_str() {
        "meta" | "zoom" | "palette" => path.len() == 2,
        "camera" => path.len() == 2 && path[1] != "path",
        "transform" => path.len() >= 3,
        "subscene" => path.len() >= 3 && !matches!(path[2].as_str(), "transform" | "symmetry"),
        _ => false,
    }
}

/// The changes that turn `a` into `b`.
pub fn diff(a: &Scene, b: &Scene) -> Result<Vec<Change>, String> {
    Ok(diff_flat(&flatten(&canonical(a)?), &flatten(&canonical(b)?)))
}

fn diff_flat(a: &Flat, b: &Flat) -> Vec<Change> {
    let mut out = Vec::new();
    let has = |f: &Flat, c: &Option<Path_>| c.as_ref().is_none_or(|c| f.leaves.contains_key(c));
    let paths: BTreeSet<&Path_> = a.leaves.keys().chain(b.leaves.keys()).collect();
    // Containers first, so the reader meets "+ transform x" before anything
    // about x, then values in path order.
    for path in &paths {
        if !path[0].starts_with('@') {
            continue;
        }
        let kind = path[0][1..].to_string();
        match (a.leaves.contains_key(*path), b.leaves.contains_key(*path)) {
            (false, true) if path.len() == 2 => out.push(Change::Added { kind, id: path[1].clone() }),
            (true, false) if path.len() == 2 => out.push(Change::Removed { kind, id: path[1].clone() }),
            (false, true) => out.push(Change::Switched { table: kind, on: true }),
            (true, false) => out.push(Change::Switched { table: kind, on: false }),
            _ => {}
        }
    }
    for path in paths {
        if path[0].starts_with('@') {
            continue;
        }
        let container = container_of(path);
        // An added or removed container is one line, not one per field.
        if !has(a, &container) || !has(b, &container) {
            continue;
        }
        match (a.leaves.get(path), b.leaves.get(path)) {
            (x, y) if x == y => {}
            (_, Some(v)) if settable(path) => out.push(Change::Set(format!("{}={}", spelled(path), shown(v)))),
            (_, Some(_)) => out.push(Change::Replaced(spelled(path))),
            (Some(_), None) => out.push(Change::Unset(spelled(path))),
            (None, None) => unreachable!(),
        }
    }
    for (kind, order) in &b.order {
        let Some(before) = a.order.get(kind) else { continue };
        let common = |o: &Vec<String>, other: &Vec<String>| {
            o.iter().filter(|id| other.contains(id)).cloned().collect::<Vec<_>>()
        };
        let (x, y) = (common(before, order), common(order, before));
        if x != y {
            out.push(Change::Reordered { kind: kind.clone(), order: y });
        }
    }
    out
}

/// The diff as `--diff` prints it: set-ops first, each a valid `-S`
/// argument, then the structural changes.
pub fn render(changes: &[Change]) -> String {
    let mut s = String::new();
    let sets: Vec<&Change> = changes.iter().filter(|c| !c.is_structural()).collect();
    let structural: Vec<&Change> = changes.iter().filter(|c| c.is_structural()).collect();
    if changes.is_empty() {
        return "# no changes\n".to_string();
    }
    if !sets.is_empty() {
        s.push_str("# set-ops: each line is a valid -S argument\n");
        for c in sets {
            if let Change::Set(op) = c {
                s.push_str(op);
                s.push('\n');
            }
        }
    }
    if !structural.is_empty() {
        s.push_str("# structural: not expressible as -S\n");
        for c in structural {
            let line = match c {
                Change::Added { kind, id } => format!("+ {kind} {id:?}"),
                Change::Removed { kind, id } => format!("- {kind} {id:?}"),
                Change::Switched { table, on: true } => format!("+ [{table}]"),
                Change::Switched { table, on: false } => format!("- [{table}]"),
                Change::Unset(p) => format!("~ {p} back to its default"),
                Change::Replaced(p) => format!("~ {p} rewritten"),
                Change::Reordered { kind, order } => format!("~ {kind} order: {}", order.join(", ")),
                Change::Set(_) => unreachable!(),
            };
            s.push_str(&line);
            s.push('\n');
        }
    }
    s
}

/// One line for a card: "4 values, +1 transform". Unused until the
/// proposal cards exist (AGENT-COLLAB-PLAN.md phase 2).
#[allow(dead_code)]
pub fn summary(changes: &[Change]) -> String {
    let n = |f: &dyn Fn(&Change) -> bool| changes.iter().filter(|c| f(c)).count();
    let plural = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    let mut parts = Vec::new();
    let values = n(&|c| matches!(c, Change::Set(_) | Change::Unset(_) | Change::Replaced(_)));
    if values > 0 {
        parts.push(plural(values, "value", "values"));
    }
    for (kind, word) in [("transform", "transform"), ("subscene", "subscene")] {
        let added = n(&|c| matches!(c, Change::Added { kind: k, .. } if k == kind));
        let removed = n(&|c| matches!(c, Change::Removed { kind: k, .. } if k == kind));
        if added > 0 {
            parts.push(format!("+{}", plural(added, word, &format!("{word}s"))));
        }
        if removed > 0 {
            parts.push(format!("−{}", plural(removed, word, &format!("{word}s"))));
        }
    }
    let other = n(&|c| matches!(c, Change::Switched { .. } | Change::Reordered { .. }));
    if other > 0 {
        parts.push(plural(other, "other change", "other changes"));
    }
    if parts.is_empty() { "no changes".to_string() } else { parts.join(", ") }
}

// --------------------------------------------------------------- the merge

/// A place where both sides changed the same thing differently.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// The path as `--set` spells it, or `transform.<id>` for a whole one
    pub path: String,
    /// Each side's value as text; `None` where it is absent (unset, removed)
    pub base: Option<String>,
    pub mine: Option<String>,
    pub theirs: Option<String>,
    /// Which side the merged scene took
    pub took_theirs: bool,
}

/// A three-way merge: `theirs`' changes from `base`, laid over `mine`.
#[derive(Clone)]
pub struct Merge {
    pub scene: Scene,
    /// Every conflict, resolved as asked (mine unless named in `take_theirs`)
    pub conflicts: Vec<Conflict>,
}

/// Lay the changes `base → theirs` over `mine`. A value only one side changed
/// takes that side; a value both changed alike is no conflict; one they
/// changed differently is a [`Conflict`], resolved to `mine` unless its path
/// is in `take_theirs`. A transform one side removed and the other edited is
/// a conflict on the whole transform. `base_dir` is where `file =`
/// references resolve.
pub fn merge(
    base: &Scene,
    mine: &Scene,
    theirs: &Scene,
    take_theirs: &BTreeSet<String>,
    base_dir: &Path,
) -> Result<Merge, String> {
    let (b, m, t) = (flatten(&canonical(base)?), flatten(&canonical(mine)?), flatten(&canonical(theirs)?));
    let (flat, conflicts) = merge_flat(&b, &m, &t, take_theirs);
    let text = toml::to_string(&unflatten(&flat)).map_err(|e| format!("merge: {e}"))?;
    let scene = Scene::from_toml_str(&text, base_dir).map_err(|e| {
        let paths: Vec<&str> = conflicts.iter().map(|c| c.path.as_str()).collect();
        if paths.is_empty() {
            format!("the merged scene doesn't load: {e}")
        } else {
            format!("the merged scene doesn't load: {e} (conflicts at {})", paths.join(", "))
        }
    })?;
    Ok(Merge { scene, conflicts })
}

fn merge_flat(b: &Flat, m: &Flat, t: &Flat, take_theirs: &BTreeSet<String>) -> (Flat, Vec<Conflict>) {
    let mut out = Flat::default();
    let mut conflicts = Vec::new();
    let get = |f: &Flat, p: &Path_| f.leaves.get(p).cloned();
    let text = |v: &Option<Value>| v.as_ref().map(shown);

    // Did this side change anything inside the container, against base?
    let touched = |f: &Flat, c: &Path_| {
        let inside = |p: &&Path_| container_of(p).as_ref() == Some(c);
        b.leaves.keys().chain(f.leaves.keys()).filter(inside).any(|p| b.leaves.get(p) != f.leaves.get(p))
    };

    // 1. Which containers survive.
    let markers: BTreeSet<Path_> =
        [b, m, t].iter().flat_map(|f| f.leaves.keys()).filter(|p| p[0].starts_with('@')).cloned().collect();
    // For each container, which sides' leaves to merge: both, or just the one
    // whose copy was kept.
    let mut source: BTreeMap<Path_, Option<bool>> = BTreeMap::new(); // None: 3-way; Some(true): theirs only
    for c in &markers {
        let (inb, inm, int) = (b.leaves.contains_key(c), m.leaves.contains_key(c), t.leaves.contains_key(c));
        let keep = match (inb, inm, int) {
            (_, true, true) => Some(None),
            (false, false, false) => None,
            (false, true, false) => Some(Some(false)),
            (false, false, true) => Some(Some(true)),
            (true, false, false) => None,
            // Removed by one side: gone, unless the other edited it.
            (true, true, false) | (true, false, true) => {
                let mine_kept = inm;
                let keeper = if mine_kept { m } else { t };
                if !touched(keeper, c) {
                    None
                } else {
                    let path = spelled(c).trim_start_matches('@').to_string();
                    let took_theirs = take_theirs.contains(&path);
                    let edited = Some("kept, edited".to_string());
                    let removed = None;
                    conflicts.push(Conflict {
                        path,
                        base: Some("present".into()),
                        mine: if mine_kept { edited.clone() } else { removed.clone() },
                        theirs: if mine_kept { removed } else { edited },
                        took_theirs,
                    });
                    // Keep it if the side we took is the side that kept it.
                    if took_theirs != mine_kept { Some(Some(!mine_kept)) } else { None }
                }
            }
        };
        if let Some(src) = keep {
            out.leaves.insert(c.clone(), Value::Boolean(true));
            source.insert(c.clone(), src);
        }
    }

    // 2. The values inside whatever survived, and the free-standing ones.
    let paths: BTreeSet<Path_> = [b, m, t]
        .iter()
        .flat_map(|f| f.leaves.keys())
        .filter(|p| !p[0].starts_with('@'))
        .cloned()
        .collect();
    for p in paths {
        let src = match container_of(&p) {
            Some(c) => match source.get(&c) {
                Some(s) => *s,
                None => continue, // its container is gone
            },
            None => None,
        };
        let v = match src {
            Some(true) => get(t, &p),
            Some(false) => get(m, &p),
            None => {
                let (vb, vm, vt) = (get(b, &p), get(m, &p), get(t, &p));
                if vm == vt || vt == vb {
                    vm
                } else if vm == vb {
                    vt
                } else {
                    let path = spelled(&p);
                    let took_theirs = take_theirs.contains(&path);
                    conflicts.push(Conflict {
                        path,
                        base: text(&vb),
                        mine: text(&vm),
                        theirs: text(&vt),
                        took_theirs,
                    });
                    if took_theirs { vt } else { vm }
                }
            }
        };
        if let Some(v) = v {
            out.leaves.insert(p, v);
        }
    }

    // 3. Order: theirs' if only they reordered, else mine, with anything
    // new appended in the order its side had it.
    for key in KEYED {
        let (ob, om, ot) = (b.order.get(*key), m.order.get(*key), t.order.get(*key));
        let mut order: Vec<String> = if om == ob { ot.or(om).cloned() } else { om.cloned() }.unwrap_or_default();
        for id in om.into_iter().chain(ot).flatten() {
            if !order.contains(id) {
                order.push(id.clone());
            }
        }
        if !order.is_empty() {
            out.order.insert(key.to_string(), order);
        }
    }
    (out, conflicts)
}

/// The conflicts as `--diff --base` prints them.
pub fn render_conflicts(conflicts: &[Conflict]) -> String {
    if conflicts.is_empty() {
        return "# no conflicts\n".to_string();
    }
    let mut s = String::from("# conflicts: both sides changed these. Took mine unless noted\n");
    let or_unset = |v: &Option<String>| v.clone().unwrap_or_else(|| "(unset)".to_string());
    for c in conflicts {
        s.push_str(&format!(
            "! {}  base {}  mine {}  theirs {}{}\n",
            c.path,
            or_unset(&c.base),
            or_unset(&c.mine),
            or_unset(&c.theirs),
            if c.took_theirs { "  (took theirs)" } else { "" }
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    fn scene(text: &str) -> Scene {
        Scene::from_toml_str(text, &dir()).unwrap()
    }

    fn set(s: &Scene, ops: &[String]) -> Scene {
        let text = s.to_toml_string().unwrap();
        let (text, _) = crate::set::apply_recording(&text, ops).unwrap();
        scene(&text)
    }

    fn same(a: &Scene, b: &Scene) -> bool {
        canonical_text(a).unwrap() == canonical_text(b).unwrap()
    }

    const TWO: &str = r#"
[meta]
name = "two"
haze = 0.2

[[transform]]
name = "a"
translation = [0.5, 0.0, 0.0]
scale = 0.5
color = [1.0, 0.2, 0.2]
color_value = 0.0

[[transform]]
name = "b"
translation = [-0.5, 0.0, 0.0]
scale = 0.5
color = [0.2, 0.2, 1.0]
color_value = 0.5
variations = { swirl = 0.3, linear = 0.7 }
"#;

    fn ops(changes: &[Change]) -> Vec<String> {
        changes.iter().filter_map(|c| if let Change::Set(s) = c { Some(s.clone()) } else { None }).collect()
    }

    #[test]
    fn an_unchanged_scene_has_no_diff() {
        let a = scene(TWO);
        assert_eq!(diff(&a, &a).unwrap(), vec![]);
        assert_eq!(render(&[]), "# no changes\n");
    }

    #[test]
    fn values_come_out_as_set_ops_that_apply() {
        let a = scene(TWO);
        let ops_in = vec![
            "meta.haze=0.5".to_string(),
            "transform.b.variations.swirl=0.45".to_string(),
            "transform.a.weight=2.0".to_string(),
        ];
        let b = set(&a, &ops_in);
        let d = diff(&a, &b).unwrap();
        assert!(d.iter().all(|c| !c.is_structural()), "{d:?}");
        assert!(ops(&d).contains(&"meta.haze=0.5".to_string()), "{d:?}");
        assert!(same(&set(&a, &ops(&d)), &b));
    }

    #[test]
    fn floats_print_as_the_f32_they_are() {
        let a = scene(TWO);
        let b = set(&a, &["meta.haze=0.3".to_string()]);
        assert_eq!(ops(&diff(&a, &b).unwrap()), vec!["meta.haze=0.3"]);
    }

    #[test]
    fn an_added_transform_is_one_line() {
        let a = scene(TWO);
        let b = scene(&format!("{TWO}\n[[transform]]\nname = \"c\"\ntranslation = [0.0, 0.5, 0.0]\nscale = 0.3\ncolor = [0.2, 1.0, 0.2]\ncolor_value = 0.9\n"));
        let d = diff(&a, &b).unwrap();
        assert_eq!(d, vec![Change::Added { kind: "transform".into(), id: "c".into() }]);
        assert_eq!(summary(&d), "+1 transform");
        assert_eq!(diff(&b, &a).unwrap(), vec![Change::Removed { kind: "transform".into(), id: "c".into() }]);
    }

    /// A scene that leaves its colours to their positions keeps leaving
    /// them, so adding a map is that map and nothing else.
    #[test]
    fn adding_to_an_auto_coloured_scene_is_one_line() {
        let auto = TWO.replace("color = [1.0, 0.2, 0.2]\ncolor_value = 0.0\n", "")
            .replace("color = [0.2, 0.2, 1.0]\ncolor_value = 0.5\n", "");
        assert!(!auto.contains("color"));
        let a = scene(&auto);
        let mut b = a.clone();
        let mut spec = b.transforms[0].clone();
        spec.matrix.w_axis.y = 0.5;
        b.insert_transform(2, spec, Some("c".into()), None, None);
        assert_eq!(diff(&a, &b).unwrap(), vec![Change::Added { kind: "transform".into(), id: "c".into() }]);
    }

    #[test]
    fn merge_takes_each_sides_disjoint_edits() {
        let base = scene(TWO);
        let mine = set(&base, &["meta.haze=0.6".into()]);
        let theirs = set(&base, &["transform.a.weight=3.0".into()]);
        let m = merge(&base, &mine, &theirs, &BTreeSet::new(), &dir()).unwrap();
        assert!(m.conflicts.is_empty());
        let want = set(&base, &["meta.haze=0.6".into(), "transform.a.weight=3.0".into()]);
        assert!(same(&m.scene, &want), "{}", m.scene.to_toml_string().unwrap());
    }

    #[test]
    fn merge_identities() {
        let a = scene(TWO);
        let b = set(&a, &["meta.haze=0.6".into(), "transform.b.variations.swirl=0.1".into()]);
        let none = BTreeSet::new();
        assert!(same(&merge(&a, &a, &b, &none, &dir()).unwrap().scene, &b));
        assert!(same(&merge(&a, &b, &a, &none, &dir()).unwrap().scene, &b));
        assert!(same(&merge(&a, &b, &b, &none, &dir()).unwrap().scene, &b));
    }

    #[test]
    fn a_value_both_changed_is_a_conflict_resolved_as_asked() {
        let base = scene(TWO);
        let mine = set(&base, &["meta.haze=0.6".into()]);
        let theirs = set(&base, &["meta.haze=0.9".into()]);
        let m = merge(&base, &mine, &theirs, &BTreeSet::new(), &dir()).unwrap();
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(m.conflicts[0].path, "meta.haze");
        assert!(same(&m.scene, &mine));
        let take: BTreeSet<String> = ["meta.haze".to_string()].into();
        let m = merge(&base, &mine, &theirs, &take, &dir()).unwrap();
        assert!(m.conflicts[0].took_theirs);
        assert!(same(&m.scene, &theirs));
    }

    #[test]
    fn a_transform_they_add_lands_beside_my_edits() {
        let base = scene(TWO);
        let mine = set(&base, &["transform.a.weight=2.0".into()]);
        let theirs = scene(&format!("{TWO}\n[[transform]]\nname = \"c\"\ntranslation = [0.0, 0.5, 0.0]\nscale = 0.3\ncolor = [0.2, 1.0, 0.2]\ncolor_value = 0.9\n"));
        let m = merge(&base, &mine, &theirs, &BTreeSet::new(), &dir()).unwrap();
        assert!(m.conflicts.is_empty());
        assert_eq!(m.scene.transform_names.len(), 3);
        assert!(diff(&mine, &m.scene).unwrap()
            == vec![Change::Added { kind: "transform".into(), id: "c".into() }]);
    }

    #[test]
    fn removing_what_the_other_side_edited_is_a_conflict() {
        let base = scene(&format!("{TWO}\n[[transform]]\nname = \"c\"\ntranslation = [0.0, 0.5, 0.0]\nscale = 0.3\ncolor = [0.2, 1.0, 0.2]\ncolor_value = 0.9\n"));
        let theirs = scene(TWO); // removed c
        let mine = set(&base, &["transform.c.weight=4.0".into()]);
        let m = merge(&base, &mine, &theirs, &BTreeSet::new(), &dir()).unwrap();
        assert_eq!(m.conflicts.len(), 1, "{:?}", m.conflicts);
        assert_eq!(m.conflicts[0].path, "transform.c");
        assert!(same(&m.scene, &mine));
        let take: BTreeSet<String> = ["transform.c".to_string()].into();
        let m = merge(&base, &mine, &theirs, &take, &dir()).unwrap();
        assert!(same(&m.scene, &theirs));
        // ...and removing one nobody edited is no conflict at all.
        let m = merge(&base, &base, &theirs, &BTreeSet::new(), &dir()).unwrap();
        assert!(m.conflicts.is_empty() && same(&m.scene, &theirs));
    }

    /// The plan's round-trip property, over every scene the project ships:
    /// mutate it, diff, and the set-ops alone rebuild the mutation whenever
    /// the diff has nothing structural in it. And merging the mutation in
    /// over an unchanged scene gives the mutation, structural or not.
    #[test]
    fn set_ops_rebuild_the_other_scene() {
        let mut checked = 0;
        let mut entries: Vec<_> = std::fs::read_dir("scenes").unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries.into_iter().filter(|p| p.extension().is_some_and(|e| e == "toml")) {
            let Ok(a) = Scene::load(&path) else { continue };
            if !a.subscenes.is_empty() {
                continue; // a borrowed subscene's maps aren't the file's to set
            }
            for seed in 0..2u64 {
                let Ok(roll) = crate::dice::mutate_seeded(&crate::dice::RandomSettings::default(), &a, seed)
                else {
                    continue;
                };
                let b = roll.scene;
                let d = diff(&a, &b).unwrap();
                let dir = path.parent().unwrap();
                let merged = merge(&a, &a, &b, &BTreeSet::new(), dir)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert!(
                    same(&merged.scene, &b),
                    "{} seed {seed}: merge(a, a, b) != b, off by\n{}\nfor the diff\n{}",
                    path.display(),
                    render(&diff(&b, &merged.scene).unwrap()),
                    render(&d)
                );
                if d.iter().any(Change::is_structural) {
                    continue;
                }
                let text = canonical_text(&a).unwrap();
                let (text, _) = crate::set::apply_recording(&text, &ops(&d))
                    .unwrap_or_else(|e| panic!("{}: {e}\n{}", path.display(), render(&d)));
                let rebuilt = Scene::from_toml_str(&text, dir).unwrap();
                assert!(
                    same(&rebuilt, &b),
                    "{} seed {seed}:\n{}\nrebuilt is off by\n{}",
                    path.display(),
                    render(&d),
                    render(&diff(&b, &rebuilt).unwrap())
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "only {checked} scene/mutation pairs were checked");
    }
}
