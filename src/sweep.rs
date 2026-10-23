//! `--sweep <path>=<spec>` — vary one scene parameter across a contact sheet.
//!
//! Tier 2 of the exploration workflow. Tier 1 (`--set`, `src/set.rs`) already
//! makes a sweep a shell loop; this makes it one command with a sheet that
//! labels itself, and adds the case a shell loop is genuinely awkward for: two
//! parameters at once, varying across columns and down rows.
//!
//! Two spec forms:
//!
//!     --sweep transform.facet-1.variations.absfold=0.05:0.55   # range
//!     --sweep palette.name=ember,abyss,peacock                 # explicit list
//!     --sweep transform.a.weight+transform.b.weight=0.5:2.0    # in lockstep
//!
//! A list is checked for first, so a value containing `,` is never read as a
//! range. Values are emitted verbatim into `--set` strings and parsed by
//! `set::apply`, so the two flags cannot disagree about what `ember` means.
//!
//! Everything here is pure: no GPU, no scene, no I/O.

/// One swept axis: the paths that move together, and the values they take.
///
/// Several paths per axis because the first real use of this hit the need
/// immediately — three ornament maps whose `absfold` has to stay equal, or the
/// sweep varies one map against two fixed ones and the tiles barely differ.
/// Join them with `+`.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub paths: Vec<String>,
    pub values: Vec<String>,
}

impl Axis {
    /// The `--set` arguments that select value `i` — one per path.
    fn set_args(&self, i: usize) -> Vec<String> {
        self.paths
            .iter()
            .map(|p| format!("{}={}", p, self.values[i]))
            .collect()
    }

    /// Short human label: the last segment of the first path, which is the
    /// part that actually varies. `transform.facet-1.variations.absfold`
    /// -> `absfold`.
    fn short(&self) -> &str {
        self.suffix(1)
    }

    /// The last `n` segments of the first path (all of it, if it has fewer).
    fn suffix(&self, n: usize) -> &str {
        let first = &self.paths[0];
        match first.rmatch_indices('.').nth(n - 1) {
            Some((i, _)) => &first[i + 1..],
            None => first,
        }
    }
}

/// Labels for two axes that can be told apart. Usually the leaves, but two
/// axes can share one (`cone.color_value` against `mantle.color_value`), and
/// then a sheet labelled `color_value=… color_value=…` says nothing about
/// which is which. Lengthen both until they differ.
fn short_pair<'a>(a: &'a Axis, b: &'a Axis) -> (&'a str, &'a str) {
    let mut n = 1;
    loop {
        let (sa, sb) = (a.suffix(n), b.suffix(n));
        if sa != sb || (sa == a.paths[0] && sb == b.paths[0]) {
            return (sa, sb);
        }
        n += 1;
    }
}

/// One tile: the overrides that make it, and what to write on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    /// `--set` arguments for this tile, applied after any base `--set`.
    pub sets: Vec<String>,
    /// Short label drawn into the tile.
    pub label: String,
    /// Full description for stdout — copy-pasteable to reproduce the tile.
    pub description: String,
}

/// Parse one `--sweep path=spec`. `steps` applies to the range form only.
pub fn parse_axis(spec: &str, steps: usize) -> Result<Axis, String> {
    let (path, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("--sweep '{spec}': expected <path>=<a:b> or <path>=<a,b,c>"))?;
    let rest = rest.trim();
    let paths: Vec<String> = path
        .split('+')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if paths.is_empty() {
        return Err(format!("--sweep '{spec}': empty path"));
    }
    // Same paths `--set` takes, and the same failure mode: a typo'd section
    // or field name silently adds a key nothing reads. Share the check
    // rather than re-deriving the field lists here.
    for p in &paths {
        crate::set::validate_path(p).map_err(|e| format!("--sweep '{spec}': {e}"))?;
    }
    if rest.is_empty() {
        return Err(format!("--sweep '{spec}': no values"));
    }

    // An array range first — `[0,0,0]:[0.1,0.05,0]` walks each component —
    // because its ends contain commas that are not a list's.
    if let Some((a, b)) = split_top_level(rest, ':').and_then(|v| match v.as_slice() {
        [a, b] => Some((a.clone(), b.clone())),
        _ => None,
    }) {
        if let (Some(a), Some(b)) = (parse_num_array(&a), parse_num_array(&b)) {
            if a.len() != b.len() {
                return Err(format!(
                    "--sweep '{spec}': the two ends have {} and {} components",
                    a.len(),
                    b.len()
                ));
            }
            if steps == 0 {
                return Err("--sweep-steps must be at least 1".to_string());
            }
            let values = (0..steps)
                .map(|i| {
                    let t = if steps == 1 { 0.0 } else { i as f64 / (steps - 1) as f64 };
                    let parts: Vec<String> =
                        a.iter().zip(&b).map(|(x, y)| fmt_num(x + (y - x) * t)).collect();
                    format!("[{}]", parts.join(","))
                })
                .collect();
            return Ok(Axis { paths, values });
        }
    }

    // A list: a value that contains a comma *at the top level* is never a
    // range. Commas inside `[…]` or `{…}` belong to the value, so a list of
    // colours (`[0,0,0],[0.1,0,0.1]`) is two values, not six.
    if rest.contains(',') {
        let values: Vec<String> = split_top_level(rest, ',')
            .ok_or_else(|| format!("--sweep '{spec}': unbalanced brackets"))?
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if values.len() < 2 {
            return Err(format!(
                "--sweep '{spec}': a list needs at least two values"
            ));
        }
        return Ok(Axis { paths, values });
    }

    if let Some((a, b)) = rest.split_once(':') {
        if steps == 0 {
            return Err("--sweep-steps must be at least 1".to_string());
        }
        let a: f64 = a.trim().parse().map_err(|_| {
            format!("--sweep '{spec}': '{}' is not a number (a range is a:b)", a.trim())
        })?;
        let b: f64 = b.trim().parse().map_err(|_| {
            format!("--sweep '{spec}': '{}' is not a number (a range is a:b)", b.trim())
        })?;
        let values = (0..steps)
            .map(|i| {
                let t = if steps == 1 { 0.0 } else { i as f64 / (steps - 1) as f64 };
                fmt_num(a + (b - a) * t)
            })
            .collect();
        return Ok(Axis { paths, values });
    }

    Err(format!(
        "--sweep '{spec}': expected a range (0.1:0.5) or a list (a,b,c); \
         for a single value use --set"
    ))
}

/// Split on `sep` where it is not inside brackets, braces or quotes. `None`
/// when the brackets don't balance, so a malformed value is an error rather
/// than a different list.
fn split_top_level(s: &str, sep: char) -> Option<Vec<String>> {
    let (mut depth, mut quoted, mut cur, mut out) = (0i32, false, String::new(), Vec::new());
    for ch in s.chars() {
        match ch {
            '"' => quoted = !quoted,
            '[' | '{' if !quoted => depth += 1,
            ']' | '}' if !quoted => depth -= 1,
            c if c == sep && depth == 0 && !quoted => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
        cur.push(ch);
    }
    (depth == 0 && !quoted).then(|| {
        out.push(cur);
        out
    })
}

/// `[0.1, 0.2, 0.3]` as numbers, or `None` if it isn't an array of numbers.
fn parse_num_array(s: &str) -> Option<Vec<f64>> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    inner.split(',').map(|p| p.trim().parse().ok()).collect()
}

/// Lay axes out over a sheet. One axis is a row of tiles; two vary across
/// columns (the first) and down rows (the second).
///
/// Returns the tiles in row-major order, with the grid shape.
pub fn tiles(axes: &[Axis]) -> Result<(Vec<Tile>, u32, u32), String> {
    match axes {
        [] => Err("--sweep: nothing to sweep".to_string()),
        [a] => {
            let n = a.values.len();
            let tiles = (0..n)
                .map(|i| Tile {
                    sets: a.set_args(i),
                    label: format!("{}={}", a.short(), a.values[i]),
                    description: describe(&a.set_args(i)),
                })
                .collect();
            Ok((tiles, n as u32, 1))
        }
        [a, b] => {
            let (cols, rows) = (a.values.len(), b.values.len());
            let (sa, sb) = short_pair(a, b);
            let mut out = Vec::with_capacity(cols * rows);
            for r in 0..rows {
                for c in 0..cols {
                    let mut sets = a.set_args(c);
                    sets.extend(b.set_args(r));
                    out.push(Tile {
                        label: format!(
                            "{}={} {}={}",
                            sa, a.values[c], sb, b.values[r]
                        ),
                        description: describe(&sets),
                        sets,
                    });
                }
            }
            Ok((out, cols as u32, rows as u32))
        }
        _ => Err(format!(
            "--sweep: at most two axes (got {}). A third dimension isn't \
             something you can look at on a sheet.",
            axes.len()
        )),
    }
}

/// Render a tile's overrides as a copy-pasteable flag list.
fn describe(sets: &[String]) -> String {
    sets.iter()
        .map(|s| format!("--set {s}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Shortest decimal that still names the value.
///
/// Linear interpolation produces 0.30000000000000004 and 0.21666666666666667,
/// neither of which belongs in a label or a `--set`. A fixed number of decimals
/// doesn't work either: 4 is noise for 0.2166667 and loses a `point_size` of
/// 0.0016667 entirely. So: take the shortest precision that reproduces the
/// value to a relative 1e-4, which is finer than any scene parameter is
/// meaningfully authored to.
fn fmt_num(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    for prec in 0..=9 {
        let s = format!("{v:.prec$}");
        let back: f64 = s.parse().unwrap_or(f64::NAN);
        if ((back - v) / v).abs() <= 1e-4 {
            return trim(&s);
        }
    }
    trim(&format!("{v:.9}"))
}

/// Drop trailing zeros, and the point if nothing follows it.
fn trim(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let t = s.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() || t == "-" { "0".to_string() } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(spec: &str, steps: usize) -> Vec<String> {
        parse_axis(spec, steps).unwrap().values
    }

    #[test]
    fn a_range_is_inclusive_at_both_ends() {
        assert_eq!(vals("meta.haze=0:1", 5), ["0", "0.25", "0.5", "0.75", "1"]);
    }

    #[test]
    fn range_endpoints_are_exact_not_float_noise() {
        let v = vals("transform.a.variations.absfold=0.05:0.55", 4);
        assert_eq!(v.first().unwrap(), "0.05");
        assert_eq!(v.last().unwrap(), "0.55");
        // and nothing in between is a 17-digit mess
        assert!(v.iter().all(|s| s.len() <= 8), "{v:?}");
    }

    #[test]
    fn a_descending_range_works() {
        assert_eq!(vals("meta.haze=1:0", 3), ["1", "0.5", "0"]);
    }

    #[test]
    fn one_step_takes_the_start() {
        assert_eq!(vals("meta.haze=0.2:0.9", 1), ["0.2"]);
    }

    #[test]
    fn a_list_is_taken_verbatim() {
        assert_eq!(vals("palette.name=ember,abyss,peacock", 5), ["ember", "abyss", "peacock"]);
    }

    #[test]
    fn a_list_wins_over_the_range_form() {
        // contains a comma, so it is a list even though it also has a colon
        assert_eq!(vals("meta.haze=x:y,z", 5), ["x:y", "z"]);
    }

    #[test]
    fn list_entries_are_trimmed() {
        assert_eq!(vals("meta.haze= ember , abyss ", 5), ["ember", "abyss"]);
    }

    #[test]
    fn the_short_label_is_the_last_path_segment() {
        let a = parse_axis("transform.facet-1.variations.absfold=0:1", 2).unwrap();
        assert_eq!(a.short(), "absfold");
    }

    #[test]
    fn one_axis_is_a_single_row() {
        let a = parse_axis("meta.haze=0:1", 3).unwrap();
        let (t, cols, rows) = tiles(&[a]).unwrap();
        assert_eq!((cols, rows), (3, 1));
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].sets, ["meta.haze=0.5"]);
        assert_eq!(t[1].label, "haze=0.5");
    }

    #[test]
    fn two_axes_are_columns_then_rows_in_row_major_order() {
        let a = parse_axis("meta.haze=0:1", 2).unwrap();
        let b = parse_axis("meta.color_contrast=1:3", 3).unwrap();
        let (t, cols, rows) = tiles(&[a, b]).unwrap();
        assert_eq!((cols, rows), (2, 3));
        assert_eq!(t.len(), 6);
        // row 0: both haze values at the first contrast
        assert_eq!(t[0].sets, ["meta.haze=0", "meta.color_contrast=1"]);
        assert_eq!(t[1].sets, ["meta.haze=1", "meta.color_contrast=1"]);
        // row 1 moves the second axis on
        assert_eq!(t[2].sets, ["meta.haze=0", "meta.color_contrast=2"]);
        // last tile is both maxima
        assert_eq!(t[5].sets, ["meta.haze=1", "meta.color_contrast=3"]);
    }

    #[test]
    fn the_description_is_copy_pasteable() {
        let a = parse_axis("meta.haze=0:1", 2).unwrap();
        let (t, _, _) = tiles(&[a]).unwrap();
        assert_eq!(t[0].description, "--set meta.haze=0");
    }

    // --- errors ---

    #[test]
    fn a_missing_equals_is_an_error() {
        assert!(parse_axis("meta.haze", 5).unwrap_err().contains("expected <path>="));
    }

    #[test]
    fn a_bare_single_value_points_at_set() {
        let e = parse_axis("meta.haze=0.5", 5).unwrap_err();
        assert!(e.contains("--set"), "{e}");
    }

    #[test]
    fn a_non_numeric_range_is_an_error() {
        assert!(parse_axis("meta.haze=lo:hi", 5).unwrap_err().contains("not a number"));
    }

    // A misspelled section or field is the same silent-typo failure mode
    // `--set` guards against, and shares its check (`set::validate_path`)
    // rather than re-deriving it.
    #[test]
    fn a_misspelled_field_is_an_error() {
        let e = parse_axis("meta.wieght=0:1", 5).unwrap_err();
        assert!(e.contains("'wieght' is not a known meta field"), "{e}");
    }

    #[test]
    fn a_misspelled_field_in_a_lockstep_path_is_an_error() {
        let e = parse_axis("transform.a.weight+transform.b.wieght=1:2", 5).unwrap_err();
        assert!(e.contains("'wieght' is not a known transform field"), "{e}");
    }

    #[test]
    fn a_one_element_list_is_an_error() {
        assert!(parse_axis("meta.haze=x,", 5).unwrap_err().contains("at least two"));
    }

    #[test]
    fn no_values_is_an_error() {
        assert!(parse_axis("meta.haze=", 5).unwrap_err().contains("no values"));
    }

    #[test]
    fn three_axes_are_refused() {
        let a = parse_axis("meta.haze=0:1", 2).unwrap();
        let e = tiles(&[a.clone(), a.clone(), a]).unwrap_err();
        assert!(e.contains("at most two"), "{e}");
    }

    #[test]
    fn zero_steps_is_an_error() {
        assert!(parse_axis("meta.haze=0:1", 0).unwrap_err().contains("at least 1"));
    }

    #[test]
    fn several_paths_move_in_lockstep() {
        let a = parse_axis("transform.a.variations.absfold+transform.b.variations.absfold=0:1", 2).unwrap();
        assert_eq!(a.paths.len(), 2);
        let (t, cols, rows) = tiles(&[a]).unwrap();
        assert_eq!((cols, rows), (2, 1));
        // one tile sets both paths to the same value
        assert_eq!(
            t[1].sets,
            ["transform.a.variations.absfold=1", "transform.b.variations.absfold=1"]
        );
    }

    #[test]
    fn a_list_of_arrays_is_split_between_them_not_inside() {
        let a = parse_axis("meta.background=[0,0,0], [0.1,0.05,0.02]", 2).unwrap();
        assert_eq!(a.values, ["[0,0,0]", "[0.1,0.05,0.02]"]);
        // Plain lists and names are unchanged.
        let a = parse_axis("palette.name=ember,abyss,peacock", 2).unwrap();
        assert_eq!(a.values, ["ember", "abyss", "peacock"]);
        assert!(parse_axis("meta.background=[0,0,0],[0.1,0", 2).unwrap_err().contains("unbalanced"));
    }

    #[test]
    fn a_range_between_arrays_walks_each_component() {
        let a = parse_axis("meta.background=[0,0,0.2]:[0.1,0.05,0]", 3).unwrap();
        assert_eq!(a.values, ["[0,0,0.2]", "[0.05,0.025,0.1]", "[0.1,0.05,0]"]);
        assert!(parse_axis("meta.background=[0,0]:[1,1,1]", 3).unwrap_err().contains("components"));
        // A scalar range is still a scalar range.
        assert_eq!(parse_axis("meta.haze=0:1", 3).unwrap().values, ["0", "0.5", "1"]);
    }

    #[test]
    fn two_axes_with_one_leaf_are_labelled_apart() {
        let a = parse_axis("transform.cone.color_value=0:1", 2).unwrap();
        let b = parse_axis("transform.mantle.color_value=0:1", 2).unwrap();
        let (t, _, _) = tiles(&[a, b]).unwrap();
        assert_eq!(t[0].label, "cone.color_value=0 mantle.color_value=0");
        // Different leaves stay short.
        let a = parse_axis("meta.haze=0:1", 2).unwrap();
        let b = parse_axis("transform.a.weight=0:1", 2).unwrap();
        let (t, _, _) = tiles(&[a, b]).unwrap();
        assert_eq!(t[0].label, "haze=0 weight=0");
    }

    #[test]
    fn a_lockstep_axis_labels_with_the_shared_leaf() {
        let a = parse_axis("transform.a.variations.absfold+transform.b.variations.absfold=0:1", 2).unwrap();
        let (t, _, _) = tiles(&[a]).unwrap();
        assert_eq!(t[0].label, "absfold=0");
    }

    #[test]
    fn lockstep_paths_are_trimmed() {
        let a = parse_axis("  transform.a.weight + transform.b.weight = 1,2", 5).unwrap();
        assert_eq!(a.paths, ["transform.a.weight", "transform.b.weight"]);
    }

    #[test]
    fn lockstep_composes_with_a_second_axis() {
        let a = parse_axis("transform.a.weight+transform.b.weight=1:2", 2).unwrap();
        let b = parse_axis("meta.haze=0:1", 2).unwrap();
        let (t, cols, rows) = tiles(&[a, b]).unwrap();
        assert_eq!((cols, rows), (2, 2));
        // three overrides per tile: two lockstep paths plus the second axis
        assert_eq!(t[0].sets.len(), 3);
        assert!(t[0].description.starts_with("--set transform.a.weight=1 --set transform.b.weight=1"));
    }

    #[test]
    fn values_are_named_as_briefly_as_precision_allows() {
        // a repeating decimal gets enough digits to be right, and no more
        // 0.216666... needs 5 places to stay inside the tolerance; 0.383333...
        // clears it at 4. Each value gets exactly what it needs.
        let v = vals("meta.haze=0.05:0.55", 4);
        assert_eq!(v, ["0.05", "0.21667", "0.3833", "0.55"]);
    }

    #[test]
    fn small_values_keep_their_significant_digits() {
        // 4 decimal places would round these to nothing useful
        let v = vals("meta.point_size=0.001:0.003", 3);
        assert_eq!(v, ["0.001", "0.002", "0.003"]);
        let v = vals("meta.point_size=0.001:0.002", 3);
        assert_eq!(v[1], "0.0015");
    }

    #[test]
    fn round_numbers_stay_round() {
        assert_eq!(vals("meta.haze=0:10", 3), ["0", "5", "10"]);
        assert_eq!(vals("meta.haze=1:2", 2), ["1", "2"]);
    }

    #[test]
    fn negative_values_survive() {
        assert_eq!(vals("meta.haze=-1:1", 3), ["-1", "0", "1"]);
    }
}
