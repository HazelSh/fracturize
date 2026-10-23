//! Dice profiles as files: what the dice may do, saved, named, and shared.
//!
//! A profile reads like a scene file — a header comment, a `[meta]` table,
//! then a section per concern — so a person, an agent or a script can read
//! one, write one from scratch, or change one line of it:
//!
//! ```toml
//! [meta]
//! name = "spiky"
//! description = "Few transforms, sharp folds, never smooth"
//!
//! [parts]                    # what the dice may make: one line per part
//! color = { on = false }     # off: kept as it was, by a roll or a mutation
//! variations = { odds = 0.4, reroll = 0.25 }
//!
//! [ranges]                   # how each part is drawn
//! transforms = [2, 4]
//! lacunarity = { on = true, range = [12.0, 60.0] }
//!
//! [ranges.variations]        # one line per variation
//! julia = { enabled = true, weight = [0.2, 0.6] }
//!
//! [ranges.symmetry]
//! chance = 0.3
//!
//! [mutation]                 # how big a step a mutation takes
//! strength = 0.7
//! ```
//!
//! There is no roll section and mutate section: a part that's on may be made
//! by either, and one that's off is left alone by both.
//!
//! Every key is optional and takes its default when left out, and a key this
//! version doesn't know is reported, not silently dropped. Two older shapes
//! still read: a bare settings table (what `--random-config` always took), and
//! any file with a `[random]` table in it — prefs.toml.
//!
//! Profiles live by name in three places. The built-in ones are compiled in
//! ([`BUILTINS`], from `src/dice/profiles/`), so they are there wherever the
//! app is started from: `default` is always the shipped settings, and the rest
//! are presets — `anything`, `no-symmetry`, the two colour masks. Then the
//! person's own (`~/.config/fracturize/dice/<name>.toml`), and the
//! repository's shared ones (`scenes/dice/<name>.toml`, relative to where the
//! app runs). A built-in's name can't be taken by a saved profile; between the
//! other two, the person's win a clash.
//!
//! Saving over an existing file merges into it with `toml_edit`, as
//! `Scene::save` does, so comments written into a profile by hand survive the
//! window's Save.

use std::path::{Path, PathBuf};

use super::settings::RandomSettings;

/// The built-in profile: the shipped defaults, under a name that always
/// resolves and can't be saved over.
pub const BUILTIN: &str = "default";

/// The other built-in profiles, in the order the picker lists them: name and
/// file text. Each is an ordinary profile file, read the way any other is.
pub const BUILTINS: &[(&str, &str)] = &[
    ("anything", include_str!("profiles/anything.toml")),
    ("no-symmetry", include_str!("profiles/no-symmetry.toml")),
    ("colour-only", include_str!("profiles/colour-only.toml")),
    ("no-colour", include_str!("profiles/no-colour.toml")),
    ("symmetric", include_str!("profiles/symmetric.toml")),
];

fn builtin_text(name: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

/// Whether `name` is one of the built-in profiles, `default` included.
pub fn is_builtin(name: &str) -> bool {
    name == BUILTIN || builtin_text(name).is_some()
}

/// What a profile says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub name: String,
    pub description: String,
    pub author: String,
}

/// A profile, read.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub settings: RandomSettings,
    pub meta: Meta,
    /// Keys the file has that this version doesn't understand, as dotted
    /// paths, and anything else worth telling the person who wrote it.
    pub warnings: Vec<String>,
}

/// The tables a profile file is made of, in the order they're written.
const SECTIONS: [&str; 4] = ["meta", "parts", "ranges", "mutation"];

/// Read a profile from TOML text: a profile, a prefs file, or a bare table.
pub fn parse(text: &str) -> Result<Loaded, String> {
    let top: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let mut meta = Meta::default();
    let is_profile = top.get("random").is_none() && SECTIONS.iter().any(|k| top.get(*k).is_some_and(toml::Value::is_table));
    let (table, prefix) = if let Some(t) = top.get("random") {
        // prefs.toml: everything else in it is other preferences.
        let toml::Value::Table(t) = t else { return Err("[random] is not a table".into()) };
        (t.clone(), "random.")
    } else if SECTIONS.iter().any(|k| top.get(*k).is_some_and(toml::Value::is_table)) {
        // A profile file: [ranges] holds the settings' plain fields, and
        // [parts] and [mutation] are the struct's own tables.
        let section = |k: &str| match top.get(k) {
            Some(toml::Value::Table(t)) => Ok(t.clone()),
            Some(_) => Err(format!("[{k}] is not a table")),
            None => Ok(toml::Table::new()),
        };
        let mut table = section("ranges")?;
        for k in ["parts", "mutation"] {
            if table.remove(k).is_some() {
                warnings.push(format!("ranges.{k} belongs in its own [{k}] table, ignored"));
            }
        }
        table.insert("parts".into(), toml::Value::Table(section("parts")?));
        table.insert("mutation".into(), toml::Value::Table(section("mutation")?));
        if let Some(toml::Value::Table(m)) = top.get("meta") {
            let text = |k: &str| m.get(k).and_then(toml::Value::as_str).unwrap_or_default().to_string();
            meta = Meta { name: text("name"), description: text("description"), author: text("author") };
            for k in m.keys().filter(|k| !["name", "description", "author"].contains(&k.as_str())) {
                warnings.push(format!("unknown key meta.{k}, ignored"));
            }
        }
        for k in top.keys().filter(|k| !SECTIONS.contains(&k.as_str())) {
            warnings.push(format!("unknown key {k}, ignored (a profile has [meta], [parts], [ranges] and [mutation])"));
        }
        (table, "")
    } else {
        (top, "")
    };
    // Laid over the shipped defaults at every depth, so a key left out — even
    // one inside an entry, like a part's `reroll` — is the shipped value and
    // not the type's zero.
    let mut full = match toml::Value::try_from(RandomSettings::default()) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    };
    overlay(&mut full, &table);
    let settings: RandomSettings =
        toml::Value::Table(full).try_into().map_err(|e: toml::de::Error| e.to_string())?;
    let settings = settings.normalized();
    // Anything in the file that didn't land in the settings was not understood.
    if let Ok(toml::Value::Table(understood)) = toml::Value::try_from(&settings) {
        let mut found = Vec::new();
        unknown_keys(&table, &understood, prefix, &mut found);
        if is_profile {
            // Said the way the file spells it: a plain field lives in [ranges].
            for w in &mut found {
                if !w.starts_with("unknown key parts.") && !w.starts_with("unknown key mutation.") {
                    *w = w.replacen("unknown key ", "unknown key ranges.", 1);
                }
            }
        }
        warnings.extend(found);
    }
    Ok(Loaded { settings, meta, warnings })
}

/// `given` written over `base`, table by table.
fn overlay(base: &mut toml::Table, given: &toml::Table) {
    for (k, v) in given {
        match (base.get_mut(k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(g)) => overlay(b, g),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

fn unknown_keys(given: &toml::Table, understood: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    for (k, v) in given {
        let path = format!("{prefix}{k}");
        match (v, understood.get(k)) {
            (_, None) => out.push(format!("unknown key {path}, ignored")),
            (toml::Value::Table(g), Some(toml::Value::Table(u))) => unknown_keys(g, u, &format!("{path}."), out),
            _ => {}
        }
    }
}

/// The text of a fresh profile file.
#[cfg(test)]
pub fn to_toml_string(settings: &RandomSettings, meta: &Meta) -> String {
    fresh_document(settings, meta).to_string()
}

/// Write a profile to `path`, merging into what's there so comments written
/// by hand survive.
pub fn save(path: &Path, settings: &RandomSettings, meta: &Meta) -> Result<(), String> {
    let fresh = fresh_document(settings, meta);
    let content = match std::fs::read_to_string(path).ok().and_then(|s| s.parse::<toml_edit::DocumentMut>().ok()) {
        Some(mut doc) if SECTIONS.iter().any(|k| doc.contains_key(k)) => {
            merge(doc.as_table_mut(), fresh.as_table());
            doc.to_string()
        }
        _ => fresh.to_string(),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
    }
    std::fs::write(path, content).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

const HEADER: &str = "\
# A Fracturize dice profile: the parts of a flame the dice may make, and the
# ranges each is drawn from. A roll draws every part that's on; a mutation
# changes a few of them; a part that's off is kept as it was by both. Any key
# left out takes its default; ranges are [min, max].
#
#   fracturize --random --random-config <name or file>
#   fracturize -s scene.toml --random --random-config <name or file>
#   fracturize -s scene.toml -r sheet.png --mutations 8 --random-config <name or file>
#
# Or load it in the app's Random settings window. AGENTS.md, \"The dice\",
# says what every key does.
";

/// One-line explanations written above keys in a fresh file, by dotted path.
const NOTES: &[(&str, &str)] = &[
    (
        "parts",
        "What the dice may make. on: whether they may (off = keep the scene's own,\n\
         by a roll or a mutation). odds: how often a mutation picks it against the\n\
         others. reroll: chance a mutation replaces it rather than nudging it.",
    ),
    ("ranges", "How each part is drawn."),
    ("ranges.transforms", "transforms per flame"),
    ("ranges.scale", "each transform's size, drawn log-uniformly"),
    ("ranges.squash", "chance a transform has one axis squashed"),
    ("ranges.affine", "chance a flame has no variations at all"),
    ("ranges.kinds", "different variations one flame draws from"),
    ("ranges.per_map", "variations on each transform, from the flame's kinds"),
    ("ranges.negative", "chance a variation weight is negative"),
    ("ranges.dimension", "similarity dimension bound; met by resizing"),
    ("ranges.lacunarity", "lacunarity bound; met by drawing again"),
    ("ranges.attempts", "candidates tried before giving up"),
    ("ranges.variations", "Which variations may be drawn, and their weight ranges (magnitudes)."),
    ("ranges.symmetry", "Which symmetry groups, and the chance a roll takes one."),
    ("mutation", "How big a step a mutation takes. A roll draws every part that's on,\nso none of this applies to it."),
    ("mutation.strength", "scales every nudge; 1 is a noticeable step"),
    ("mutation.changes", "parts changed per mutation"),
    ("mutation.backoff", "halve the strength after this many rejections in a row; 0 = never"),
    ("mutation.steer_dimension", "resize mutations onto the dimension bound, as rolls are"),
];

fn note(path: &str) -> Option<&'static str> {
    NOTES.iter().find(|(p, _)| *p == path).map(|(_, n)| *n)
}

/// `# note` lines for a table header.
fn table_decor(t: &mut toml_edit::Table, path: &str) {
    if let Some(n) = note(path) {
        let lines: String = n.lines().map(|l| format!("# {l}\n")).collect();
        t.decor_mut().set_prefix(format!("\n{lines}"));
    }
}

fn fresh_document(settings: &RandomSettings, meta: &Meta) -> toml_edit::DocumentMut {
    use toml_edit::{DocumentMut, Item, Table};
    let mut all = match toml::Value::try_from(settings) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    };
    let parts = all.remove("parts");
    let mutation = all.remove("mutation");
    let variations = all.remove("variations");
    let symmetry = all.remove("symmetry");

    let mut doc = DocumentMut::new();
    doc.decor_mut().set_prefix(HEADER);

    let mut m = Table::new();
    m["name"] = toml_edit::value(meta.name.as_str());
    m["description"] = toml_edit::value(meta.description.as_str());
    if !meta.author.is_empty() {
        m["author"] = toml_edit::value(meta.author.as_str());
    }
    m.decor_mut().set_prefix("\n");
    m.set_position(0);
    doc["meta"] = Item::Table(m);

    // Plain values in the order the struct declares them, each with its note
    // trailing on the same line, so the file reads like the settings window.
    let section = |path: &str, values: &toml::Table, order: &[&str]| {
        let mut t = Table::new();
        table_decor(&mut t, path);
        for k in order.iter().filter_map(|k| values.get_key_value(*k)) {
            let mut v = edit_value(k.1);
            if let Some(n) = note(&format!("{path}.{}", k.0)) {
                v.decor_mut().set_suffix(format!("  # {n}"));
            }
            t[k.0.as_str()] = Item::Value(v);
        }
        t
    };
    // One inline table per line: the long lists stay one line per entry.
    let lines = |path: &str, values: Option<toml::Value>, order: &[&str]| {
        let mut t = Table::new();
        table_decor(&mut t, path);
        if let Some(toml::Value::Table(values)) = values {
            for k in order {
                if let Some(v) = values.get(*k) {
                    t[*k] = Item::Value(edit_value(v));
                }
            }
        }
        t
    };

    // The parts first: they are what a profile is.
    let mut parts = lines("parts", parts, &super::settings::Parts::NAMES);
    parts.set_position(1);
    doc["parts"] = Item::Table(parts);

    let ranges_order = [
        "transforms", "scale", "squash", "affine", "kinds", "per_map", "negative", "dimension", "lacunarity",
        "attempts",
    ];
    let mut ranges = section("ranges", &all, &ranges_order);
    ranges.set_position(2);
    let names: Vec<String> = match &variations {
        Some(toml::Value::Table(v)) => v.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut vars = lines("ranges.variations", variations, &names);
    vars.set_position(3);
    ranges["variations"] = Item::Table(vars);
    let mut sym = match symmetry {
        Some(toml::Value::Table(t)) => {
            let order: Vec<&str> = t.keys().map(String::as_str).collect();
            section("ranges.symmetry", &t, &order)
        }
        _ => Table::new(),
    };
    sym.set_position(4);
    ranges["symmetry"] = Item::Table(sym);
    doc["ranges"] = Item::Table(ranges);

    if let Some(toml::Value::Table(mt)) = mutation {
        let mut mutation = section("mutation", &mt, &["strength", "changes", "backoff", "steer_dimension"]);
        mutation.set_position(5);
        doc["mutation"] = Item::Table(mutation);
    }
    doc
}

/// A `toml` value as a `toml_edit` one, tables inline. Floats are printed at
/// the precision they're held in (`f32`), so `0.18` is written `0.18` and not
/// `0.18000000715255737`.
fn edit_value(v: &toml::Value) -> toml_edit::Value {
    use toml_edit::Value;
    match v {
        toml::Value::String(s) => Value::from(s.as_str()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(format!("{}", *f as f32).parse::<f64>().unwrap_or(*f)),
        toml::Value::Boolean(b) => Value::from(*b),
        toml::Value::Datetime(d) => Value::from(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(edit_value).collect()),
        toml::Value::Table(t) => {
            // The switch first, then the numbers: `{ on = true, odds = 0.18 }`
            // reads as the sentence it is, where alphabetical order wouldn't.
            const FIRST: [&str; 4] = ["on", "enabled", "odds", "weight"];
            let rank = |k: &str| FIRST.iter().position(|f| *f == k).unwrap_or(FIRST.len());
            let mut keys: Vec<&String> = t.keys().collect();
            keys.sort_by_key(|k| (rank(k), k.as_str()));
            let mut it = toml_edit::InlineTable::new();
            for k in keys {
                it.insert(k, edit_value(&t[k]));
            }
            Value::InlineTable(it)
        }
    }
}

/// Put `fresh`'s values into `doc`, keeping `doc`'s comments and layout: a
/// key both have keeps its own decoration, a key only `doc` has is removed,
/// and a key only `fresh` has is added.
fn merge(doc: &mut toml_edit::Table, fresh: &toml_edit::Table) {
    use toml_edit::Item;
    let gone: Vec<String> = doc.iter().map(|(k, _)| k.to_string()).filter(|k| !fresh.contains_key(k)).collect();
    for k in gone {
        doc.remove(&k);
    }
    for (k, new) in fresh.iter() {
        match (doc.get_mut(k), new) {
            (Some(Item::Table(old)), Item::Table(new)) => merge(old, new),
            (Some(Item::Value(old)), Item::Value(new)) => {
                let decor = old.decor().clone();
                *old = new.clone();
                *old.decor_mut() = decor;
            }
            _ => {
                doc.insert(k, new.clone());
            }
        }
    }
}

// --- Named profiles ---------------------------------------------------------

/// The person's own profiles: `$XDG_CONFIG_HOME/fracturize/dice`.
pub fn user_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("fracturize").join("dice"))
}

/// Profiles shared in the repository, beside the scenes.
pub fn shared_dir() -> PathBuf {
    PathBuf::from("scenes").join("dice")
}

/// A profile that can be loaded by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// `None` for a built-in.
    pub path: Option<PathBuf>,
    /// From `scenes/dice/` rather than the person's own folder.
    pub shared: bool,
}

impl Entry {
    /// The built-in profile called `name`.
    pub fn builtin(name: &str) -> Self {
        Self { name: name.into(), path: None, shared: false }
    }

    pub fn is_builtin(&self) -> bool {
        self.path.is_none()
    }
}

/// Every profile by name: the built-ins first, in their own order, then the
/// person's, then the shared ones not shadowed by a profile of the person's,
/// each sorted.
pub fn list() -> Vec<Entry> {
    let mut out: Vec<Entry> =
        std::iter::once(BUILTIN).chain(BUILTINS.iter().map(|(n, _)| *n)).map(Entry::builtin).collect();
    let dirs = user_dir().map(|d| (d, false)).into_iter().chain([(shared_dir(), true)]);
    for (dir, shared) in dirs {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut found: Vec<Entry> = read
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .filter_map(|p| {
                let name = p.file_stem()?.to_str()?.to_string();
                Some(Entry { name, path: Some(p), shared })
            })
            .filter(|e| !out.iter().any(|o| o.name == e.name))
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(found);
    }
    out
}

/// What `--random-config` and the window's picker name: a file path, or a
/// profile's name.
pub fn resolve(arg: &str) -> Result<Entry, String> {
    let as_path = Path::new(arg);
    let looks_like_path = arg.contains(std::path::MAIN_SEPARATOR) || arg.ends_with(".toml") || as_path.is_file();
    if looks_like_path {
        if !as_path.is_file() {
            return Err(format!("no such file {arg}"));
        }
        let name = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or(arg).to_string();
        return Ok(Entry { name, path: Some(as_path.to_path_buf()), shared: false });
    }
    let all = list();
    all.iter().find(|e| e.name == arg).cloned().ok_or_else(|| {
        let names: Vec<_> = all.iter().map(|e| e.name.as_str()).collect();
        format!("no profile named '{arg}' (have: {})", names.join(", "))
    })
}

/// Read an entry: the file, or a built-in. A file's `[meta] name` is
/// overridden by its file name, which is what it's known by.
pub fn load(entry: &Entry) -> Result<Loaded, String> {
    let Some(path) = &entry.path else {
        if let Some(text) = builtin_text(&entry.name) {
            let mut loaded = parse(text).map_err(|e| format!("built-in {}: {}", entry.name, e))?;
            loaded.meta.name = entry.name.clone();
            return Ok(loaded);
        }
        return Ok(Loaded {
            settings: RandomSettings::default(),
            meta: Meta {
                name: BUILTIN.into(),
                description: "The shipped settings: bounded variations, no symmetry".into(),
                author: String::new(),
            },
            warnings: Vec::new(),
        });
    };
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut loaded = parse(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
    loaded.meta.name = entry.name.clone();
    Ok(loaded)
}

/// `resolve` then `load`.
pub fn open(arg: &str) -> Result<(Entry, Loaded), String> {
    let entry = resolve(arg)?;
    let loaded = load(&entry)?;
    Ok((entry, loaded))
}

/// A name a profile can be saved under, tidied (spaces become dashes), or
/// why not.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim().replace(' ', "-");
    if name.is_empty() {
        return Err("a profile needs a name".into());
    }
    if is_builtin(&name) {
        return Err(format!("'{name}' is a built-in profile; pick another name"));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') || name.starts_with('.') {
        return Err("letters, digits, '-', '_' and '.' only".into());
    }
    Ok(name)
}

/// Where a profile of the person's called `name` is saved.
pub fn user_path(name: &str) -> Option<PathBuf> {
    Some(user_dir()?.join(format!("{name}.toml")))
}

/// The entry for a profile file, by where it is: the person's own if it's in
/// their folder, shared otherwise.
pub fn entry_for(path: &Path) -> Entry {
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("profile").to_string();
    let shared = user_dir().is_none_or(|d| !path.starts_with(d));
    Entry { name, path: Some(path.to_path_buf()), shared }
}

/// The profile the window's settings came from, and what it said — what the
/// window's `*` is measured against, like a scene's dirty marker.
#[derive(Clone, Debug)]
pub struct Selected {
    pub entry: Entry,
    /// What the profile says it is, from its `[meta] description`.
    pub description: String,
    /// The profile as loaded or last saved. `None` when its file has gone or
    /// no longer reads, so the settings can't be anything but changed.
    pub baseline: Option<RandomSettings>,
}

impl Selected {
    pub fn builtin() -> Self {
        let description = load(&Entry::builtin(BUILTIN)).map(|l| l.meta.description).unwrap_or_default();
        Self { entry: Entry::builtin(BUILTIN), description, baseline: Some(RandomSettings::default()) }
    }

    /// The profile prefs remember the settings came from, read again: a file
    /// by its path, a built-in by its name, else `default`.
    pub fn remembered(path: Option<&Path>, builtin: Option<&str>) -> Self {
        let entry = match (path, builtin) {
            (Some(path), _) => entry_for(path),
            (None, Some(name)) if is_builtin(name) => Entry::builtin(name),
            _ => return Self::builtin(),
        };
        let loaded = load(&entry).ok();
        let description = loaded.as_ref().map(|l| l.meta.description.clone()).unwrap_or_default();
        Self { entry, description, baseline: loaded.map(|l| l.settings) }
    }

    /// Whether `current` differs from what the profile says.
    pub fn modified(&self, current: &RandomSettings) -> bool {
        self.baseline.as_ref() != Some(current)
    }

    /// Whether Save may write it in place. The built-in can't be saved over,
    /// and a shared profile belongs to the repository — both go to Save as.
    pub fn writable(&self) -> bool {
        self.entry.path.is_some() && !self.entry.shared
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::NUM_VARIATIONS;

    #[test]
    fn settings_read_bare_or_from_prefs() {
        let bare = "transforms = [6, 3]\n[variations.spherical]\nenabled = true\n";
        let s = parse(bare).unwrap().settings;
        assert_eq!(s.transforms, [3, 6], "range put in order");
        assert!(s.variation(2).enabled, "spherical switched on");
        assert!(s.variation(3).enabled, "swirl kept its default");
        assert_eq!(s.variations.len(), NUM_VARIATIONS - 1);

        let prefs = "invert_pitch = true\n[random]\nattempts = 5\n[random.lacunarity]\non = true\nrange = [4.0, 2.0]\n";
        let l = parse(prefs).unwrap();
        assert_eq!(l.settings.attempts, 5);
        assert!(l.settings.lacunarity.on);
        assert_eq!(l.settings.lacunarity.range, [2.0, 4.0]);
        assert!(l.warnings.is_empty(), "prefs' other keys aren't the profile's business: {:?}", l.warnings);
        assert!(parse("transforms = \"many\"").is_err());
    }

    /// The file is the format: written, read back, it's the same profile,
    /// and nothing in it is reported as unknown.
    #[test]
    fn a_profile_round_trips() {
        let mut s = RandomSettings::default();
        s.transforms = [3, 4];
        s.lacunarity.on = true;
        s.variations.get_mut("julia").unwrap().weight = [0.2, 0.6];
        s.symmetry.chance = 0.3;
        s.mutation.strength = 0.7;
        s.parts.background.odds = 0.05;
        s.parts.color.on = false;
        let meta = Meta { name: "spiky".into(), description: "few maps, sharp".into(), author: "Hazel".into() };
        let text = to_toml_string(&s, &meta);
        let back = parse(&text).unwrap();
        assert_eq!(back.settings, s, "{text}");
        assert_eq!(back.meta, meta);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        // And reads like it's meant to: meta first, tidy floats, one line each.
        let at = |k: &str| text.find(k).unwrap_or_else(|| panic!("no {k}:\n{text}"));
        assert!(at("[meta]") < at("[parts]") && at("[parts]") < at("[ranges]") && at("[ranges]") < at("[mutation]"), "{text}");
        assert!(text.contains("strength = 0.7"), "{text}");
        assert!(text.contains("julia = { enabled = true, weight = [0.2, 0.6] }"), "{text}");
        assert!(text.contains("background = { on = true, odds = 0.05, reroll = 0.5 }"), "{text}");
        assert!(text.contains("color = { on = false,"), "{text}");
        assert!(at("translate =") < at("background ="), "parts in their own order:\n{text}");
    }

    /// A profile written by hand states only what it cares about.
    #[test]
    fn a_partial_profile_takes_defaults_and_reports_typos() {
        let text = "[meta]\nname = \"x\"\ncolour = 1\n[parts]\nrotate = { on = false }\nwobble = { odds = 1.0 }\n\
                    count = { odds = 0.1 }\n[ranges]\ntransforms = [4, 4]\ntransfroms = [1, 1]\n\
                    [ranges.variations]\njulia = { enabled = false }\nnot_a_variation = { enabled = true }\n\
                    [mutation]\nstrength = 2.0\n";
        let l = parse(text).unwrap();
        let d = RandomSettings::default();
        assert_eq!(l.settings.transforms, [4, 4]);
        assert_eq!(l.settings.scale, d.scale);
        assert!(!l.settings.variations["julia"].enabled);
        assert_eq!(l.settings.variations["julia"].weight, d.variations["julia"].weight);
        assert_eq!(l.settings.mutation.strength, 2.0);
        assert!(!l.settings.parts.rotate.on);
        assert_eq!(l.settings.parts.rotate.odds, d.parts.rotate.odds);
        assert_eq!(l.settings.parts.translate, d.parts.translate);
        // A part given only its odds keeps the rest as shipped.
        assert_eq!(l.settings.parts.count.odds, 0.1);
        assert_eq!(l.settings.parts.count.reroll, d.parts.count.reroll);
        assert!(l.settings.parts.count.on);
        let w = l.warnings.join("\n");
        for typo in ["meta.colour", "ranges.transfroms", "ranges.variations.not_a_variation", "parts.wobble"] {
            assert!(w.contains(typo), "{typo} not reported:\n{w}");
        }
        assert_eq!(l.warnings.len(), 4, "{w}");
    }

    /// Saving over a profile somebody annotated keeps their notes.
    #[test]
    fn saving_keeps_hand_written_comments() {
        let dir = std::env::temp_dir().join(format!("fracturize-profile-{}", std::process::id()));
        let path = dir.join("mine.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "# my favourite\n[meta]\nname = \"mine\"\n\n[ranges]\n# fewer is better\ntransforms = [2, 3] # keep it small\n",
        )
        .unwrap();
        let mut s = RandomSettings::default();
        s.transforms = [2, 4];
        save(&path, &s, &Meta { name: "mine".into(), ..Default::default() }).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert!(text.contains("# my favourite"), "{text}");
        assert!(text.contains("# fewer is better"), "{text}");
        assert!(text.contains("transforms = [2, 4] # keep it small"), "{text}");
        assert_eq!(parse(&text).unwrap().settings, s, "{text}");
    }

    /// The built-in profiles are examples people copy, so they had better
    /// read cleanly, say what they are, and each be a different profile.
    #[test]
    fn the_builtin_profiles_read_without_warnings() {
        let mut seen: Vec<RandomSettings> = vec![RandomSettings::default()];
        for (name, _) in BUILTINS {
            let l = load(&Entry::builtin(name)).unwrap_or_else(|e| panic!("{e}"));
            assert!(l.warnings.is_empty(), "{name}: {:?}", l.warnings);
            assert_eq!(l.meta.name, *name);
            assert!(!l.meta.description.is_empty(), "{name} has no description");
            assert!(!seen.contains(&l.settings), "{name} is the same as another built-in");
            assert_eq!(l.settings.clone().normalized(), l.settings, "{name} isn't normalized as written");
            seen.push(l.settings);
        }
        let names: Vec<_> = list().into_iter().take(BUILTINS.len() + 1).map(|e| e.name).collect();
        assert_eq!(names[0], BUILTIN);
        assert!(BUILTINS.iter().all(|(n, _)| names.iter().any(|m| m == n)));
    }

    /// What the presets promise, checked against what they say.
    #[test]
    fn the_presets_do_what_they_are_called() {
        use super::super::Parts;
        let get = |n: &str| load(&Entry::builtin(n)).unwrap().settings;
        let on = |s: &RandomSettings| -> Vec<&str> {
            Parts::NAMES.iter().zip(s.parts.all()).filter(|(_, p)| p.on).map(|(n, _)| *n).collect()
        };
        assert_eq!(on(&RandomSettings::default()), Parts::NAMES, "the default has every part on");

        let any = get("anything");
        assert!(any.variations.values().all(|v| v.enabled));
        assert!(any.symmetry.chance > 0.0 && any.symmetry.repeat);
        assert!(any.parts.all().iter().all(|p| p.on && p.odds > 0.0), "anything leaves a part alone");

        // On, with no chance of a group: a roll draws "no group", where off
        // would keep the base scene's.
        let plain = get("no-symmetry");
        assert!(plain.parts.symmetry.on);
        assert_eq!(plain.symmetry.chance, 0.0);
        assert_eq!(plain.parts.symmetry.odds, 0.0);
        assert_eq!(plain.variations, any.variations, "no-symmetry should be anything's range");

        assert_eq!(on(&get("colour-only")), ["color", "color_value", "background"]);
        assert_eq!(on(&get("no-colour")), ["translate", "rotate", "scale", "weight", "variations", "symmetry"]);
    }

    /// A built-in is remembered by name, so prefs bring it back after a restart.
    #[test]
    fn a_builtin_is_remembered_by_name() {
        let s = Selected::remembered(None, Some("anything"));
        assert_eq!(s.entry, Entry::builtin("anything"));
        assert!(!s.writable());
        assert!(!s.modified(&load(&Entry::builtin("anything")).unwrap().settings));
        assert_eq!(Selected::remembered(None, Some("gone")).entry.name, BUILTIN);
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name(" big sparse ").unwrap(), "big-sparse");
        assert!(check_name("default").is_err());
        assert!(check_name("colour-only").is_err());
        assert!(check_name("../etc").is_err());
        assert!(check_name("").is_err());
    }

    /// Named lookup: the person's folder, then `scenes/dice`, then the
    /// built-in; a path is a path.
    #[test]
    fn profiles_resolve_by_name_or_path() {
        assert_eq!(resolve(BUILTIN).unwrap().path, None);
        assert_eq!(resolve("anything").unwrap(), Entry::builtin("anything"));
        assert!(resolve("surely-nobody-has-this-one").unwrap_err().contains("default"));
        let file = std::env::temp_dir().join(format!("fracturize-resolve-{}.toml", std::process::id()));
        std::fs::write(&file, "[ranges]\ntransforms = [3, 3]\n").unwrap();
        let (entry, loaded) = open(file.to_str().unwrap()).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(entry.path.as_deref(), Some(file.as_path()));
        assert_eq!(loaded.settings.transforms, [3, 3]);
        assert!(resolve("/no/such/profile.toml").is_err());
    }
}
