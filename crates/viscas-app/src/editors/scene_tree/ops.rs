//! Node operations shared by the scene tree, the inspector and the command
//! palette. Each runs on the engine (one undo step) and then updates the
//! shared selection, outside the engine borrow.

use std::rc::Rc;

use viscas_scene::{KindTag, NodeId, SceneError};

use super::model::{self, DropPlan};
use crate::commands::{Command, CommandProvider};
use crate::ctx::{AppCtx, WeakCtx};

/// Adds a node of `kind`. `parent`: `Some(p)` appends under `p` (`None`
/// inside = top level); `None` picks a place from the selection. Selects
/// and returns the new node.
pub fn add_node(ctx: &AppCtx, kind: KindTag, parent: Option<Option<NodeId>>) -> Option<NodeId> {
    let primary = ctx.selection().selected().last().copied();
    let id = ctx.engine().edit(|e| {
        let content = model::new_content(e.scene(), kind);
        let (parent, index) = match parent {
            Some(p) => (p, e.scene().children(p).len()),
            None => model::insertion_point(e.scene(), primary),
        };
        e.scene_mut()
            .begin_group(&format!("Add {}", model::kind_title(kind)));
        let r = e.add_node(parent, content).ok().inspect(|id| {
            let _ = e.scene_mut().move_node(*id, parent, index);
        });
        e.scene_mut().end_group();
        r
    })?;
    ctx.selection().set_selected([id]);
    Some(id)
}

/// Deletes the selected nodes (and their subtrees) as one undo step.
pub fn delete_selected(ctx: &AppCtx) -> Result<(), SceneError> {
    let ids = ctx.selection().selected();
    if ids.is_empty() {
        return Ok(());
    }
    ctx.engine().edit(|e| e.scene_mut().remove_many(&ids))
}

/// Duplicates the selected nodes and selects the copies.
pub fn duplicate_selected(ctx: &AppCtx) -> Result<(), SceneError> {
    let ids = ctx.selection().selected();
    if ids.is_empty() {
        return Ok(());
    }
    let copies = ctx
        .engine()
        .edit(|e| model::duplicate_all(e.scene_mut(), &ids))?;
    ctx.selection().set_selected(copies);
    Ok(())
}

/// Groups the selected nodes into a new folder and selects it.
pub fn group_selected(ctx: &AppCtx) -> Result<Option<NodeId>, SceneError> {
    let ids = ctx.selection().selected();
    let folder = ctx
        .engine()
        .edit(|e| model::group_into_folder(e.scene_mut(), &ids))?;
    if let Some(f) = folder {
        ctx.selection().set_selected([f]);
    }
    Ok(folder)
}

/// Executes a validated drop.
pub fn drop_nodes(ctx: &AppCtx, plan: &DropPlan) -> Result<(), SceneError> {
    ctx.engine()
        .edit(|e| model::apply_drop(e.scene_mut(), plan))
}

/// Renames `id` to `text`, rewriting references for defining nodes.
/// Returns the toast message, or the error to show inline.
pub fn rename(ctx: &AppCtx, id: NodeId, text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    ctx.engine()
        .edit(|e| model::rename_node(e.scene_mut(), id, text))
}

/// Palette commands for nodes ("Scene" category).
pub struct NodeCommands {
    ctx: WeakCtx,
}

impl NodeCommands {
    pub fn new(ctx: &AppCtx) -> Self {
        Self {
            ctx: ctx.downgrade(),
        }
    }
}

impl CommandProvider for NodeCommands {
    fn commands(&self) -> Vec<Command> {
        let Some(ctx) = self.ctx.upgrade() else {
            return Vec::new();
        };
        let has_selection = !ctx.selection().selected().is_empty();
        let mut out = Vec::new();
        let mut push = |id: String, title: String, shortcut: Option<&str>, run: Rc<dyn Fn()>| {
            out.push(Command {
                id,
                title,
                category: "Scene",
                shortcut: shortcut.map(str::to_owned),
                run,
            });
        };
        for kind in model::ADDABLE {
            let weak = self.ctx.clone();
            push(
                format!("scene.add::{}", kind.as_str()),
                format!("Add {}", model::kind_title(kind)),
                None,
                Rc::new(move || {
                    if let Some(ctx) = weak.upgrade() {
                        add_node(&ctx, kind, None);
                    }
                }),
            );
        }
        if has_selection {
            type Op = fn(&AppCtx);
            let ops: [(&str, &str, &str, Op); 3] = [
                ("scene.delete", "Delete Selected Nodes", "Delete", |c| {
                    let _ = delete_selected(c);
                }),
                (
                    "scene.duplicate",
                    "Duplicate Selected Nodes",
                    "Ctrl+D",
                    |c| {
                        let _ = duplicate_selected(c);
                    },
                ),
                ("scene.group", "Group Selected into Folder", "Ctrl+G", |c| {
                    let _ = group_selected(c);
                }),
            ];
            for (id, title, keys, op) in ops {
                let weak = self.ctx.clone();
                push(
                    id.to_owned(),
                    title.to_owned(),
                    Some(keys),
                    Rc::new(move || {
                        if let Some(ctx) = weak.upgrade() {
                            op(&ctx);
                        }
                    }),
                );
            }
        }
        out
    }
}
