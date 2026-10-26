//! Pick one of the scene's parameters: a button that opens a browsable tree,
//! with a search field on top.
//!
//! The tree is `params::catalogue`, so it names things for people —
//! *Transforms › whorl › Rotation › y* — and only offers what the scene can
//! actually take. Browse it with the mouse (every level folds open), or type:
//! any words, in any order, match against the whole name, and the tree flattens
//! into the matches with their full names. Each row shows the value the scene
//! has now. The `-S` path is in each row's tooltip for anyone who wants it, and
//! never needed.
//!
//! Generic on purpose: it returns a `Param`, and the caller decides what
//! choosing one means (the Timeline starts a track on it). `taken` greys rows
//! out in place, with the reason, rather than hiding them.

use crate::params::Param;

use super::hints::hinted;
use super::{icons, num, UiState};

/// The popup's height: the search field and a tree with room to open.
const POPUP_H: f32 = 420.0;

/// One level of the tree: its sub-groups in first-seen order, then its leaves.
#[derive(Default)]
struct Node {
    groups: Vec<(String, Node)>,
    leaves: Vec<usize>,
}

impl Node {
    fn build(params: &[Param]) -> Self {
        let mut root = Node::default();
        for (i, p) in params.iter().enumerate() {
            let mut node = &mut root;
            for g in &p.group {
                let at = match node.groups.iter().position(|(name, _)| name == g) {
                    Some(at) => at,
                    None => {
                        node.groups.push((g.clone(), Node::default()));
                        node.groups.len() - 1
                    }
                };
                node = &mut node.groups[at].1;
            }
            node.leaves.push(i);
        }
        root
    }
}

/// The picker's button and, while open, its popup. Returns the parameter
/// chosen this frame.
///
/// `taken` says why a parameter can't be chosen (`Some(reason)`), or `None`
/// when it can.
pub fn picker(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    id_salt: &str,
    button: &str,
    tooltip: &str,
    params: &[Param],
    taken: &dyn Fn(&Param) -> Option<String>,
) -> Option<Param> {
    let resp = ui.button((icons::PLUS, button));
    let resp = hinted(resp, ui_state, tooltip, "click: browse the scene's parameters");
    let filter_id = egui::Id::new(("param_picker_filter", id_salt));
    let focus_id = egui::Id::new(("param_picker_focus", id_salt));
    if resp.clicked() {
        // Typing should go straight into the search field when it opens.
        ui.data_mut(|d| d.insert_temp(focus_id, true));
    }
    let mut chosen = None;
    // Below the button if there's room, above it if not: the Timeline lives at
    // the bottom of the screen, and a tree squeezed into the strip under it is
    // a scrollbar with one row.
    // Decided here rather than left to egui's alternatives, which are judged
    // on the popup's first frame, before it has a size.
    let room_below = ui.ctx().content_rect().bottom() - resp.rect.bottom();
    let align = if room_below < POPUP_H + 20.0 { egui::RectAlign::TOP_START } else { egui::RectAlign::BOTTOM_START };
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(align)
        .align_alternatives(&[])
        .show(|ui| {
            // A fixed size from the first frame, so where it opens is decided
            // once, with the room the tree will need when its groups are open.
            ui.set_min_size(egui::vec2(340.0, POPUP_H));
            ui.set_max_width(340.0);
            let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
            ui.horizontal(|ui| {
                ui.label(icons::SEARCH);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .hint_text("type to search, or browse below")
                        .desired_width(300.0),
                );
                if ui.data_mut(|d| d.remove_temp::<bool>(focus_id)).is_some() {
                    field.request_focus();
                }
                // Enter takes the first match that can be chosen.
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !filter.trim().is_empty() {
                    chosen = params.iter().find(|p| p.matches(&filter) && taken(p).is_none()).cloned();
                }
                hinted(
                    field,
                    ui_state,
                    "Words in any order: \"petal rot\" finds the petal map's rotation. Enter takes \
                     the first match.",
                    "type to narrow the list",
                );
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([true, false]).max_height(370.0).show(ui, |ui| {
                if params.is_empty() {
                    ui.label(egui::RichText::new("This scene has nothing to offer here.").weak());
                } else if filter.trim().is_empty() {
                    let tree = Node::build(params);
                    draw_node(ui, ui_state, id_salt, &tree, params, taken, &mut chosen, 0);
                } else {
                    let hits: Vec<usize> =
                        (0..params.len()).filter(|&i| params[i].matches(&filter)).collect();
                    if hits.is_empty() {
                        ui.label(egui::RichText::new("Nothing matches.").weak());
                    }
                    for &i in hits.iter().take(300) {
                        leaf(ui, ui_state, &params[i], params[i].breadcrumb(), taken, &mut chosen);
                    }
                    if hits.len() > 300 {
                        ui.label(
                            egui::RichText::new(format!("…and {} more: keep typing", hits.len() - 300))
                                .small()
                                .weak(),
                        );
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(filter_id, filter));
            if chosen.is_some() {
                ui.data_mut(|d| d.remove_temp::<String>(filter_id));
                ui.close();
            }
        });
    chosen
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    id_salt: &str,
    node: &Node,
    params: &[Param],
    taken: &dyn Fn(&Param) -> Option<String>,
    chosen: &mut Option<Param>,
    depth: usize,
) {
    for &i in &node.leaves {
        leaf(ui, ui_state, &params[i], params[i].label.clone(), taken, chosen);
    }
    for (name, child) in &node.groups {
        // Ids from the whole path down to here, so two maps' "Rotation"
        // groups open and close independently.
        let id = egui::Id::new(("param_picker_group", id_salt, depth, name.as_str(), child.leaves.first(), child.groups.len()));
        let resp = egui::CollapsingHeader::new(name.as_str())
            .id_salt(id)
            .show(ui, |ui| draw_node(ui, ui_state, id_salt, child, params, taken, chosen, depth + 1));
        hinted(resp.header_response, ui_state, format!("{name}: click to open"), "click: open or close this group");
    }
}

fn leaf(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    p: &Param,
    text: String,
    taken: &dyn Fn(&Param) -> Option<String>,
    chosen: &mut Option<Param>,
) {
    let why_not = taken(p);
    let value = match &p.value {
        toml::Value::Float(x) => num::fixed(*x as f32, 3),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    };
    let resp = ui
        .add_enabled_ui(why_not.is_none(), |ui| {
            ui.horizontal(|ui| {
                let r = ui.selectable_label(false, text.as_str());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(value).monospace().small().weak());
                });
                r
            })
            .inner
        })
        .inner;
    let tooltip = match &why_not {
        Some(reason) => format!("{}\n\n{reason}", p.breadcrumb()),
        None => format!("{}\nnow {}\n\n(-S {})", p.breadcrumb(), p.value, p.path),
    };
    let resp = hinted(resp, ui_state, tooltip, "click: choose this");
    if resp.clicked() && why_not.is_none() {
        *chosen = Some(p.clone());
    }
}
