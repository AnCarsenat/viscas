//! Scene Tree editor (REQUIREMENTS §2): a Godot-style tree of every node.
//!
//! A [`gtk::TreeListModel`] over one [`gio::ListStore`] per expanded
//! parent, kept in sync with the scene incrementally ([`model::sync_ops`])
//! so expansion, scroll position and selection survive edits. Rows follow
//! the shared selection and hover of [`AppCtx`], and tint the upstream
//! inputs and downstream dependents of the hovered (or primary selected)
//! node (§2.1.1).

pub mod model;
pub mod ops;
mod row;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use viscas_scene::{KindTag, NodeId, NodeKind, SceneEvent};

use self::model::{DropPos, SyncOp};
use self::row::Row;
use crate::ctx::AppCtx;
use crate::doc::{DocEvent, Subscription};
use crate::editors::pick_icon;

const DROP_CLASSES: [&str; 3] = ["drop-before", "drop-after", "drop-into"];
const DEP_CLASSES: [&str; 3] = ["dep-focus", "dep-up", "dep-down"];
type ActionFn = Rc<dyn Fn(&SceneTree, Option<&glib::Variant>)>;
const TOAST_TIME: Duration = Duration::from_millis(2600);

/// Weak stores per parent (`None` = top level). Several may be alive for a
/// parent: GTK may ask for a child model just to test expandability.
type Stores = Rc<RefCell<HashMap<Option<NodeId>, Vec<glib::WeakRef<gio::ListStore>>>>>;

/// Item key: the node id, plus `+` for non-folders that have children
/// (expandability is fixed per row, so gaining children recreates it).
fn item_key(scene: &viscas_scene::Scene, id: NodeId) -> String {
    match scene.get(id) {
        Some(n) if !matches!(n.kind, NodeKind::Folder) && !n.children.is_empty() => {
            format!("{}+", id.0)
        }
        _ => id.0.to_string(),
    }
}

fn key_id(key: &str) -> Option<NodeId> {
    key.trim_end_matches('+').parse().ok().map(NodeId)
}

fn obj_id(obj: &glib::Object) -> Option<NodeId> {
    let row = obj.downcast_ref::<gtk::TreeListRow>();
    let item = match row {
        Some(r) => r.item()?,
        None => obj.clone(),
    };
    key_id(&item.downcast_ref::<gtk::StringObject>()?.string())
}

fn store_keys(store: &gio::ListStore) -> Vec<String> {
    (0..store.n_items())
        .filter_map(|i| store.item(i).and_downcast::<gtk::StringObject>())
        .map(|s| s.string().to_string())
        .collect()
}

fn apply_sync(store: &gio::ListStore, target: &[String]) {
    for op in model::sync_ops(&store_keys(store), target) {
        match op {
            SyncOp::Remove(i) => store.remove(i as u32),
            SyncOp::Insert(i, k) => store.insert(i as u32, &gtk::StringObject::new(&k)),
        }
    }
}

struct Inner {
    ctx: AppCtx,
    root: gtk::Box,
    list: gtk::ListView,
    tree: gtk::TreeListModel,
    selection: gtk::MultiSelection,
    roots: gio::ListStore,
    stores: Stores,
    /// Flat filter results instead of the tree.
    flat: Rc<Cell<bool>>,
    filter: RefCell<String>,
    content: gtk::Stack,
    empty_label: gtk::Label,
    rows: RefCell<HashMap<usize, Rc<Row>>>,
    bound: RefCell<HashMap<NodeId, Vec<Rc<Row>>>>,
    /// Set while the tree pushes selection to the model or back.
    syncing: Cell<bool>,
    drag_ids: RefCell<Vec<NodeId>>,
    menu: gtk::PopoverMenu,
    toast: gtk::Revealer,
    toast_label: gtk::Label,
    toast_timer: RefCell<Option<glib::SourceId>>,
    /// Upstream / downstream of the focus node.
    tint: RefCell<(Option<NodeId>, HashSet<NodeId>, HashSet<NodeId>)>,
    /// Expanded nodes saved while filtering.
    saved_expanded: RefCell<Vec<NodeId>>,
    sub: RefCell<Option<Subscription>>,
}

/// The scene tree editor. Cloning shares it.
#[derive(Clone)]
pub struct SceneTree {
    inner: Rc<Inner>,
}

impl SceneTree {
    pub fn new(ctx: &AppCtx) -> SceneTree {
        let stores: Stores = Rc::default();
        let flat = Rc::new(Cell::new(false));
        let roots = gio::ListStore::new::<gtk::StringObject>();
        let engine = ctx.engine().clone();
        let tree = {
            let stores = stores.clone();
            let flat = flat.clone();
            gtk::TreeListModel::new(roots.clone(), false, false, move |obj| {
                if flat.get() {
                    return None;
                }
                let id = obj_id(obj)?;
                let keys = engine.with(|e| {
                    let s = e.scene();
                    let n = s.get(id)?;
                    let folder = matches!(n.kind, NodeKind::Folder);
                    (folder || !n.children.is_empty()).then(|| {
                        n.children
                            .iter()
                            .map(|c| item_key(s, *c))
                            .collect::<Vec<_>>()
                    })
                })?;
                let store = gio::ListStore::new::<gtk::StringObject>();
                for k in keys {
                    store.append(&gtk::StringObject::new(&k));
                }
                let mut map = stores.borrow_mut();
                let list = map.entry(Some(id)).or_default();
                list.retain(|w| w.upgrade().is_some());
                list.push(store.downgrade());
                Some(store.upcast())
            })
        };
        let selection = gtk::MultiSelection::new(Some(tree.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .css_classes(["scene-tree"])
            .vexpand(true)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let empty_label = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["tree-empty"])
            .valign(gtk::Align::Center)
            .build();
        let content = gtk::Stack::builder().vexpand(true).build();
        content.add_named(&scrolled, Some("list"));
        content.add_named(&empty_label, Some("empty"));

        let toast_label = gtk::Label::builder().css_classes(["tree-toast"]).build();
        let toast = gtk::Revealer::builder()
            .child(&toast_label)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(8)
            .can_target(false)
            .build();
        let overlay = gtk::Overlay::builder()
            .child(&content)
            .vexpand(true)
            .build();
        overlay.add_overlay(&toast);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["scene-tree-editor"])
            .build();
        let menu = gtk::PopoverMenu::builder()
            .has_arrow(false)
            .halign(gtk::Align::Start)
            .build();

        let tree_ed = SceneTree {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                root: root.clone(),
                list,
                tree,
                selection,
                roots,
                stores,
                flat,
                filter: RefCell::default(),
                content,
                empty_label,
                rows: RefCell::default(),
                bound: RefCell::default(),
                syncing: Cell::new(false),
                drag_ids: RefCell::default(),
                menu,
                toast,
                toast_label,
                toast_timer: RefCell::default(),
                tint: RefCell::default(),
                saved_expanded: RefCell::default(),
                sub: RefCell::default(),
            }),
        };
        root.append(&tree_ed.build_toolbar());
        root.append(&overlay);
        root.append(&tree_ed.inner.menu);
        tree_ed.install_factory(&factory);
        tree_ed.install_actions();
        tree_ed.install_keys();
        tree_ed.install_list_signals();
        tree_ed.install_listeners();
        tree_ed.sync_all();
        tree_ed.expand_all();
        tree_ed.apply_selection_from_ctx();
        tree_ed.update_tint();
        tree_ed
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.inner)
    }

    fn with_weak(weak: &Weak<Inner>, f: impl FnOnce(&SceneTree)) {
        if let Some(inner) = weak.upgrade() {
            f(&SceneTree { inner });
        }
    }

    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    fn build_toolbar(&self) -> gtk::Box {
        let bar = gtk::Box::builder()
            .spacing(2)
            .css_classes(["tree-toolbar"])
            .build();
        let add_menu = gio::Menu::new();
        for kind in model::ADDABLE {
            add_menu.append(
                Some(model::kind_title(kind)),
                Some(&format!("tree.add::{}", kind.as_str())),
            );
        }
        let add = gtk::MenuButton::builder()
            .icon_name(pick_icon(&["list-add-symbolic"]))
            .tooltip_text("Add Node")
            .menu_model(&add_menu)
            .css_classes(["tree-tool"])
            .build();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Filter nodes")
            .hexpand(true)
            .css_classes(["tree-filter"])
            .build();
        let weak = self.downgrade();
        search.connect_search_changed(move |s| {
            let text = s.text().to_string();
            Self::with_weak(&weak, |t| t.set_filter(&text));
        });
        let tool = |icons: &'static [&'static str], tip: &str, action: &str| {
            gtk::Button::builder()
                .icon_name(pick_icon(icons))
                .tooltip_text(tip)
                .action_name(action)
                .css_classes(["tree-tool"])
                .build()
        };
        bar.append(&add);
        bar.append(&search);
        bar.append(&tool(
            &["view-list-tree-symbolic", "pan-down-symbolic"],
            "Expand All",
            "tree.expand-all",
        ));
        bar.append(&tool(
            &["view-list-bullet-symbolic", "pan-end-symbolic"],
            "Collapse All",
            "tree.collapse-all",
        ));
        bar
    }

    fn install_factory(&self, factory: &gtk::SignalListItemFactory) {
        let weak = self.downgrade();
        factory.connect_setup(move |_, obj| {
            let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = Rc::new(Row::new());
            item.set_child(Some(&row.expander));
            Self::with_weak(&weak, |t| {
                t.inner
                    .rows
                    .borrow_mut()
                    .insert(row.expander.as_ptr() as usize, row.clone());
                t.wire_row(&row);
            });
        });
        let weak = self.downgrade();
        factory.connect_teardown(move |_, obj| {
            let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some(child) = item.child() {
                Self::with_weak(&weak, |t| {
                    t.inner.rows.borrow_mut().remove(&(child.as_ptr() as usize));
                });
            }
        });
        let weak = self.downgrade();
        factory.connect_bind(move |_, obj| {
            let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            Self::with_weak(&weak, |t| t.bind(item));
        });
        let weak = self.downgrade();
        factory.connect_unbind(move |_, obj| {
            let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            Self::with_weak(&weak, |t| t.unbind(item));
        });
    }

    fn row_of(&self, item: &gtk::ListItem) -> Option<Rc<Row>> {
        let child = item.child()?;
        self.inner
            .rows
            .borrow()
            .get(&(child.as_ptr() as usize))
            .cloned()
    }

    fn bind(&self, item: &gtk::ListItem) {
        let Some(row) = self.row_of(item) else {
            return;
        };
        let list_row = item.item().and_downcast::<gtk::TreeListRow>();
        let id = list_row.as_ref().and_then(|r| obj_id(r.upcast_ref()));
        row.expander.set_list_row(list_row.as_ref());
        *row.list_row.borrow_mut() = list_row;
        row.id.set(id);
        if let Some(id) = id {
            self.inner
                .bound
                .borrow_mut()
                .entry(id)
                .or_default()
                .push(row.clone());
        }
        self.inner.ctx.engine().with(|e| row.refresh(e));
        self.apply_row_classes(&row);
    }

    fn unbind(&self, item: &gtk::ListItem) {
        let Some(row) = self.row_of(item) else {
            return;
        };
        if let Some(id) = row.id.take() {
            let mut bound = self.inner.bound.borrow_mut();
            if let Some(list) = bound.get_mut(&id) {
                list.retain(|r| !Rc::ptr_eq(r, &row));
                if list.is_empty() {
                    bound.remove(&id);
                }
            }
        }
        if row.is_renaming() {
            row.end_rename();
        }
        row.set_class_from(&DROP_CLASSES, None);
        row.expander.set_list_row(None);
        row.list_row.borrow_mut().take();
    }

    /// Controllers and signals of a row (connected once per row widget).
    fn wire_row(&self, row: &Rc<Row>) {
        let w = Rc::downgrade(row);
        let weak = self.downgrade();
        let with_row = move |f: &dyn Fn(&SceneTree, &Row)| {
            if let (Some(t), Some(r)) = (weak.upgrade(), w.upgrade()) {
                f(&SceneTree { inner: t }, &r);
            }
        };
        let with_row = Rc::new(with_row);

        // Hover.
        let motion = gtk::EventControllerMotion::new();
        let wr = with_row.clone();
        motion.connect_enter(move |_, _, _| {
            wr(&|t, r| {
                if let Some(id) = r.id.get() {
                    t.inner.ctx.selection().set_hovered(Some(id));
                }
            });
        });
        let wr = with_row.clone();
        motion.connect_leave(move |_| {
            wr(&|t, r| {
                let sel = t.inner.ctx.selection();
                if r.id.get().is_some() && sel.hovered() == r.id.get() {
                    sel.set_hovered(None);
                }
            });
        });
        row.expander.add_controller(motion);

        // Toggles.
        let wr = with_row.clone();
        row.eye.connect_clicked(move |_| {
            wr(&|t, r| {
                if let Some(id) = r.id.get() {
                    t.inner.ctx.engine().edit(|e| {
                        let v = e.scene().get(id).is_some_and(|n| n.visible);
                        let _ = e.scene_mut().set_visible(id, !v);
                    });
                }
            });
        });
        let wr = with_row.clone();
        row.lock.connect_clicked(move |_| {
            wr(&|t, r| {
                if let Some(id) = r.id.get() {
                    t.inner.ctx.engine().edit(|e| {
                        let v = e.scene().get(id).is_some_and(|n| n.locked);
                        let _ = e.scene_mut().set_locked(id, !v);
                    });
                }
            });
        });

        // Rename entry.
        let wr = with_row.clone();
        row.entry
            .connect_activate(move |_| wr(&|t, r| t.commit_rename(r)));
        let keys = gtk::EventControllerKey::new();
        let wr = with_row.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                wr(&|t, r| {
                    r.end_rename();
                    t.inner.list.grab_focus();
                });
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        row.entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let wr = with_row.clone();
        focus.connect_leave(move |_| {
            wr(&|_, r| {
                // Leaving with an error cancels; otherwise commit.
                if r.is_renaming() {
                    let r2 = r.entry.clone();
                    r2.emit_activate();
                    if r.is_renaming() {
                        r.end_rename();
                    }
                }
            });
        });
        row.entry.add_controller(focus);

        // Context menu.
        let click = gtk::GestureClick::builder().button(3).build();
        let wr = with_row.clone();
        click.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            wr(&|t, r| t.popup_menu(r, x, y));
        });
        row.expander.add_controller(click);

        // Drag source.
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        let wr = with_row.clone();
        source.connect_prepare(move |_, _, _| {
            let ok = Cell::new(false);
            wr(&|t, r| {
                if let Some(id) = r.id.get()
                    && !r.is_renaming()
                {
                    let sel = t.inner.ctx.selection().selected();
                    let ids = if sel.contains(&id) { sel } else { vec![id] };
                    *t.inner.drag_ids.borrow_mut() = ids;
                    ok.set(true);
                }
            });
            ok.get()
                .then(|| gdk::ContentProvider::for_value(&"viscas-nodes".to_value()))
        });
        let wr = with_row.clone();
        source.connect_drag_begin(move |src, _| {
            wr(&|_, r| {
                let paintable = gtk::WidgetPaintable::new(Some(&r.expander));
                src.set_icon(Some(&paintable), 12, 10);
            });
        });
        let wr = with_row.clone();
        source.connect_drag_end(move |_, _, _| {
            wr(&|t, _| t.inner.drag_ids.borrow_mut().clear());
        });
        row.expander.add_controller(source);

        // Drop target.
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let wr = with_row.clone();
        target.connect_motion(move |_, _, y| {
            let action = Cell::new(gdk::DragAction::empty());
            wr(&|t, r| {
                let plan = t.row_plan(r, y);
                r.set_class_from(
                    &DROP_CLASSES,
                    plan.as_ref().map(|(_, pos)| drop_class(*pos)),
                );
                if plan.is_some() {
                    action.set(gdk::DragAction::MOVE);
                }
            });
            action.get()
        });
        let wr = with_row.clone();
        target.connect_leave(move |_| wr(&|_, r| r.set_class_from(&DROP_CLASSES, None)));
        let wr = with_row;
        target.connect_drop(move |_, _, _, y| {
            let done = Cell::new(false);
            wr(&|t, r| {
                r.set_class_from(&DROP_CLASSES, None);
                if let Some((plan, _)) = t.row_plan(r, y) {
                    done.set(t.run_drop(&plan));
                }
            });
            done.get()
        });
        row.expander.add_controller(target);
    }

    /// The drop plan for the pointer at `y` over `row`.
    fn row_plan(&self, row: &Row, y: f64) -> Option<(model::DropPlan, DropPos)> {
        if self.inner.flat.get() {
            return None;
        }
        let target = row.id.get()?;
        let dragged = self.inner.drag_ids.borrow().clone();
        let h = f64::from(row.expander.height());
        self.inner.ctx.engine().with(|e| {
            let scene = e.scene();
            let folder = matches!(scene.get(target)?.kind, NodeKind::Folder);
            let pos = model::drop_position(y, h, folder);
            model::plan_drop(scene, &dragged, Some(target), pos)
                .ok()
                .map(|p| (p, pos))
        })
    }

    fn run_drop(&self, plan: &model::DropPlan) -> bool {
        let ids = plan.ids.clone();
        match ops::drop_nodes(&self.inner.ctx, plan) {
            Ok(()) => {
                if let model::Anchor::End(Some(folder)) = plan.anchor {
                    self.expand_node(folder);
                }
                self.inner.ctx.selection().set_selected(ids);
                true
            }
            Err(e) => {
                self.toast(&e.to_string());
                false
            }
        }
    }

    fn install_list_signals(&self) {
        // Double click / Enter: rename.
        let weak = self.downgrade();
        self.inner.list.connect_activate(move |_, pos| {
            Self::with_weak(&weak, |t| {
                if let Some(id) = t.id_at(pos) {
                    t.begin_rename(id);
                }
            });
        });
        let weak = self.downgrade();
        self.inner
            .selection
            .connect_selection_changed(move |_, _, _| {
                Self::with_weak(&weak, |t| t.push_selection_to_ctx());
            });
        // Drops on the empty space below the rows append at top level.
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let weak = self.downgrade();
        target.connect_drop(move |_, _, _, _| {
            let mut done = false;
            Self::with_weak(&weak, |t| {
                if t.inner.flat.get() {
                    return;
                }
                let dragged = t.inner.drag_ids.borrow().clone();
                let plan = t
                    .inner
                    .ctx
                    .engine()
                    .with(|e| model::plan_drop(e.scene(), &dragged, None, DropPos::After));
                if let Ok(plan) = plan {
                    done = t.run_drop(&plan);
                }
            });
            done
        });
        self.inner.list.add_controller(target);
    }

    fn install_listeners(&self) {
        let weak = self.downgrade();
        let sub = self.inner.ctx.engine().subscribe(move |ev| {
            Self::with_weak(&weak, |t| t.on_doc_event(ev));
        });
        *self.inner.sub.borrow_mut() = Some(sub);
        let weak = self.downgrade();
        self.inner.ctx.selection().connect_changed(move |_| {
            Self::with_weak(&weak, |t| {
                t.apply_selection_from_ctx();
                t.update_tint();
            });
        });
        let weak = self.downgrade();
        self.inner.ctx.preferences().connect_changed(move |_| {
            let Some(inner) = weak.upgrade() else {
                return false;
            };
            SceneTree { inner }.update_empty();
            true
        });
    }

    fn install_actions(&self) {
        let group = gio::SimpleActionGroup::new();
        let weak = self.downgrade();
        let add_action = |name: &str, f: ActionFn, param: bool| {
            let action = gio::SimpleAction::new(name, param.then_some(glib::VariantTy::STRING));
            let weak = weak.clone();
            action.connect_activate(move |_, v| Self::with_weak(&weak, |t| f(t, v)));
            group.add_action(&action);
        };
        let kind_of =
            |v: Option<&glib::Variant>| v.and_then(|v| v.str()).and_then(model::kind_from_str);
        add_action(
            "add",
            Rc::new(move |t, v| {
                if let Some(kind) = kind_of(v) {
                    t.add_and_rename(kind, None);
                }
            }),
            true,
        );
        add_action(
            "add-child",
            Rc::new(move |t, v| {
                let parent = t.inner.ctx.selection().selected().last().copied();
                if let (Some(kind), Some(p)) = (kind_of(v), parent) {
                    t.add_and_rename(kind, Some(Some(p)));
                }
            }),
            true,
        );
        add_action(
            "rename",
            Rc::new(|t, _| {
                if let Some(id) = t.inner.ctx.selection().selected().last().copied() {
                    t.begin_rename(id);
                }
            }),
            false,
        );
        add_action(
            "duplicate",
            Rc::new(|t, _| t.report(ops::duplicate_selected(&t.inner.ctx))),
            false,
        );
        add_action(
            "delete",
            Rc::new(|t, _| t.report(ops::delete_selected(&t.inner.ctx))),
            false,
        );
        add_action(
            "group",
            Rc::new(|t, _| match ops::group_selected(&t.inner.ctx) {
                Ok(Some(f)) => t.expand_node(f),
                Ok(None) => {}
                Err(e) => t.toast(&e.to_string()),
            }),
            false,
        );
        add_action(
            "reveal",
            Rc::new(|t, _| {
                let playful = t.inner.ctx.preferences().with(|p| p.interface.playful);
                t.toast(if playful {
                    "The Dependency Graph is still growing its wires (M4)"
                } else {
                    "Dependency Graph is not available yet"
                });
            }),
            false,
        );
        add_action("expand-all", Rc::new(|t, _| t.expand_all()), false);
        add_action("collapse-all", Rc::new(|t, _| t.collapse_all()), false);
        self.inner.root.insert_action_group("tree", Some(&group));
    }

    fn install_keys(&self) {
        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let mut handled = false;
            Self::with_weak(&weak, |t| {
                if t.renaming() {
                    return;
                }
                let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                let action = match (key, ctrl) {
                    (gdk::Key::F2, false) => "rename",
                    (gdk::Key::Delete | gdk::Key::KP_Delete, false) => "delete",
                    (gdk::Key::d | gdk::Key::D, true) => "duplicate",
                    (gdk::Key::g | gdk::Key::G, true) => "group",
                    _ => return,
                };
                handled = t
                    .inner
                    .root
                    .activate_action(&format!("tree.{action}"), None)
                    .is_ok();
            });
            if handled {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.inner.root.add_controller(keys);
    }

    // ------------------------------------------------------------------
    // Document events
    // ------------------------------------------------------------------

    fn on_doc_event(&self, ev: &DocEvent) {
        match ev {
            DocEvent::Scene(events) => {
                let structural = events.iter().any(|e| {
                    matches!(
                        e,
                        SceneEvent::NodeAdded(_)
                            | SceneEvent::NodeRemoved(_)
                            | SceneEvent::Moved { .. }
                    )
                });
                let renamed = events.iter().any(|e| {
                    matches!(
                        e,
                        SceneEvent::Changed {
                            what: viscas_scene::Change::Name | viscas_scene::Change::Kind,
                            ..
                        }
                    )
                });
                if structural || (renamed && self.inner.flat.get()) {
                    self.sync_all();
                    // New folders start expanded (Godot-like).
                    let folders: Vec<NodeId> = self.inner.ctx.engine().with(|e| {
                        events
                            .iter()
                            .filter_map(|ev| match ev {
                                SceneEvent::NodeAdded(id) => Some(*id),
                                _ => None,
                            })
                            .filter(|id| {
                                e.scene()
                                    .get(*id)
                                    .is_some_and(|n| matches!(n.kind, NodeKind::Folder))
                            })
                            .collect()
                    });
                    if !self.inner.flat.get() {
                        for f in folders {
                            self.expand_node(f);
                        }
                    }
                    self.apply_selection_from_ctx();
                    self.refresh_all();
                } else {
                    let changed: HashSet<NodeId> = events
                        .iter()
                        .filter_map(|e| match e {
                            SceneEvent::Changed { id, .. } => Some(*id),
                            _ => None,
                        })
                        .collect();
                    self.refresh_ids(&changed);
                }
                if events
                    .iter()
                    .any(|e| matches!(e, SceneEvent::GraphChanged { .. }))
                    || structural
                {
                    self.update_tint();
                }
            }
            DocEvent::Replaced => {
                self.sync_all();
                self.expand_all();
                self.update_tint();
            }
            DocEvent::Settings(_) => self.refresh_all(),
            _ => {}
        }
    }

    /// Brings every alive store in line with the scene.
    fn sync_all(&self) {
        let ctx = &self.inner.ctx;
        if self.inner.flat.get() {
            let query = self.inner.filter.borrow().clone();
            let keys: Vec<String> = ctx.engine().with(|e| {
                model::filter_nodes(e.scene(), &query)
                    .into_iter()
                    .map(|id| id.0.to_string())
                    .collect()
            });
            apply_sync(&self.inner.roots, &keys);
            self.update_empty();
            return;
        }
        let targets: Vec<(Option<NodeId>, Option<Vec<String>>)> = ctx.engine().with(|e| {
            let s = e.scene();
            let mut parents: Vec<Option<NodeId>> =
                self.inner.stores.borrow().keys().copied().collect();
            parents.push(None);
            parents
                .into_iter()
                .map(|p| {
                    let keys = match p {
                        Some(id) if !s.contains(id) => None,
                        _ => Some(s.children(p).iter().map(|c| item_key(s, *c)).collect()),
                    };
                    (p, keys)
                })
                .collect()
        });
        for (parent, keys) in targets {
            let stores: Vec<gio::ListStore> = match parent {
                None => vec![self.inner.roots.clone()],
                Some(_) => {
                    let mut map = self.inner.stores.borrow_mut();
                    let list = map.entry(parent).or_default();
                    list.retain(|w| w.upgrade().is_some());
                    let alive = list.iter().filter_map(|w| w.upgrade()).collect();
                    if keys.is_none() || list.is_empty() {
                        map.remove(&parent);
                    }
                    alive
                }
            };
            if let Some(keys) = keys {
                for store in stores {
                    apply_sync(&store, &keys);
                }
            }
        }
        self.update_empty();
    }

    fn refresh_ids(&self, ids: &HashSet<NodeId>) {
        let rows: Vec<Rc<Row>> = {
            let bound = self.inner.bound.borrow();
            ids.iter()
                .filter_map(|id| bound.get(id))
                .flatten()
                .cloned()
                .collect()
        };
        self.inner.ctx.engine().with(|e| {
            for r in &rows {
                r.refresh(e);
            }
        });
    }

    fn refresh_all(&self) {
        let rows: Vec<Rc<Row>> = self
            .inner
            .bound
            .borrow()
            .values()
            .flatten()
            .cloned()
            .collect();
        self.inner.ctx.engine().with(|e| {
            for r in &rows {
                r.refresh(e);
            }
        });
    }

    fn update_empty(&self) {
        let (n, playful) = (
            self.inner.ctx.engine().with(|e| e.scene().len()),
            self.inner.ctx.preferences().with(|p| p.interface.playful),
        );
        let filter = self.inner.filter.borrow().clone();
        let text = if n == 0 {
            if playful {
                "No nodes yet. Every tree starts as a seed.\nPlant one with +".to_owned()
            } else {
                "No nodes. Add one with +".to_owned()
            }
        } else if self.inner.flat.get() && self.inner.roots.n_items() == 0 {
            if playful {
                format!("Nothing answers to “{filter}”.")
            } else {
                format!("No node matches “{filter}”")
            }
        } else {
            String::new()
        };
        self.inner.empty_label.set_label(&text);
        self.inner
            .content
            .set_visible_child_name(if text.is_empty() { "list" } else { "empty" });
    }

    // ------------------------------------------------------------------
    // Selection, hover and dependency tint
    // ------------------------------------------------------------------

    fn id_at(&self, pos: u32) -> Option<NodeId> {
        let row = self.inner.tree.row(pos)?;
        obj_id(row.upcast_ref())
    }

    fn position_of(&self, id: NodeId) -> Option<u32> {
        (0..self.inner.tree.n_items()).find(|p| self.id_at(*p) == Some(id))
    }

    fn push_selection_to_ctx(&self) {
        if self.inner.syncing.get() {
            return;
        }
        let sel = &self.inner.selection;
        let now: Vec<NodeId> = (0..sel.n_items())
            .filter(|p| sel.is_selected(*p))
            .filter_map(|p| self.id_at(p))
            .collect();
        let ctx_sel = self.inner.ctx.selection();
        let merged = model::merge_selection(&ctx_sel.selected(), &now);
        self.inner.syncing.set(true);
        ctx_sel.set_selected(merged);
        self.inner.syncing.set(false);
        self.update_tint();
    }

    fn apply_selection_from_ctx(&self) {
        if self.inner.syncing.get() {
            return;
        }
        let wanted = self.inner.ctx.selection().selected();
        if !self.inner.flat.get() {
            for id in &wanted {
                self.reveal(*id);
            }
        }
        let n = self.inner.tree.n_items();
        let selected = gtk::Bitset::new_empty();
        let mut first = None;
        for p in 0..n {
            if let Some(id) = self.id_at(p)
                && wanted.contains(&id)
            {
                selected.add(p);
                if wanted.last() == Some(&id) {
                    first = Some(p);
                }
            }
        }
        self.inner.syncing.set(true);
        self.inner
            .selection
            .set_selection(&selected, &gtk::Bitset::new_range(0, n));
        self.inner.syncing.set(false);
        if let Some(p) = first {
            self.inner
                .list
                .scroll_to(p, gtk::ListScrollFlags::NONE, None);
        }
    }

    /// Expands the ancestors of `id`.
    fn reveal(&self, id: NodeId) {
        let ancestors: Vec<NodeId> = self.inner.ctx.engine().with(|e| {
            let s = e.scene();
            let mut v = Vec::new();
            let mut cur = s.parent(id);
            while let Some(p) = cur {
                v.push(p);
                cur = s.parent(p);
            }
            v.reverse();
            v
        });
        for a in ancestors {
            self.expand_node(a);
        }
    }

    fn expand_node(&self, id: NodeId) {
        if let Some(p) = self.position_of(id)
            && let Some(row) = self.inner.tree.row(p)
        {
            row.set_expanded(true);
        }
    }

    fn expand_all(&self) {
        let mut p = 0;
        while p < self.inner.tree.n_items() {
            if let Some(row) = self.inner.tree.row(p) {
                row.set_expanded(true);
            }
            p += 1;
        }
    }

    fn collapse_all(&self) {
        let n = self.inner.tree.n_items();
        for p in (0..n).rev() {
            if let Some(row) = self.inner.tree.row(p) {
                row.set_expanded(false);
            }
        }
    }

    fn expanded_ids(&self) -> Vec<NodeId> {
        (0..self.inner.tree.n_items())
            .filter_map(|p| self.inner.tree.row(p))
            .filter(|r| r.is_expanded())
            .filter_map(|r| obj_id(r.upcast_ref()))
            .collect()
    }

    fn set_filter(&self, text: &str) {
        let query = text.trim().to_owned();
        let was_flat = self.inner.flat.get();
        let flat = !query.is_empty();
        if flat && !was_flat {
            *self.inner.saved_expanded.borrow_mut() = self.expanded_ids();
        }
        *self.inner.filter.borrow_mut() = query;
        if flat != was_flat {
            // Rows must be recreated to change their expandability.
            self.inner.roots.remove_all();
        }
        self.inner.flat.set(flat);
        self.sync_all();
        if was_flat && !flat {
            let saved = std::mem::take(&mut *self.inner.saved_expanded.borrow_mut());
            for id in saved {
                self.expand_node(id);
            }
        }
        self.apply_selection_from_ctx();
    }

    /// Recomputes the dependency tint for the hovered (else primary
    /// selected) node and restyles the bound rows.
    fn update_tint(&self) {
        let sel = self.inner.ctx.selection();
        let focus = sel.hovered().or_else(|| sel.selected().last().copied());
        let (up, down) = self.inner.ctx.engine().with(|e| match focus {
            Some(f) if e.scene().contains(f) => (
                e.scene().upstream(f).into_iter().collect(),
                e.scene().downstream(f).into_iter().collect(),
            ),
            _ => (HashSet::new(), HashSet::new()),
        });
        *self.inner.tint.borrow_mut() = (focus, up, down);
        let rows: Vec<Rc<Row>> = self
            .inner
            .bound
            .borrow()
            .values()
            .flatten()
            .cloned()
            .collect();
        for r in rows {
            self.apply_row_classes(&r);
        }
    }

    fn apply_row_classes(&self, row: &Row) {
        let id = row.id.get();
        let class = {
            let tint = self.inner.tint.borrow();
            match id {
                Some(id) if tint.0 == Some(id) => Some("dep-focus"),
                Some(id) if tint.1.contains(&id) => Some("dep-up"),
                Some(id) if tint.2.contains(&id) => Some("dep-down"),
                _ => None,
            }
        };
        row.set_class_from(&DEP_CLASSES, class);
        let hovered = id.is_some() && self.inner.ctx.selection().hovered() == id;
        if hovered {
            row.expander.add_css_class("ext-hover");
        } else {
            row.expander.remove_css_class("ext-hover");
        }
        let tip = match class {
            Some("dep-up") => Some("Input of the highlighted node"),
            Some("dep-down") => Some("Depends on the highlighted node"),
            _ => None,
        };
        row.expander.set_tooltip_text(tip);
    }

    // ------------------------------------------------------------------
    // Rename, menus, toasts
    // ------------------------------------------------------------------

    fn renaming(&self) -> bool {
        self.inner.rows.borrow().values().any(|r| r.is_renaming())
    }

    fn begin_rename(&self, id: NodeId) {
        self.reveal(id);
        if let Some(p) = self.position_of(id) {
            self.inner
                .list
                .scroll_to(p, gtk::ListScrollFlags::NONE, None);
        }
        let row = self
            .inner
            .bound
            .borrow()
            .get(&id)
            .and_then(|v| v.first().cloned());
        let Some(row) = row else {
            return;
        };
        let (name, locked) = self.inner.ctx.engine().with(|e| {
            let n = e.scene().get(id);
            (
                n.and_then(|n| n.name.clone()).unwrap_or_default(),
                n.is_some_and(|n| n.locked),
            )
        });
        if locked {
            self.toast("Locked — unlock it to rename");
            return;
        }
        row.begin_rename(&name);
    }

    fn commit_rename(&self, row: &Row) {
        let Some(id) = row.id.get() else {
            row.end_rename();
            return;
        };
        let text = row.entry.text().to_string();
        match ops::rename(&self.inner.ctx, id, &text) {
            Ok(msg) => {
                row.end_rename();
                self.inner.list.grab_focus();
                if let Some(m) = msg {
                    self.toast(&m);
                }
            }
            Err(e) => row.set_error(Some(&e)),
        }
    }

    fn add_and_rename(&self, kind: KindTag, parent: Option<Option<NodeId>>) {
        if let Some(Some(p)) = parent {
            self.expand_node(p);
        }
        if let Some(id) = ops::add_node(&self.inner.ctx, kind, parent) {
            // The row appears on the next pump tick.
            let weak = self.downgrade();
            glib::idle_add_local_once(move || {
                Self::with_weak(&weak, |t| {
                    if let Some(Some(p)) = parent {
                        t.expand_node(p);
                    }
                    t.apply_selection_from_ctx();
                    if t.inner
                        .ctx
                        .engine()
                        .with(|e| e.scene().get(id).is_some_and(|n| n.name.is_some()))
                    {
                        t.begin_rename(id);
                    }
                });
            });
        }
    }

    fn popup_menu(&self, row: &Row, x: f64, y: f64) {
        let Some(id) = row.id.get() else {
            return;
        };
        let sel = self.inner.ctx.selection();
        if !sel.selected().contains(&id) {
            sel.set_selected([id]);
        }
        let menu = gio::Menu::new();
        let add = gio::Menu::new();
        for kind in model::ADDABLE {
            add.append(
                Some(model::kind_title(kind)),
                Some(&format!("tree.add-child::{}", kind.as_str())),
            );
        }
        menu.append_submenu(Some("Add Child"), &add);
        let edit = gio::Menu::new();
        edit.append(Some("Rename\tF2"), Some("tree.rename"));
        edit.append(Some("Duplicate\tCtrl+D"), Some("tree.duplicate"));
        edit.append(Some("Group into Folder\tCtrl+G"), Some("tree.group"));
        edit.append(Some("Delete\tDel"), Some("tree.delete"));
        menu.append_section(None, &edit);
        let more = gio::Menu::new();
        more.append(Some("Reveal in Dependency Graph"), Some("tree.reveal"));
        menu.append_section(None, &more);
        self.inner.menu.set_menu_model(Some(&menu));
        let point = row
            .expander
            .compute_point(
                &self.inner.root,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
        self.inner.menu.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        self.inner.menu.popup();
    }

    fn report(&self, r: Result<(), viscas_scene::SceneError>) {
        if let Err(e) = r {
            self.toast(&e.to_string());
        }
    }

    /// Shows a short message at the bottom of the tree.
    fn toast(&self, text: &str) {
        self.inner.toast_label.set_label(text);
        self.inner.toast.set_reveal_child(true);
        if let Some(old) = self.inner.toast_timer.borrow_mut().take() {
            old.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(TOAST_TIME, move || {
            Self::with_weak(&weak, |t| {
                t.inner.toast_timer.borrow_mut().take();
                t.inner.toast.set_reveal_child(false);
            });
        });
        *self.inner.toast_timer.borrow_mut() = Some(id);
    }
}

fn drop_class(pos: DropPos) -> &'static str {
    match pos {
        DropPos::Before => "drop-before",
        DropPos::After => "drop-after",
        DropPos::Into => "drop-into",
    }
}
