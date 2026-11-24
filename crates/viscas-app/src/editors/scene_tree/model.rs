//! Pure logic of the scene tree editor (GTK-free, unit tested): row text,
//! kind glyphs, drop-position math and drop plans, selection ordering,
//! filtering and the scene operations behind the tree's commands.

use std::collections::HashSet;

use viscas_scene::{
    Column, KindTag, Node, NodeContent, NodeId, NodeKind, Scene, SceneError, ScriptLanguage, Status,
};

use crate::fuzzy;

// ---------------------------------------------------------------------------
// Row text
// ---------------------------------------------------------------------------

/// What a row shows besides its icon and toggles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowText {
    /// The node name, or an expression preview for unnamed nodes.
    pub title: String,
    /// The title is source text (render in monospace).
    pub title_is_source: bool,
    /// Dim text after the title (`= 3`, `(x)`), if any.
    pub detail: Option<String>,
}

/// One-line, length-limited preview of source text.
pub fn one_line(text: &str, max_chars: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        flat
    } else {
        let cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// The main source text of a node, for previews.
pub fn main_source(kind: &NodeKind) -> Option<&str> {
    match kind {
        NodeKind::Variable { expr } | NodeKind::Point { expr } | NodeKind::List { expr } => {
            Some(expr)
        }
        NodeKind::Function { body, .. } => Some(body),
        NodeKind::Plot { relation } => Some(relation),
        NodeKind::Slider { value, .. } => Some(value),
        NodeKind::Script { source, .. }
        | NodeKind::Action { source, .. }
        | NodeKind::Regression { source, .. } => Some(source),
        NodeKind::Note { text } => Some(text),
        NodeKind::Folder | NodeKind::Table { .. } | NodeKind::Viewport { .. } => None,
    }
}

/// Row text for `node`; `value` is the evaluated display (`1/3`), if any.
pub fn row_text(node: &Node, value: Option<&str>) -> RowText {
    const MAX: usize = 48;
    let value_detail = value
        .filter(|v| !v.is_empty())
        .map(|v| format!("= {}", one_line(v, 24)));
    match (&node.name, &node.kind) {
        (Some(name), NodeKind::Function { params, .. }) => RowText {
            title: name.clone(),
            title_is_source: false,
            detail: Some(format!("({})", params.join(", "))),
        },
        (Some(name), NodeKind::Folder) => RowText {
            title: name.clone(),
            title_is_source: false,
            detail: (!node.children.is_empty()).then(|| node.children.len().to_string()),
        },
        (Some(name), NodeKind::Plot { relation }) => RowText {
            title: name.clone(),
            title_is_source: false,
            detail: Some(one_line(relation, 28)),
        },
        (Some(name), _) => RowText {
            title: name.clone(),
            title_is_source: false,
            detail: value_detail,
        },
        (None, kind) => match main_source(kind).map(|s| one_line(s, MAX)) {
            Some(src) if !src.is_empty() => RowText {
                title: src,
                title_is_source: !matches!(kind, NodeKind::Note { .. }),
                detail: match kind {
                    NodeKind::Variable { .. } => value_detail,
                    _ => None,
                },
            },
            _ => RowText {
                title: kind_title(kind.tag()).to_owned(),
                title_is_source: false,
                detail: None,
            },
        },
    }
}

/// Human title of a node kind.
pub fn kind_title(tag: KindTag) -> &'static str {
    match tag {
        KindTag::Folder => "Folder",
        KindTag::Variable => "Variable",
        KindTag::Function => "Function",
        KindTag::Plot => "Plot",
        KindTag::Point => "Point",
        KindTag::Slider => "Slider",
        KindTag::List => "List",
        KindTag::Table => "Table",
        KindTag::Script => "Script",
        KindTag::Note => "Note",
        KindTag::Viewport => "Viewport",
        KindTag::Action => "Action",
        KindTag::Regression => "Regression",
    }
}

/// Short glyph drawn as the row icon (folders use a themed icon). Every
/// kind gets a distinct glyph; the CSS class `kind-<tag>` colors it.
pub fn kind_glyph(tag: KindTag) -> &'static str {
    match tag {
        KindTag::Folder => "▸",
        KindTag::Variable => "x",
        KindTag::Function => "ƒ",
        KindTag::Plot => "∿",
        KindTag::Point => "◉",
        KindTag::Slider => "⊶",
        KindTag::List => "[ ]",
        KindTag::Table => "⊞",
        KindTag::Script => "{ }",
        KindTag::Note => "¶",
        KindTag::Viewport => "▭",
        KindTag::Action => "→",
        KindTag::Regression => "~",
    }
}

/// Kinds whose rows show an evaluation status dot.
pub fn has_status(tag: KindTag) -> bool {
    !matches!(tag, KindTag::Folder | KindTag::Note | KindTag::Viewport)
}

/// Status dot state of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dot {
    Ok,
    Pending,
    Error(String),
}

/// The dot for a node: blocking diagnostics win over the evaluation status.
pub fn status_dot(scene: &Scene, id: NodeId) -> Dot {
    if let Some(d) = scene.diagnostics(id).iter().find(|d| d.is_error()) {
        return Dot::Error(d.message.clone());
    }
    match scene.status(id) {
        Status::Ok => Dot::Ok,
        Status::Pending => Dot::Pending,
        Status::Error { message, .. } => Dot::Error(message.clone()),
    }
}

// ---------------------------------------------------------------------------
// Drag and drop
// ---------------------------------------------------------------------------

/// Where a dragged row lands relative to the row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPos {
    Before,
    After,
    /// As the last child (folders only).
    Into,
}

/// Drop position for pointer height `y` within a row of height `height`.
/// Folders take the middle half as "into"; other rows split in two.
pub fn drop_position(y: f64, height: f64, can_contain: bool) -> DropPos {
    let h = height.max(1.0);
    let t = (y / h).clamp(0.0, 1.0);
    if can_contain {
        if t < 0.25 {
            DropPos::Before
        } else if t > 0.75 {
            DropPos::After
        } else {
            DropPos::Into
        }
    } else if t < 0.5 {
        DropPos::Before
    } else {
        DropPos::After
    }
}

/// Where the dragged nodes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Each node right before this one (keeps their order).
    Before(NodeId),
    /// Appended to the children of this parent (`None` = top level).
    End(Option<NodeId>),
}

/// A validated drop: the nodes to move (tree order, no node inside
/// another dragged subtree) and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropPlan {
    pub ids: Vec<NodeId>,
    pub anchor: Anchor,
}

/// Why a drop is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropError {
    Nothing,
    /// The target is a dragged node or inside a dragged subtree.
    IntoItself,
    /// Moving would change nothing.
    NoOp,
}

/// The dragged nodes that are not inside another dragged node's subtree,
/// in tree order.
pub fn top_level(scene: &Scene, ids: &[NodeId]) -> Vec<NodeId> {
    let set: HashSet<NodeId> = ids.iter().copied().filter(|i| scene.contains(*i)).collect();
    scene
        .dfs_ids()
        .into_iter()
        .filter(|id| set.contains(id))
        .filter(|id| !set.iter().any(|a| scene.is_ancestor(*a, *id)))
        .collect()
}

/// Plans dropping `dragged` at `pos` relative to `target` (`None` = the
/// empty space below the last row: append at top level).
pub fn plan_drop(
    scene: &Scene,
    dragged: &[NodeId],
    target: Option<NodeId>,
    pos: DropPos,
) -> Result<DropPlan, DropError> {
    let ids = top_level(scene, dragged);
    if ids.is_empty() {
        return Err(DropError::Nothing);
    }
    let Some(target) = target.filter(|t| scene.contains(*t)) else {
        let plan = DropPlan {
            ids,
            anchor: Anchor::End(None),
        };
        return check_noop(scene, plan);
    };
    if ids
        .iter()
        .any(|d| *d == target || scene.is_ancestor(*d, target))
    {
        return Err(DropError::IntoItself);
    }
    let parent = scene.parent(target);
    let anchor = match pos {
        DropPos::Into => Anchor::End(Some(target)),
        DropPos::Before => Anchor::Before(target),
        DropPos::After => {
            // The first following sibling that is not being moved.
            let siblings = scene.children(parent);
            let at = siblings.iter().position(|s| *s == target).unwrap_or(0);
            match siblings[at + 1..].iter().find(|s| !ids.contains(s)) {
                Some(next) => Anchor::Before(*next),
                None => Anchor::End(parent),
            }
        }
    };
    check_noop(scene, DropPlan { ids, anchor })
}

/// [`plan_drop`] for a drop on a row as displayed: "after" an expanded
/// folder with children lands visually above its first child, so it means
/// "before the first child" (the indicator and the result agree).
pub fn plan_row_drop(
    scene: &Scene,
    dragged: &[NodeId],
    target: NodeId,
    pos: DropPos,
    expanded: bool,
) -> Result<DropPlan, DropError> {
    if pos == DropPos::After
        && expanded
        && let Some(&first) = scene.children(Some(target)).first()
    {
        return plan_drop(scene, dragged, Some(first), DropPos::Before);
    }
    plan_drop(scene, dragged, Some(target), pos)
}

/// Refuses plans that leave the tree as it is.
fn check_noop(scene: &Scene, plan: DropPlan) -> Result<DropPlan, DropError> {
    let mut sim = scene_order(scene, &plan);
    let before: Vec<(Option<NodeId>, Vec<NodeId>)> = sim.clone();
    apply_to_order(&mut sim, &plan);
    if sim == before {
        Err(DropError::NoOp)
    } else {
        Ok(plan)
    }
}

/// Sibling lists of every parent touched by `plan`.
fn scene_order(scene: &Scene, plan: &DropPlan) -> Vec<(Option<NodeId>, Vec<NodeId>)> {
    let mut parents: Vec<Option<NodeId>> = plan.ids.iter().map(|i| scene.parent(*i)).collect();
    parents.push(match plan.anchor {
        Anchor::Before(a) => scene.parent(a),
        Anchor::End(p) => p,
    });
    parents.sort();
    parents.dedup();
    parents
        .into_iter()
        .map(|p| (p, scene.children(p).to_vec()))
        .collect()
}

/// Applies `plan` to plain sibling lists (used for the no-op check).
fn apply_to_order(order: &mut [(Option<NodeId>, Vec<NodeId>)], plan: &DropPlan) {
    for (_, list) in order.iter_mut() {
        list.retain(|c| !plan.ids.contains(c));
    }
    match plan.anchor {
        Anchor::Before(a) => {
            if let Some((_, list)) = order.iter_mut().find(|(_, l)| l.contains(&a)) {
                let at = list.iter().position(|c| *c == a).unwrap_or(0);
                for (k, id) in plan.ids.iter().enumerate() {
                    list.insert(at + k, *id);
                }
            }
        }
        Anchor::End(p) => {
            if let Some((_, list)) = order.iter_mut().find(|(q, _)| *q == p) {
                list.extend(plan.ids.iter().copied());
            }
        }
    }
}

/// Executes a drop plan as one undo step.
pub fn apply_drop(scene: &mut Scene, plan: &DropPlan) -> Result<(), SceneError> {
    scene.transaction("Move", |s| {
        for &id in &plan.ids {
            match plan.anchor {
                Anchor::Before(a) => s.move_before(id, a)?,
                Anchor::End(p) => {
                    let index = s.children(p).iter().filter(|c| **c != id).count();
                    s.move_node(id, p, index)?;
                }
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Selection and filtering
// ---------------------------------------------------------------------------

/// The new selection order: nodes still selected keep their order, newly
/// selected ones follow in `now` order (the last one becomes primary).
pub fn merge_selection(previous: &[NodeId], now: &[NodeId]) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = previous
        .iter()
        .copied()
        .filter(|id| now.contains(id))
        .collect();
    for id in now {
        if !out.contains(id) {
            out.push(*id);
        }
    }
    out
}

/// The text a filter query is matched against.
pub fn search_text(node: &Node) -> String {
    let mut text = row_text(node, None).title;
    if node.name.is_some()
        && let Some(src) = main_source(&node.kind)
    {
        text.push(' ');
        text.push_str(&one_line(src, 64));
    }
    text.push(' ');
    text.push_str(kind_title(node.kind.tag()));
    text
}

/// Nodes matching `query`, best first (ties in tree order). Names are
/// matched first: a name hit outranks an expression or kind hit.
pub fn filter_nodes(scene: &Scene, query: &str) -> Vec<NodeId> {
    let ids = scene.dfs_ids();
    let mut scored: Vec<(i32, usize, NodeId)> = ids
        .iter()
        .enumerate()
        .filter_map(|(order, id)| {
            let node = scene.get(*id)?;
            let by_title =
                fuzzy::fuzzy_match(query, &row_text(node, None).title).map(|m| m.score + 1000);
            let by_text = fuzzy::fuzzy_match(query, &search_text(node)).map(|m| m.score);
            by_title.or(by_text).map(|s| (s, order, *id))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, id)| id).collect()
}

/// `a ▸ b` path of a node's ancestors (empty at top level).
pub fn parent_path(scene: &Scene, id: NodeId) -> String {
    let mut parts = Vec::new();
    let mut cur = scene.parent(id);
    while let Some(p) = cur {
        parts.push(scene.get(p).map(|n| n.title()).unwrap_or_default());
        cur = scene.parent(p);
    }
    parts.reverse();
    parts.join(" ▸ ")
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Node kinds offered by the "Add" menus, in menu order.
pub const ADDABLE: [KindTag; 13] = [
    KindTag::Variable,
    KindTag::Function,
    KindTag::Plot,
    KindTag::Point,
    KindTag::Slider,
    KindTag::List,
    KindTag::Table,
    KindTag::Script,
    KindTag::Note,
    KindTag::Action,
    KindTag::Regression,
    KindTag::Viewport,
    KindTag::Folder,
];

/// Parses a [`KindTag`] from its `as_str` name.
pub fn kind_from_str(s: &str) -> Option<KindTag> {
    KindTag::ALL.into_iter().find(|k| k.as_str() == s)
}

/// Default content of a new node of `kind` (free names in `scene`).
pub fn new_content(scene: &Scene, kind: KindTag) -> NodeContent {
    let name = |base: &str| scene.unique_name(base);
    match kind {
        KindTag::Folder => NodeContent::folder("Folder"),
        KindTag::Variable => NodeContent::variable(name("a"), "1"),
        KindTag::Function => NodeContent::function(name("f"), &["x"], "x^2"),
        KindTag::Plot => NodeContent::plot("y = x^2"),
        KindTag::Point => NodeContent::point(name("P"), "(1, 1)"),
        KindTag::Slider => NodeContent::slider(name("k"), "1", "-10", "10"),
        KindTag::List => NodeContent::list(name("L"), "[1, 2, 3]"),
        KindTag::Table => NodeContent::table(
            name("T"),
            vec![
                Column::new("x1", &["1", "2", "3"]),
                Column::new("y1", &["1", "4", "9"]),
            ],
        ),
        KindTag::Script => NodeContent::script(ScriptLanguage::Viscas, ""),
        KindTag::Note => NodeContent::note("Note"),
        KindTag::Viewport => NodeContent::viewport("Viewport"),
        KindTag::Action => NodeContent::action(""),
        KindTag::Regression => NodeContent::regression("", &[]),
    }
}

/// Where a new node goes: into the primary selection when it is a folder,
/// right after it otherwise, at the end of the top level without one.
pub fn insertion_point(scene: &Scene, primary: Option<NodeId>) -> (Option<NodeId>, usize) {
    match primary.and_then(|p| scene.get(p)) {
        Some(n) if matches!(n.kind, NodeKind::Folder) => (Some(n.id), n.children.len()),
        Some(n) => {
            let index = scene.index_in_parent(n.id).map_or(0, |i| i + 1);
            (n.parent, index)
        }
        None => (None, scene.roots().len()),
    }
}

/// Groups `ids` into a new folder placed where the first of them was, as
/// one undo step. Returns the folder.
pub fn group_into_folder(scene: &mut Scene, ids: &[NodeId]) -> Result<Option<NodeId>, SceneError> {
    let ids = top_level(scene, ids);
    let Some(&first) = ids.first() else {
        return Ok(None);
    };
    let parent = scene.parent(first);
    let index = scene.index_in_parent(first).unwrap_or(0);
    scene.transaction("Group into Folder", |s| {
        let folder = s.insert(parent, index, NodeContent::folder("Group"))?;
        for (k, id) in ids.iter().enumerate() {
            s.move_node(*id, Some(folder), k)?;
        }
        Ok(Some(folder))
    })
}

/// Duplicates each top-level node of `ids` (one undo step); returns the
/// copies.
pub fn duplicate_all(scene: &mut Scene, ids: &[NodeId]) -> Result<Vec<NodeId>, SceneError> {
    let ids = top_level(scene, ids);
    scene.transaction("Duplicate", |s| {
        ids.iter().map(|id| s.duplicate(*id)).collect()
    })
}

/// Renames `id` to `text` (trimmed), rewriting references for defining
/// nodes. Locked nodes are refused; clearing the name of a node others
/// use is refused. Returns the toast message (`None`: unchanged) or the
/// error to show.
pub fn rename_node(scene: &mut Scene, id: NodeId, text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    let node = scene.get(id).ok_or_else(|| "node is gone".to_owned())?;
    if node.name.as_deref().unwrap_or("") == text {
        return Ok(None);
    }
    if node.locked {
        return Err("locked — unlock it to rename".to_owned());
    }
    if text.is_empty() {
        let users = if node.defined_name().is_some() {
            scene.graph().direct_downstream(id).len()
        } else {
            0
        };
        if users > 0 {
            let s = if users == 1 { "" } else { "s" };
            return Err(format!("needs a name: {users} node{s} use it"));
        }
        return scene
            .rename(id, None)
            .map(|()| Some("Name cleared".to_owned()))
            .map_err(|e| e.to_string());
    }
    scene
        .rename_refactor(id, text)
        .map(|r| Some(rename_message(r.rewritten)))
        .map_err(|e| match e {
            SceneError::NameCapture { node, name } => {
                let who = scene
                    .get(node)
                    .map(|n| n.title())
                    .unwrap_or_else(|| node.to_string());
                format!("`{name}` would be captured in {who}")
            }
            other => other.to_string(),
        })
}

/// Toast text after a rename.
pub fn rename_message(rewritten: usize) -> String {
    match rewritten {
        0 => "Renamed".to_owned(),
        1 => "Renamed, 1 reference updated".to_owned(),
        n => format!("Renamed, {n} references updated"),
    }
}

// ---------------------------------------------------------------------------
// Incremental list updates
// ---------------------------------------------------------------------------

/// One edit of a list model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncOp<T> {
    Remove(usize),
    Insert(usize, T),
}

/// Edits turning `current` into `target`, touching few rows: items that
/// stay keep their row (and so their expansion), a moved item is removed
/// and reinserted once. Items must be unique in each list.
pub fn sync_ops<T: Clone + PartialEq>(current: &[T], target: &[T]) -> Vec<SyncOp<T>> {
    let mut ops = Vec::new();
    let mut cur: Vec<T> = current.to_vec();
    for i in (0..cur.len()).rev() {
        if !target.contains(&cur[i]) {
            cur.remove(i);
            ops.push(SyncOp::Remove(i));
        }
    }
    let mut i = 0;
    while i < target.len() {
        if i < cur.len() && cur[i] == target[i] {
            i += 1;
            continue;
        }
        let y = &target[i];
        let py = cur.iter().position(|c| c == y);
        if i < cur.len() {
            // Displaced `x`: moved far later? Remove it rather than `y`.
            let x = &cur[i];
            let px = target.iter().position(|t| t == x).unwrap_or(usize::MAX);
            if py.is_some_and(|py| px - i > py - i) {
                cur.remove(i);
                ops.push(SyncOp::Remove(i));
                continue;
            }
        }
        if let Some(py) = py {
            cur.remove(py);
            ops.push(SyncOp::Remove(py));
        }
        cur.insert(i, y.clone());
        ops.push(SyncOp::Insert(i, y.clone()));
        i += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_ops_are_correct_and_small() {
        fn apply(mut v: Vec<u32>, ops: &[SyncOp<u32>]) -> Vec<u32> {
            for op in ops {
                match op {
                    SyncOp::Remove(i) => {
                        v.remove(*i);
                    }
                    SyncOp::Insert(i, x) => v.insert(*i, *x),
                }
            }
            v
        }
        let cases: [(&[u32], &[u32]); 7] = [
            (&[1, 2, 3, 4], &[2, 3, 4, 1]),
            (&[1, 2, 3, 4], &[4, 1, 2, 3]),
            (&[1, 2, 3], &[1, 3]),
            (&[], &[5, 6]),
            (&[1, 2], &[]),
            (&[1, 2, 3], &[3, 9, 1]),
            (&[1, 2, 3, 4, 5], &[5, 4, 3, 2, 1]),
        ];
        for (cur, target) in cases {
            let ops = sync_ops(cur, target);
            assert_eq!(apply(cur.to_vec(), &ops), target, "{cur:?} -> {target:?}");
        }
        // Moving one item touches only that item.
        assert_eq!(sync_ops(&[1, 2, 3, 4], &[2, 3, 4, 1]).len(), 2);
        assert_eq!(sync_ops(&[1, 2, 3, 4], &[4, 1, 2, 3]).len(), 2);
        assert!(sync_ops(&[1, 2], &[1, 2]).is_empty());
    }

    fn scene() -> (Scene, [NodeId; 6]) {
        // a, F{ b, c }, d, G{}
        let mut s = Scene::new();
        let a = s.add(None, NodeContent::variable("a", "1")).unwrap();
        let f = s.add(None, NodeContent::folder("F")).unwrap();
        let b = s.add(Some(f), NodeContent::variable("b", "a + 1")).unwrap();
        let c = s.add(Some(f), NodeContent::plot("y = b x")).unwrap();
        let d = s.add(None, NodeContent::variable("d", "2")).unwrap();
        let g = s.add(None, NodeContent::folder("G")).unwrap();
        (s, [a, f, b, c, d, g])
    }

    #[test]
    fn drop_position_math() {
        // Plain rows split in half.
        assert_eq!(drop_position(0.0, 24.0, false), DropPos::Before);
        assert_eq!(drop_position(11.9, 24.0, false), DropPos::Before);
        assert_eq!(drop_position(12.0, 24.0, false), DropPos::After);
        assert_eq!(drop_position(30.0, 24.0, false), DropPos::After);
        // Folders: quarter bands for before/after, middle half for into.
        assert_eq!(drop_position(5.0, 24.0, true), DropPos::Before);
        assert_eq!(drop_position(6.0, 24.0, true), DropPos::Into);
        assert_eq!(drop_position(18.0, 24.0, true), DropPos::Into);
        assert_eq!(drop_position(18.5, 24.0, true), DropPos::After);
        assert_eq!(drop_position(-4.0, 24.0, true), DropPos::Before);
        // Degenerate heights never divide by zero.
        assert_eq!(drop_position(0.0, 0.0, false), DropPos::Before);
    }

    #[test]
    fn drop_plans_and_cycle_prevention() {
        let (mut s, [a, f, b, c, d, g]) = scene();
        // Into a folder.
        let plan = plan_drop(&s, &[a], Some(g), DropPos::Into).unwrap();
        assert_eq!(plan.anchor, Anchor::End(Some(g)));
        // A folder into its own child is refused, as is onto itself.
        assert_eq!(
            plan_drop(&s, &[f], Some(b), DropPos::Before),
            Err(DropError::IntoItself)
        );
        assert_eq!(
            plan_drop(&s, &[f], Some(f), DropPos::Into),
            Err(DropError::IntoItself)
        );
        // Children of a dragged folder are dropped from the list.
        let plan = plan_drop(&s, &[b, f], Some(g), DropPos::Into).unwrap();
        assert_eq!(plan.ids, vec![f]);
        // Dropping right where it is changes nothing.
        assert_eq!(
            plan_drop(&s, &[a], Some(f), DropPos::Before),
            Err(DropError::NoOp)
        );
        assert_eq!(
            plan_drop(&s, &[b], Some(c), DropPos::Before),
            Err(DropError::NoOp)
        );
        // After the last child of a folder appends to that folder.
        let plan = plan_drop(&s, &[d], Some(c), DropPos::After).unwrap();
        assert_eq!(plan.anchor, Anchor::End(Some(f)));
        apply_drop(&mut s, &plan).unwrap();
        assert_eq!(s.children(Some(f)), &[b, c, d]);
        // After `a` skips over dragged next siblings (here: a no-op).
        assert_eq!(
            plan_drop(&s, &[g, f], Some(a), DropPos::After),
            Err(DropError::NoOp)
        );
        let plan = plan_drop(&s, &[g], Some(a), DropPos::After).unwrap();
        assert_eq!(plan.anchor, Anchor::Before(f));
        // One undo step for the whole move.
        let plan = plan_drop(&s, &[b, c], None, DropPos::After).unwrap();
        apply_drop(&mut s, &plan).unwrap();
        assert_eq!(s.roots(), &[a, f, g, b, c]);
        s.undo();
        assert_eq!(s.roots(), &[a, f, g]);
        assert_eq!(s.children(Some(f)), &[b, c, d]);
    }

    #[test]
    fn rename_rules() {
        let (mut s, [a, f, b, _c, _d, _g]) = scene();
        // `b := a + 1` follows the rename of `a`.
        assert_eq!(
            rename_node(&mut s, a, " alpha "),
            Ok(Some("Renamed, 1 reference updated".into()))
        );
        assert!(
            matches!(&s.get(b).unwrap().kind, NodeKind::Variable { expr } if expr == "alpha + 1")
        );
        assert_eq!(rename_node(&mut s, a, "alpha"), Ok(None));
        // Used names cannot be cleared; labels can.
        assert!(
            rename_node(&mut s, a, "")
                .unwrap_err()
                .contains("1 node use")
        );
        assert_eq!(rename_node(&mut s, f, ""), Ok(Some("Name cleared".into())));
        // Locked nodes refuse renames.
        s.set_locked(b, true).unwrap();
        assert!(
            rename_node(&mut s, b, "beta")
                .unwrap_err()
                .contains("locked")
        );
        assert!(rename_node(&mut s, a, "1x").is_err());
    }

    #[test]
    fn drop_after_expanded_folder_goes_to_first_child() {
        let (s, [a, f, b, _c, d, g]) = scene();
        // Expanded folder with children: before its first child.
        let plan = plan_row_drop(&s, &[d], f, DropPos::After, true).unwrap();
        assert_eq!(plan.anchor, Anchor::Before(b));
        // Collapsed: after the folder among its siblings.
        let plan = plan_row_drop(&s, &[a], f, DropPos::After, false).unwrap();
        assert_eq!(plan.anchor, Anchor::Before(d));
        // Expanded but empty folder: plain "after".
        let plan = plan_row_drop(&s, &[a], g, DropPos::After, true).unwrap();
        assert_eq!(plan.anchor, Anchor::End(None));
        // Dragging the folder itself onto its own header stays refused.
        assert!(plan_row_drop(&s, &[f], f, DropPos::After, true).is_err());
    }

    #[test]
    fn drop_before_keeps_dragged_order() {
        let (mut s, [a, f, _b, _c, d, g]) = scene();
        let plan = plan_drop(&s, &[g, d], Some(a), DropPos::Before).unwrap();
        assert_eq!(plan.ids, vec![d, g]);
        apply_drop(&mut s, &plan).unwrap();
        assert_eq!(s.roots(), &[d, g, a, f]);
    }

    #[test]
    fn row_text_mapping() {
        let mut s = Scene::new();
        let a = s.add(None, NodeContent::variable("a", "1/3")).unwrap();
        let f = s
            .add(None, NodeContent::function("f", &["x", "y"], "x+y"))
            .unwrap();
        let p = s.add(None, NodeContent::plot("y  =\n sin(x)")).unwrap();
        let e = s.add(None, NodeContent::expression("2 + 3")).unwrap();
        let empty = s.add(None, NodeContent::action("")).unwrap();
        let folder = s.add(None, NodeContent::folder("Stuff")).unwrap();
        s.add(Some(folder), NodeContent::note("hi")).unwrap();

        let t = row_text(s.get(a).unwrap(), Some("1/3"));
        assert_eq!(
            (t.title.as_str(), t.detail.as_deref()),
            ("a", Some("= 1/3"))
        );
        assert!(!t.title_is_source);
        let t = row_text(s.get(f).unwrap(), None);
        assert_eq!(t.detail.as_deref(), Some("(x, y)"));
        let t = row_text(s.get(p).unwrap(), None);
        assert_eq!(t.title, "y = sin(x)");
        assert!(t.title_is_source);
        let t = row_text(s.get(e).unwrap(), Some("5"));
        assert_eq!(
            (t.title.as_str(), t.detail.as_deref()),
            ("2 + 3", Some("= 5"))
        );
        let t = row_text(s.get(empty).unwrap(), None);
        assert_eq!(t.title, "Action");
        let t = row_text(s.get(folder).unwrap(), None);
        assert_eq!(t.detail.as_deref(), Some("1"));
        assert_eq!(one_line("abcdef", 4), "abc…");
    }

    #[test]
    fn glyphs_are_distinct() {
        let glyphs: HashSet<&str> = KindTag::ALL.into_iter().map(kind_glyph).collect();
        assert_eq!(glyphs.len(), KindTag::ALL.len());
        for k in KindTag::ALL {
            assert_eq!(kind_from_str(k.as_str()), Some(k));
        }
        assert_eq!(ADDABLE.len(), KindTag::ALL.len());
    }

    #[test]
    fn selection_order_merge() {
        let [a, b, c] = [NodeId(1), NodeId(2), NodeId(3)];
        assert_eq!(merge_selection(&[b], &[a, b]), vec![b, a]);
        assert_eq!(merge_selection(&[c, a], &[a, b, c]), vec![c, a, b]);
        assert_eq!(merge_selection(&[a, b], &[b]), vec![b]);
        assert!(merge_selection(&[a], &[]).is_empty());
    }

    #[test]
    fn filter_ranks_names_first() {
        let (s, [a, _f, b, c, d, _g]) = scene();
        let hits = filter_nodes(&s, "b");
        assert_eq!(hits.first(), Some(&b));
        // The plot `y = b x` matches through its source.
        assert!(hits.contains(&c));
        assert!(filter_nodes(&s, "zzz").is_empty());
        assert_eq!(filter_nodes(&s, "d").first(), Some(&d));
        assert_eq!(filter_nodes(&s, "plot"), vec![c]);
        assert_eq!(filter_nodes(&s, "").len(), s.len());
        assert_eq!(parent_path(&s, b), "F");
        assert_eq!(parent_path(&s, a), "");
    }

    #[test]
    fn group_duplicate_and_insertion() {
        let (mut s, [a, f, b, _c, d, _g]) = scene();
        let folder = group_into_folder(&mut s, &[d, a, b]).unwrap().unwrap();
        // `a` was first in tree order: the folder takes its place.
        assert_eq!(s.roots()[0], folder);
        assert_eq!(s.children(Some(folder)), &[a, b, d]);
        assert_eq!(s.undo_label(), Some("Group into Folder"));
        s.undo();
        assert_eq!(s.roots()[0], a);
        assert_eq!(s.parent(b), Some(f));

        let copies = duplicate_all(&mut s, &[a, d]).unwrap();
        assert_eq!(copies.len(), 2);
        assert_eq!(s.get(copies[0]).unwrap().name.as_deref(), Some("a_2"));
        s.undo();
        assert_eq!(s.len(), 6);

        assert_eq!(insertion_point(&s, Some(f)), (Some(f), 2));
        assert_eq!(insertion_point(&s, Some(a)), (None, 1));
        assert_eq!(insertion_point(&s, None), (None, 4));
        let content = new_content(&s, KindTag::Variable);
        assert_eq!(content.name.as_deref(), Some("a_2"));
    }

    #[test]
    fn status_dots() {
        let mut s = Scene::new();
        let a = s.add(None, NodeContent::variable("a", "1 +")).unwrap();
        assert!(matches!(status_dot(&s, a), Dot::Error(_)));
        let b = s.add(None, NodeContent::variable("b", "1")).unwrap();
        assert_eq!(status_dot(&s, b), Dot::Pending);
        assert_eq!(rename_message(3), "Renamed, 3 references updated");
        assert!(has_status(KindTag::Plot) && !has_status(KindTag::Folder));
    }
}
