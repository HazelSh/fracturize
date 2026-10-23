//! Dice profiles as files: what the dice may do, saved, named, and shared.
//!
//! A profile reads like a scene file — a header comment, a `[meta]` table,
//! then a section per concern — so a person, an agent or a script can read
//! one, write one from scratch, or change one line of it:
//!
//! ```toml
//! [meta]
//! name = "spiky"
//! description = "Few maps, sharp folds, never smooth"
//!
//! [roll]                     # what a fresh flame may be made of
//! transforms = [2, 4]
//! lacunarity = { on = true, range = [12.0, 60.0] }
//!
//! [roll.variations]          # one line per variation
//! julia = { enabled = true, weight = [0.2, 0.6] }
//!
//! [roll.symmetry]
//! chance = 0.3
//!
//! [mutate]                   # what a mutation may change, and how hard
//! strength = 0.7
//!
//! [mutate.parts]             # one line per part: relative odds, re-roll chance
//! variations = { odds = 0.4, reroll = 0.25 }
//! ```
//!
//! Every key is optional and takes its default when left out, and a key this
//! version doesn't know is reported, not silently dropped. Two older shapes
//! still read: a bare settings table (what `--random-config` always took), and
//! any file with a `[random]` table in it — prefs.toml.
//!
//! Profiles live by name in two places: the person's own
//! (`~/.config/fracturize/dice/<name>.toml`) and the repository's shared ones
//! (`scenes/dice/<name>.toml`), the person's winning on a clash. `default` is
//! built in, and is always the shipped defaults.
//!
//! Saving over an existing file merges into it with `toml_edit`, as
//! `Scene::save` does, so comments written into a profile by hand survive the
//! window's Save.

use std::path::{Path, PathBuf};

use super::settings::RandomSettings;

/// The built-in profile: the shipped defaults, under a name that always
/// resolves and can't be saved over.
pub const BUILTIN: &str = "default";

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

/// Read a profile from TOML text: a profile, a prefs file, or a bare table.
pub fn parse(text: &str) -> Result<Loaded, String> {
    let top: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let mut meta = Meta::default();
    let (table, prefix) = if let Some(t) = top.get("random") {
        // prefs.toml: everything else in it is other preferences.
        let toml::Value::Table(t) = t else { return Err("[random] is not a table".into()) };
        (t.clone(), "random.")
    } else if ["meta", "roll", "mutate"].iter().any(|k| top.get(*k).is_some_and(toml::Value::is_table)) {
        let mut table = match top.get("roll") {
            Some(toml::Value::Table(t)) => t.clone(),
            Some(_) => return Err("[roll] is not a table".into()),
            None => toml::Table::new(),
        };
        if let Some(m) = top.get("mutate") {
            table.insert("mutate".into(), m.clone());
        }
        if let Some(toml::Value::Table(m)) = top.get("meta") {
            let text = |k: &str| m.get(k).and_then(toml::Value::as_str).unwrap_or_default().to_string();
            meta = Meta { name: text("name"), description: text("description"), author: text("author") };
            for k in m.keys().filter(|k| !["name", "description", "author"].contains(&k.as_str())) {
                warnings.push(format!("unknown key meta.{k}, ignored"));
            }
        }
        for k in top.keys().filter(|k| !["meta", "roll", "mutate"].contains(&k.as_str())) {
            warnings.push(format!("unknown key {k}, ignored (settings go under [roll] or [mutate])"));
        }
        // Report the mutate half under its own name, not as `roll.mutate`.
        (table, "roll.")
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
        unknown_keys(&table, &understood, prefix, &mut warnings);
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
        // The mutate half lives at the top of a profile, whatever the prefix.
        let path = if k == "mutate" && prefix == "roll." { "mutate".to_string() } else { format!("{prefix}{k}") };
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
        Some(mut doc) if doc.contains_key("roll") || doc.contains_key("meta") => {
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
# A Fracturize dice profile: what a random roll may make, and what a mutation
# may change. Any key left out takes its default; ranges are [min, max].
#
#   fracturize --random --random-config <name or file>
#   fracturize -s scene.toml -r sheet.png --mutations 8 --random-config <name or file>
#
# Or load it in the app's Random settings window. AGENTS.md, \"Dice profiles\",
# says what every key does.
";

/// One-line explanations written above keys in a fresh file, by dotted path.
const NOTES: &[(&str, &str)] = &[
    ("roll", "What a fresh flame may be made of."),
    ("roll.transforms", "maps per flame"),
    ("roll.scale", "each map's size, drawn log-uniformly"),
    ("roll.squash", "chance a map has one axis squashed"),
    ("roll.affine", "chance a flame has no variations at all"),
    ("roll.kinds", "different variations one flame draws from"),
    ("roll.per_map", "variations on each map, from the flame's kinds"),
    ("roll.negative", "chance a variation weight is negative"),
    ("roll.dimension", "similarity dimension bound; met by rescaling"),
    ("roll.lacunarity", "lacunarity bound; met by rolling again"),
    ("roll.attempts", "candidates tried before giving up (mutations too)"),
    ("roll.variations", "Which variations may be rolled, and their weight ranges (magnitudes)."),
    ("roll.symmetry", "Symmetry groups, and how often."),
    ("mutate", "What a mutation may change, and how hard."),
    ("mutate.strength", "scales every nudge; 1 is a noticeable step"),
    ("mutate.operators", "parts changed per mutation"),
    ("mutate.backoff", "halve the strength after this many rejections in a row; 0 = never"),
    ("mutate.steer_dimension", "rescale children onto the dimension bound, as fresh rolls are"),
    (
        "mutate.parts",
        "Per part: relative odds of being changed (0 = never), and the chance a change\n\
         re-rolls the part the way a fresh roll would, rather than nudging it.",
    ),
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
    let mutate = all.remove("mutate");
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
    let lines = |path: &str, values: Option<toml::Value>| {
        let mut t = Table::new();
        table_decor(&mut t, path);
        if let Some(toml::Value::Table(values)) = values {
            for (k, v) in &values {
                t[k.as_str()] = Item::Value(edit_value(v));
            }
        }
        t
    };

    let roll_order = [
        "transforms", "scale", "squash", "affine", "kinds", "per_map", "negative", "dimension", "lacunarity",
        "attempts",
    ];
    let mut roll = section("roll", &all, &roll_order);
    roll.set_position(1);
    let mut vars = lines("roll.variations", variations);
    vars.set_position(2);
    roll["variations"] = Item::Table(vars);
    let mut sym = match symmetry {
        Some(toml::Value::Table(t)) => {
            let order: Vec<&str> = t.keys().map(String::as_str).collect();
            section("roll.symmetry", &t, &order)
        }
        _ => Table::new(),
    };
    sym.set_position(3);
    roll["symmetry"] = Item::Table(sym);
    doc["roll"] = Item::Table(roll);

    if let Some(toml::Value::Table(mut mt)) = mutate {
        let parts = mt.remove("parts");
        let mut mutate = section("mutate", &mt, &["strength", "operators", "backoff", "steer_dimension"]);
        mutate.set_position(4);
        let mut parts = {
            // In the order they're shown, not alphabetical.
            let mut t = Table::new();
            table_decor(&mut t, "mutate.parts");
            if let Some(toml::Value::Table(p)) = parts {
                for name in super::settings::Parts::NAMES {
                    if let Some(v) = p.get(name) {
                        t[name] = Item::Value(edit_value(v));
                    }
                }
            }
            t
        };
        parts.set_position(5);
        mutate["parts"] = Item::Table(parts);
        doc["mutate"] = Item::Table(mutate);
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
            let mut it = toml_edit::InlineTable::new();
            for (k, v) in t {
                it.insert(k, edit_value(v));
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
    /// `None` for the built-in.
    pub path: Option<PathBuf>,
    /// From `scenes/dice/` rather than the person's own folder.
    pub shared: bool,
}

/// Every profile by name: the built-in first, then the person's, then the
/// shared ones not shadowed by a profile of the person's, each sorted.
pub fn list() -> Vec<Entry> {
    let mut out = vec![Entry { name: BUILTIN.into(), path: None, shared: false }];
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

/// Read an entry: the file, or the defaults for the built-in. A file's
/// `[meta] name` is overridden by its file name, which is what it's known by.
pub fn load(entry: &Entry) -> Result<Loaded, String> {
    let Some(path) = &entry.path else {
        return Ok(Loaded {
            settings: RandomSettings::default(),
            meta: Meta { name: BUILTIN.into(), description: "The shipped defaults".into(), author: String::new() },
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
    if name == BUILTIN {
        return Err(format!("'{BUILTIN}' is the built-in profile; pick another name"));
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
    /// The profile as loaded or last saved. `None` when its file has gone or
    /// no longer reads, so the settings can't be anything but changed.
    pub baseline: Option<RandomSettings>,
}

impl Selected {
    pub fn builtin() -> Self {
        Self { entry: Entry { name: BUILTIN.into(), path: None, shared: false }, baseline: Some(RandomSettings::default()) }
    }

    /// The profile prefs remember the settings came from, read again.
    pub fn remembered(path: Option<&Path>) -> Self {
        let Some(path) = path else { return Self::builtin() };
        let entry = entry_for(path);
        let baseline = load(&entry).ok().map(|l| l.settings);
        Self { entry, baseline }
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
        s.mutate.strength = 0.7;
        s.mutate.parts.background.odds = 0.05;
        let meta = Meta { name: "spiky".into(), description: "few maps, sharp".into(), author: "Hazel".into() };
        let text = to_toml_string(&s, &meta);
        let back = parse(&text).unwrap();
        assert_eq!(back.settings, s, "{text}");
        assert_eq!(back.meta, meta);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        // And reads like it's meant to: meta first, tidy floats, one line each.
        assert!(text.find("[meta]").unwrap() < text.find("[roll]").unwrap(), "{text}");
        assert!(text.contains("strength = 0.7"), "{text}");
        assert!(text.contains("julia = { enabled = true, weight = [0.2, 0.6] }"), "{text}");
        assert!(text.contains("background = { odds = 0.05, reroll = 0.5 }"), "{text}");
    }

    /// A profile written by hand states only what it cares about.
    #[test]
    fn a_partial_profile_takes_defaults_and_reports_typos() {
        let text = "[meta]\nname = \"x\"\ncolour = 1\n[roll]\ntransforms = [4, 4]\ntransfroms = [1, 1]\n\
                    [roll.variations]\njulia = { enabled = false }\nnot_a_variation = { enabled = true }\n\
                    [mutate]\nstrength = 2.0\n[mutate.parts]\nrotate = { odds = 0.0 }\nwobble = { odds = 1.0 }\n\
                    add = { odds = 0.1 }\n";
        let l = parse(text).unwrap();
        let d = RandomSettings::default();
        assert_eq!(l.settings.transforms, [4, 4]);
        assert_eq!(l.settings.scale, d.scale);
        assert!(!l.settings.variations["julia"].enabled);
        assert_eq!(l.settings.variations["julia"].weight, d.variations["julia"].weight);
        assert_eq!(l.settings.mutate.strength, 2.0);
        assert_eq!(l.settings.mutate.parts.rotate.odds, 0.0);
        assert_eq!(l.settings.mutate.parts.translate, d.mutate.parts.translate);
        // A part given only its odds keeps its shipped re-roll chance.
        assert_eq!(l.settings.mutate.parts.add.odds, 0.1);
        assert_eq!(l.settings.mutate.parts.add.reroll, d.mutate.parts.add.reroll);
        let w = l.warnings.join("\n");
        for typo in ["meta.colour", "roll.transfroms", "roll.variations.not_a_variation", "mutate.parts.wobble"] {
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
            "# my favourite\n[meta]\nname = \"mine\"\n\n[roll]\n# fewer is better\ntransforms = [2, 3] # keep it small\n",
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

    /// The profiles shipped in `scenes/dice` are examples people copy, so
    /// they had better read cleanly.
    #[test]
    fn the_shared_profiles_read_without_warnings() {
        let mut n = 0;
        for e in std::fs::read_dir(shared_dir()).unwrap().flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "toml") {
                let l = load(&entry_for(&path)).unwrap_or_else(|e| panic!("{e}"));
                assert!(l.warnings.is_empty(), "{}: {:?}", path.display(), l.warnings);
                n += 1;
            }
        }
        assert!(n >= 2, "only {n} shared profiles");
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name(" big sparse ").unwrap(), "big-sparse");
        assert!(check_name("default").is_err());
        assert!(check_name("../etc").is_err());
        assert!(check_name("").is_err());
    }

    /// Named lookup: the person's folder, then `scenes/dice`, then the
    /// built-in; a path is a path.
    #[test]
    fn profiles_resolve_by_name_or_path() {
        assert_eq!(resolve(BUILTIN).unwrap().path, None);
        assert!(resolve("surely-nobody-has-this-one").unwrap_err().contains("default"));
        let file = std::env::temp_dir().join(format!("fracturize-resolve-{}.toml", std::process::id()));
        std::fs::write(&file, "[roll]\ntransforms = [3, 3]\n").unwrap();
        let (entry, loaded) = open(file.to_str().unwrap()).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(entry.path.as_deref(), Some(file.as_path()));
        assert_eq!(loaded.settings.transforms, [3, 3]);
        assert!(resolve("/no/such/profile.toml").is_err());
    }
}
