//! Status bar selection text: selection name · evaluation status · dirty
//! marker. Names are the display names of the scene tree (automatic
//! labels for unnamed plots) and follow renames.

use std::cell::RefCell;

use gtk::prelude::*;

use crate::ctx::{AppCtx, NodeId};
use crate::doc::DocEvent;
use crate::editors::scene_tree::model::{self, Dot};

/// Status bar text for the current selection and document.
pub fn selection_text(ctx: &AppCtx) -> String {
    let sel = ctx.selection();
    let selected = sel.selected();
    let hovered = sel.hovered();
    let modified = ctx.engine().is_modified();
    ctx.engine().with(|e| {
        let scene = e.scene();
        let name = |id: NodeId| model::display_name(scene, id);
        let mut parts: Vec<String> = Vec::new();
        match selected.as_slice() {
            [] => parts.push("Nothing selected".to_owned()),
            [one] => {
                parts.push(name(*one));
                let tag = scene.get(*one).map(|n| n.kind.tag());
                if tag.is_some_and(model::has_status) {
                    parts.push(match model::status_dot(scene, *one) {
                        Dot::Ok => "ok".to_owned(),
                        Dot::Pending => "evaluating…".to_owned(),
                        Dot::Error(msg) => format!("error: {}", model::one_line(&msg, 48)),
                    });
                }
            }
            many => parts.push(format!("{} nodes selected", many.len())),
        }
        if let Some(h) = hovered.filter(|h| !selected.contains(h)) {
            parts.push(format!("hover {}", name(h)));
        }
        if modified {
            parts.push("● modified".to_owned());
        }
        parts.join("  ·  ")
    })
}

/// Keeps `label` showing [`selection_text`].
pub fn connect(ctx: &AppCtx, label: &gtk::Label) {
    let update = {
        let ctx = ctx.downgrade();
        let label = label.downgrade();
        move || {
            if let (Some(ctx), Some(label)) = (ctx.upgrade(), label.upgrade()) {
                label.set_label(&selection_text(&ctx));
            }
        }
    };
    update();
    let u = update.clone();
    let weak = label.downgrade();
    ctx.selection().connect_changed(move |_| {
        if weak.upgrade().is_some() {
            u();
        }
    });
    let sub = ctx.engine().subscribe(move |ev| {
        if matches!(
            ev,
            DocEvent::Scene(_) | DocEvent::Document | DocEvent::Replaced
        ) {
            update();
        }
    });
    let sub = RefCell::new(Some(sub));
    label.connect_destroy(move |_| drop(sub.take()));
}
