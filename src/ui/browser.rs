//! Scenes window (Ctrl+O / B): a folder tree and places on the left, the
//! folder's scenes on the right, each row carrying a thumbnail rendered from
//! the scene's own camera.
//!
//! The model is `scene_browser` and the thumbnails are `gpu::thumbs`; this
//! draws them. Up/Down/Enter still walk and load the selection — the rows are
//! the same `browser_selected` state those keys drive — and the row the keys
//! are on scrolls itself into view.
//!
//! **Only the row under the pointer moves.** It flies its camera path (the
//! default orbit when it authors none); every other thumbnail is a still. A
//! list where everything moves is a list nobody can look at.
//!
//! Thumbnails are 16:9 because that is the frame scenes are composed for — the
//! default render is 1920x1080 — and they are never reframed: a scene whose own
//! camera makes a poor thumbnail makes the same poor first impression when it
//! opens, and the thumbnail is honest about it.

use std::path::{Path, PathBuf};

use egui::text::{LayoutJob, TextWrapping};

use crate::app::App;
use crate::gpu::thumbs::ThumbState;
use crate::scene_browser::{folder_label, same_path, Entry, Folder, Sort};
use crate::scene_source::SourceKind;

use super::hints::hinted;
use super::icons;

/// Thumbnail size in points.
const THUMB: egui::Vec2 = egui::vec2(128.0, 72.0);
const ROW_PAD: f32 = 4.0;
const ROW_H: f32 = THUMB.y + 2.0 * ROW_PAD;

enum Action {
    Open(PathBuf),
    Load(usize),
    AddPlace(PathBuf),
    RemovePlace(PathBuf),
    Refresh,
    Sort(Sort),
}

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if !app.show_browser {
        app.thumbs.set_hovered(None);
        return;
    }
    let mut open = true;
    let mut actions = Vec::new();
    let mut hovered = None;

    super::window(ctx, app, super::WindowKey::Scenes, "Scenes")
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Panel::top("fracturize_scenes_bar").show(ui, |ui| {
                top_bar(ui, app, &mut actions);
                ui.add_space(2.0);
            });
            egui::Panel::left("fracturize_scenes_nav")
                .resizable(true)
                .default_size(200.0)
                .min_size(120.0)
                .show(ui, |ui| {
                    // Places pinned under the tree, so a long tree can't
                    // scroll them out of reach.
                    egui::Panel::bottom("fracturize_scenes_places").show(ui, |ui| {
                        ui.add_space(4.0);
                        places(ui, app, &mut actions);
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("fracturize_scenes_tree_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| tree(ui, app, &mut actions));
                });
            hovered = list(ui, app, &mut actions);
        });
    app.thumbs.set_hovered(hovered.as_deref());

    super::remember(ctx, app, super::WindowKey::Scenes);

    for action in actions {
        match action {
            Action::Open(path) => app.browser_open_folder(&path),
            Action::Load(i) => {
                app.set_browser_selected(i);
                // Closes the browser itself.
                app.browser_load_selected();
            }
            Action::AddPlace(path) => app.add_browser_place(&path),
            Action::RemovePlace(path) => app.remove_browser_place(&path),
            Action::Refresh => app.browser_refresh(),
            Action::Sort(sort) => app.set_browser_sort(sort),
        }
    }
    if !open {
        app.show_browser = false;
    }
}

/// Up, refresh, where we are, and the sort.
fn top_bar(ui: &mut egui::Ui, app: &mut App, actions: &mut Vec<Action>) {
    let current = app.browser_folder_path().map(Path::to_path_buf);
    ui.horizontal(|ui| {
        let parent = current.as_deref().and_then(parent_of);
        let resp = ui.add_enabled(parent.is_some(), egui::Button::new(icons::UP));
        if hinted(resp, &mut app.ui_state, "Up to the parent folder", "click: go up one folder")
            .clicked()
            && let Some(p) = parent
        {
            actions.push(Action::Open(p));
        }
        let resp = ui.button(icons::REFRESH);
        if hinted(
            resp,
            &mut app.ui_state,
            "Read this folder again",
            "click: pick up scenes added since the folder was opened",
        )
        .clicked()
        {
            actions.push(Action::Refresh);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let sort = app.browser_sort();
            let chosen = super::radio::radio(&mut app.ui_state, "browser_sort", sort)
                .option(
                    Sort::Modified,
                    "Newest",
                    "Newest first, by when each file last changed",
                    "click: sort newest first",
                )
                .option(Sort::Name, "Name", "By the scene's name", "click: sort by name")
                .show(ui);
            if let Some(sort) = chosen {
                actions.push(Action::Sort(sort));
            }
            if let Some(path) = &current {
                ui.add(
                    egui::Label::new(egui::RichText::new(display_path(path)).monospace().weak())
                        .truncate(),
                );
            }
        });
    });
}

/// The folder above `path`. A relative path walks up its own components
/// first, so `scenes/local-llm` goes to `scenes` and then `.` rather than
/// jumping to an absolute path; past that, the absolute parent.
fn parent_of(path: &Path) -> Option<PathBuf> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => Some(p.to_path_buf()),
        Some(_) if path != Path::new(".") => Some(PathBuf::from(".")),
        _ => path.canonicalize().ok()?.parent().map(Path::to_path_buf),
    }
}

/// The folder tree. Rooted at home (or `/` for a folder outside it) and
/// always the same tree, so where you are is a highlighted row in a fixed
/// map rather than a new root every click. The branch down to the shown
/// folder is held open, a folder opens when it is shown, and the tree scrolls
/// it into view. Each node's subfolders are read the first time it is drawn.
fn tree(ui: &mut egui::Ui, app: &mut App, actions: &mut Vec<Action>) {
    ui.label(egui::RichText::new("Folders").small().weak());
    let Some(current) = app.browser_folder_path() else { return };
    let current = current.canonicalize().unwrap_or_else(|_| current.to_path_buf());
    let root = match home() {
        Some(home) if current.starts_with(&home) => home,
        _ => PathBuf::from("/"),
    };
    // A folder that has just become the shown one opens and scrolls into
    // view once, and is then the person's to close.
    let arrived = app.ui_state.browser_arrived.take_if(|p| *p == current);
    node(ui, app, &root, &current, arrived.is_some(), actions);
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).and_then(|h| h.canonicalize().ok())
}

/// A path as the top bar shows it: absolute, with home as `~`.
fn display_path(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match home() {
        Some(home) if abs.starts_with(&home) => {
            let rest = abs.strip_prefix(&home).unwrap_or(&abs);
            if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) }
        }
        _ => abs.display().to_string(),
    }
}

fn node(
    ui: &mut egui::Ui,
    app: &mut App,
    dir: &Path,
    current: &Path,
    arrived: bool,
    actions: &mut Vec<Action>,
) {
    let children = app
        .ui_state
        .browser_tree
        .entry(dir.to_path_buf())
        .or_insert_with(|| Folder::subdirs_of(dir))
        .clone();
    let is_current = dir == current;
    let name = if Some(dir) == home().as_deref() { "~".to_string() } else { folder_label(dir) };
    let mut label = |ui: &mut egui::Ui, app: &mut App| {
        let resp = ui.selectable_label(is_current, &name);
        if is_current && arrived {
            resp.scroll_to_me(Some(egui::Align::Center));
        }
        if hinted(resp, &mut app.ui_state, display_path(dir), "click: show this folder").clicked()
            && !is_current
        {
            actions.push(Action::Open(dir.to_path_buf()));
        }
    };
    if children.is_empty() {
        ui.horizontal(|ui| {
            // Laid out as `CollapsingState::show_header` lays out a header —
            // the arrow takes the full indent width, with no item spacing —
            // so a leaf's name lines up with its siblings'.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(ui.spacing().indent);
            label(ui, app);
        });
        return;
    }
    let id = ui.make_persistent_id(("fracturize_scenes_tree", dir));
    let on_path = current.starts_with(dir) && !is_current;
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, on_path);
    if on_path || (is_current && arrived) {
        state.set_open(true);
    }
    state
        .show_header(ui, |ui| label(ui, app))
        .body(|ui| {
            for child in &children {
                node(ui, app, child, current, arrived, actions);
            }
        });
}

/// Built-in places, then added ones, and a button to add the shown folder.
fn places(ui: &mut egui::Ui, app: &mut App, actions: &mut Vec<Action>) {
    ui.label(egui::RichText::new("Places").small().weak());
    let current = app.browser_folder_path().map(Path::to_path_buf);
    for place in app.browser_places() {
        let is_current = current.as_deref().is_some_and(|c| same_path(c, &place.path));
        let exists = place.path.is_dir();
        let resp = ui.add_enabled(exists, egui::Button::selectable(is_current, &place.label));
        let tooltip = if exists {
            place.path.display().to_string()
        } else {
            format!("{} doesn't exist yet", place.path.display())
        };
        let hint = if place.builtin {
            "click: show this folder"
        } else {
            "click: show this folder · right-click: remove it from places"
        };
        let resp = hinted(resp, &mut app.ui_state, tooltip, hint);
        if resp.clicked() {
            actions.push(Action::Open(place.path.clone()));
        }
        if !place.builtin {
            resp.context_menu(|ui| {
                if ui.button("Remove from places").clicked() {
                    actions.push(Action::RemovePlace(place.path.clone()));
                    ui.close();
                }
            });
        }
    }
    let Some(current) = current else { return };
    let already = app.browser_places().iter().any(|p| same_path(&p.path, &current));
    let resp = ui.add_enabled(!already, egui::Button::new(format!("{} Add this folder", icons::PLUS)));
    let tooltip = if already {
        "This folder is already in places".to_string()
    } else {
        format!("Add {} to places", current.display())
    };
    if hinted(resp, &mut app.ui_state, tooltip, "click: keep this folder in places").clicked() {
        actions.push(Action::AddPlace(current));
    }
}

/// The folder's scenes. Returns the one under the pointer, if any.
fn list(ui: &mut egui::Ui, app: &mut App, actions: &mut Vec<Action>) -> Option<PathBuf> {
    let selected = app.browser_selected();
    let scroll_to = app.ui_state.browser_scrolled_to != Some(selected);
    app.ui_state.browser_scrolled_to = Some(selected);
    let ppp = ui.ctx().pixels_per_point();
    let px = ((THUMB.x * ppp).round() as u32, (THUMB.y * ppp).round() as u32);
    let open_scene = app
        .scene_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| app.scene_png.clone());

    let (folder, thumbs, ui_state) = app.browser_parts();
    let folder = match folder {
        None => return None,
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return None;
        }
        Some(Ok(f)) => f,
    };
    if folder.entries.is_empty() {
        ui.label(egui::RichText::new("No scenes in this folder.").weak());
        return None;
    }

    // Follow the keyboard selection into view — but only on the frame it
    // moves, and only if it has left the viewport. Doing it every frame fights
    // the scrollbar: any scroll that takes the selection off-screen snaps
    // straight back, which makes the list unscrollable by hand.
    let id_salt = "fracturize_scenes_list";
    let mut area = egui::ScrollArea::vertical().id_salt(id_salt).auto_shrink([false, false]);
    if scroll_to {
        let view = ui.available_height();
        let offset = egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id(id_salt))
            .map_or(0.0, |s| s.offset.y);
        let (top, bottom) = (selected as f32 * ROW_H, (selected + 1) as f32 * ROW_H);
        if top < offset {
            area = area.vertical_scroll_offset(top);
        } else if bottom > offset + view {
            area = area.vertical_scroll_offset(bottom - view);
        }
    }

    let mut hovered = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    area.show_rows(ui, ROW_H, folder.entries.len(), |ui, range| {
        for i in range {
            let entry = &folder.entries[i];
            let is_open = open_scene.as_deref().is_some_and(|p| same_path(p, &entry.path));
            let (resp, problem) = row(ui, entry, i == selected, is_open, px, thumbs);
            let mut tooltip = entry.path.display().to_string();
            if let Some(e) = problem {
                tooltip = format!("{tooltip}\n\n{e}");
            }
            let resp = hinted(
                resp,
                ui_state,
                tooltip,
                "click: open this scene · the row under the pointer flies its camera path",
            );
            if resp.hovered() {
                hovered = Some(entry.path.clone());
            }
            if resp.clicked() {
                actions.push(Action::Load(i));
            }
        }
    });
    hovered
}

/// One scene: thumbnail on the left, three lines beside it. Also returns
/// what went wrong, if the scene didn't load or its thumbnail didn't draw.
fn row(
    ui: &mut egui::Ui,
    entry: &Entry,
    selected: bool,
    is_open: bool,
    px: (u32, u32),
    thumbs: &mut crate::gpu::thumbs::ThumbCache,
) -> (egui::Response, Option<String>) {
    let mut problem = entry.scene.as_ref().err().cloned();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals().clone();
    if selected {
        painter.rect_filled(rect, 3.0, visuals.selection.bg_fill.gamma_multiply(0.6));
    } else if resp.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }

    let thumb = egui::Rect::from_min_size(rect.min + egui::vec2(ROW_PAD, ROW_PAD), THUMB);
    match &entry.scene {
        Ok(scene) => match thumbs.get(&entry.path, entry.modified, px, scene) {
            ThumbState::Ready(id) => {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(id, thumb, uv, egui::Color32::WHITE);
            }
            // The scene's own ground until its picture lands a frame later,
            // so a row fills in rather than popping from grey.
            ThumbState::Pending => {
                let [r, g, b] = crate::palette::to_srgb8(scene.background);
                painter.rect_filled(thumb, 0.0, egui::Color32::from_rgb(r, g, b));
            }
            ThumbState::Failed(e) => {
                problem = Some(format!("No preview: {e}"));
                placeholder(&painter, thumb, &visuals, "no preview");
            }
        },
        Err(_) => placeholder(&painter, thumb, &visuals, "doesn't load"),
    }
    if entry.kind == SourceKind::Png {
        // Renders open as scenes too; say which rows are pictures rather
        // than scene files, since saving one makes a new file.
        let font = egui::FontId::proportional(10.0);
        let galley = painter.layout_no_wrap("PNG".into(), font, egui::Color32::WHITE);
        let chip = egui::Rect::from_min_size(
            thumb.left_bottom() + egui::vec2(3.0, -galley.size().y - 5.0),
            galley.size() + egui::vec2(6.0, 2.0),
        );
        painter.rect_filled(chip, 2.0, egui::Color32::from_black_alpha(190));
        painter.galley(chip.min + egui::vec2(3.0, 1.0), galley, egui::Color32::WHITE);
    }

    let x = thumb.right() + 10.0;
    let width = rect.right() - x - ROW_PAD;
    let line_h = THUMB.y / 3.0;
    let y = |k: f32| egui::pos2(x, thumb.top() + k * line_h + 2.0);
    let body = egui::TextStyle::Body.resolve(ui.style());

    let title = if is_open { format!("{} (open)", entry.title()) } else { entry.title() };
    line(&painter, y(0.0), title, body.clone(), visuals.strong_text_color(), width);
    match (&entry.scene, entry.author()) {
        (Err(e), _) => {
            let first = e.lines().next().unwrap_or_default().to_string();
            line(&painter, y(1.0), first, body, visuals.error_fg_color, width);
        }
        (Ok(_), Some(author)) => {
            line(&painter, y(1.0), format!("by {author}"), body, visuals.text_color(), width)
        }
        (Ok(_), None) => {
            line(&painter, y(1.0), "no author".into(), body, visuals.weak_text_color(), width)
        }
    }
    let age = entry.modified.map(age).unwrap_or_default();
    let meta = if age.is_empty() { entry.file_name() } else { format!("{} · {}", entry.file_name(), age) };
    line(&painter, y(2.0), meta, egui::TextStyle::Body.resolve(ui.style()), visuals.weak_text_color(), width);
    (resp, problem)
}

fn placeholder(painter: &egui::Painter, rect: egui::Rect, visuals: &egui::Visuals, text: &str) {
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
}

/// One line of text, cut off with an ellipsis at `width`.
fn line(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: String,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) {
    let mut job = LayoutJob::simple_singleline(text, font, color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.galley(pos, painter.layout_job(job), color);
}

/// How long ago a file changed, coarsely: "just now", "5 min ago", "3 days ago".
fn age(modified: std::time::SystemTime) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(modified)
        .map_or(0, |d| d.as_secs());
    let (n, unit) = match secs {
        0..60 => return "just now".into(),
        60..3600 => (secs / 60, "min"),
        3600..86_400 => (secs / 3600, "hour"),
        86_400..2_592_000 => (secs / 86_400, "day"),
        2_592_000..31_536_000 => (secs / 2_592_000, "month"),
        _ => (secs / 31_536_000, "year"),
    };
    if unit == "min" {
        format!("{n} min ago")
    } else {
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_walks_a_relative_path_before_going_absolute() {
        assert_eq!(parent_of(Path::new("scenes/local-llm")), Some(PathBuf::from("scenes")));
        assert_eq!(parent_of(Path::new("scenes")), Some(PathBuf::from(".")));
        let above_dot = parent_of(Path::new(".")).unwrap();
        assert!(above_dot.is_absolute());
    }

    #[test]
    fn ages_read_naturally() {
        let ago = |s: u64| age(std::time::SystemTime::now() - std::time::Duration::from_secs(s));
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(120), "2 min ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }
}
