//! Expressions editor: a flat, ordered list of the scene's statement
//! nodes (REQUIREMENTS §2) — a view of the scene, not a separate model.
//!
//! One row per listed node in scene order: color swatch, visibility
//! toggle, editable line (committed on Enter / focus-out as one undo
//! step), inline error with the span underlined, inline value preview,
//! slider controls for Slider nodes, drag handle to reorder and a delete
//! button. A "+" row at the bottom adds lines.
//!
//! Keyboard: Enter commits and moves on (from the last row to the "+"
//! row), Up/Down move between rows, Ctrl+Backspace on an empty row
//! deletes it, Escape reverts. Rows follow the shared selection and
//! hover. Scene events update only the rows they touch, so typing,
//! focus and the cursor survive re-evaluation.

pub mod logic;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use viscas_core::syntax::Span;
use viscas_scene::{Color as SceneColor, NodeId, NodeKind, SceneEvent, SourceField, Status};

use crate::ctx::AppCtx;
use crate::doc::{DocEvent, Subscription};
use crate::editors::console::{header_menu_button, render, style};
use crate::editors::pick_icon;
use crate::theme::model::{Color as UiColor, trim_float};
use crate::widgets::scrub::{ScrubField, ScrubSpec};
use crate::widgets::swatch::ColorSwatch;
use logic::{Place, underline_range};

/// A header-menu action: name and what it does.
type MenuAction = (&'static str, fn(&Expressions));

/// Prefix of the drag payload (a node id).
const DRAG_PREFIX: &str = "viscas-node:";

thread_local! {
    /// Every Expressions editor, most recently used last.
    static INSTANCES: RefCell<Vec<Weak<Inner>>> = const { RefCell::new(Vec::new()) };
}

/// Palette command "New Expression" for the active editor.
struct ExpressionCommands;

impl crate::commands::CommandProvider for ExpressionCommands {
    fn commands(&self) -> Vec<crate::commands::Command> {
        if Expressions::active().is_none() {
            return Vec::new();
        }
        vec![crate::commands::Command {
            id: "expressions.new".into(),
            title: "New Expression".into(),
            category: "Expressions",
            shortcut: None,
            run: Rc::new(|| {
                // After the palette closed and restored focus.
                glib::idle_add_local_once(|| {
                    if let Some(ed) = Expressions::active() {
                        ed.new_expression();
                    }
                });
            }),
        }]
    }
}

/// What a row shows, read from the engine in one borrow.
struct RowData {
    kind: NodeKind,
    line: String,
    visible: bool,
    color: SceneColor,
    /// Message and span within `line`.
    error: Option<(String, Option<Span>)>,
    /// Slider bound/step errors (not part of the line).
    side_error: Option<String>,
    value: Option<(String, String)>,
}

struct SliderUi {
    root: gtk::Box,
    scale: gtk::Scale,
    min: ScrubField,
    max: ScrubField,
    step: ScrubField,
    syncing: Cell<bool>,
}

struct Row {
    id: NodeId,
    root: gtk::Box,
    index: gtk::Label,
    swatch: ColorSwatch,
    eye: gtk::Button,
    entry: gtk::Entry,
    value: gtk::Label,
    error: gtk::Label,
    slider: RefCell<Option<SliderUi>>,
    /// The line last shown from the model.
    line: RefCell<String>,
    /// The user typed since the last sync.
    dirty: Cell<bool>,
    focused: Cell<bool>,
    /// Programmatic text change in progress.
    setting: Cell<bool>,
    /// A rejected commit (parse or scene error) shown until the next edit.
    local_error: RefCell<Option<(String, Option<Span>)>>,
    /// The text an error span refers to and the span: underlined only
    /// while the entry shows exactly that text.
    underline: RefCell<Option<(String, Span)>>,
    /// The node was removed while the user had unsaved typing: the row
    /// stays so the text is not lost (Enter adds it as a new node).
    orphan: Cell<bool>,
}

impl Row {
    fn set_dirty(&self, dirty: bool) {
        self.dirty.set(dirty);
        // Lets Ctrl+Z reach the entry's own text undo (see `window`).
        if dirty {
            self.entry.add_css_class("text-dirty");
        } else {
            self.entry.remove_css_class("text-dirty");
        }
    }
}

struct Inner {
    ctx: AppCtx,
    root: gtk::Box,
    list: gtk::Box,
    empty: gtk::Box,
    add_entry: gtk::Entry,
    add_error: gtk::Label,
    rows: RefCell<HashMap<NodeId, Rc<Row>>>,
    order: RefCell<Vec<NodeId>>,
    /// Slider controls hidden (header menu).
    sliders_collapsed: Cell<bool>,
    subs: RefCell<Vec<Subscription>>,
    /// Selection and hover the row styles currently show.
    styled: RefCell<(Vec<NodeId>, Option<NodeId>)>,
}

/// The Expressions editor. Cloning yields a handle to the same widget.
#[derive(Clone)]
pub struct Expressions {
    inner: Rc<Inner>,
}

impl Expressions {
    pub fn new(ctx: &AppCtx) -> Self {
        style::ensure(ctx);
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let empty = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .css_classes(["expr-empty"])
            .build();
        let add_entry = gtk::Entry::builder()
            .hexpand(true)
            .css_classes(["expr-line"])
            .build();
        add_entry.update_property(&[gtk::accessible::Property::Label("New expression")]);
        let add_error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["expr-error"])
            .build();
        let add_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["expr-row", "expr-add"])
            .build();
        let add_line = gtk::Box::builder().spacing(4).build();
        add_line.append(
            &gtk::Image::builder()
                .icon_name(pick_icon(&["list-add-symbolic"]))
                .build(),
        );
        add_line.append(&add_entry);
        add_row.append(&add_line);
        add_row.append(&add_error);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        column.append(&empty);
        column.append(&list);
        column.append(&add_row);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&column)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["expr-list"])
            .build();
        root.append(&scroller);

        let ed = Self {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                root,
                list,
                empty,
                add_entry,
                add_error,
                rows: RefCell::default(),
                order: RefCell::default(),
                sliders_collapsed: Cell::new(false),
                subs: RefCell::default(),
                styled: RefCell::default(),
            }),
        };
        ed.build_empty_state();
        ed.connect_add_row();
        ed.connect_updates();
        ed.reconcile();
        ed.register(ctx);
        ed
    }

    /// Tracks this editor for the palette's "New Expression"; installs the
    /// command provider once per process.
    fn register(&self, ctx: &AppCtx) {
        let first = INSTANCES.with_borrow_mut(|v| {
            let first = v.is_empty();
            v.retain(|w| w.strong_count() > 0);
            v.push(self.downgrade());
            first
        });
        if first {
            ctx.add_command_provider(ExpressionCommands);
        }
        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_enter(move |_| {
            INSTANCES.with_borrow_mut(|v| {
                v.retain(|w| w.strong_count() > 0 && !w.ptr_eq(&weak));
                v.push(weak.clone());
            });
        });
        self.inner.root.add_controller(focus);
    }

    /// The most recently used Expressions editor that is on screen.
    fn active() -> Option<Expressions> {
        INSTANCES.with_borrow(|v| {
            v.iter()
                .rev()
                .filter_map(Expressions::from_weak)
                .find(|e| e.inner.root.is_mapped())
        })
    }

    /// Focuses the "+" row.
    fn new_expression(&self) {
        self.inner.add_entry.grab_focus();
    }

    fn toggle_sliders(&self) {
        let collapsed = !self.inner.sliders_collapsed.get();
        self.inner.sliders_collapsed.set(collapsed);
        for row in self.inner.rows.borrow().values() {
            if let Some(ui) = row.slider.borrow().as_ref() {
                ui.root.set_visible(!collapsed);
            }
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.inner.root.clone().upcast()
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.inner)
    }

    fn from_weak(weak: &Weak<Inner>) -> Option<Self> {
        weak.upgrade().map(|inner| Self { inner })
    }

    fn row(&self, id: NodeId) -> Option<Rc<Row>> {
        self.inner.rows.borrow().get(&id).cloned()
    }

    /// Header widgets: a "new expression" button.
    pub fn header_content(&self) -> Vec<gtk::Widget> {
        let add = gtk::Button::builder()
            .icon_name(pick_icon(&["list-add-symbolic"]))
            .tooltip_text("New expression")
            .css_classes(["area-tool"])
            .build();
        let weak = self.downgrade();
        add.connect_clicked(move |_| {
            if let Some(ed) = Expressions::from_weak(&weak) {
                ed.new_expression();
            }
        });
        let group = gio::SimpleActionGroup::new();
        let actions: [MenuAction; 2] = [
            ("new", |e| e.new_expression()),
            ("toggle-sliders", |e| e.toggle_sliders()),
        ];
        for (name, f) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = self.downgrade();
            action.connect_activate(move |_, _| {
                if let Some(ed) = Expressions::from_weak(&weak) {
                    f(&ed);
                }
            });
            group.add_action(&action);
        }
        let menu = header_menu_button(
            "Edit",
            "expressions",
            &group,
            &[
                ("New Expression", "new", None),
                ("Collapse / Expand Sliders", "toggle-sliders", None),
            ],
        );
        vec![menu.upcast(), add.upcast()]
    }

    fn build_empty_state(&self) {
        let empty = &self.inner.empty;
        let op = match self.inner.ctx.engine().with(|e| e.settings().definition_op) {
            viscas_core::syntax::DefinitionOp::Eq => "=",
            viscas_core::syntax::DefinitionOp::ColonEq => ":=",
        };
        while let Some(c) = empty.first_child() {
            empty.remove(&c);
        }
        let hint = gtk::Label::builder()
            .use_markup(true)
            .label(render::info_markup(&format!(
                "No expressions yet. Type a line below: `a {op} 3`, `f(x) {op} x^2`, `y = sin x`."
            )))
            .wrap(true)
            .xalign(0.0)
            .css_classes(["console-info"])
            .build();
        let quip = gtk::Label::builder()
            .label("One line at a time.")
            .xalign(0.0)
            .css_classes(["console-hint"])
            .visible(self.inner.ctx.preferences().with(|p| p.interface.playful))
            .build();
        self.inner
            .ctx
            .preferences()
            .connect_changed_for(quip.downgrade(), |q, p| q.set_visible(p.interface.playful));
        empty.append(&hint);
        empty.append(&quip);
        self.inner
            .add_entry
            .set_placeholder_text(Some(&format!("a {op} 3  ·  y = sin x")));
    }

    // ------------------------------------------------------------ rows

    /// Matches the rows to the scene: drops rows of removed or no longer
    /// listed nodes, creates missing ones and puts them in scene order.
    fn reconcile(&self) {
        let order = self
            .inner
            .ctx
            .engine()
            .with(|e| logic::rows_in_order(e.scene()));
        let keep: HashSet<NodeId> = order.iter().copied().collect();
        let gone: Vec<Rc<Row>> = {
            let mut rows = self.inner.rows.borrow_mut();
            let ids: Vec<NodeId> = rows
                .iter()
                .filter(|(id, row)| !keep.contains(id) && !row.dirty.get())
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter().filter_map(|id| rows.remove(&id)).collect()
        };
        for row in gone {
            self.inner.list.remove(&row.root);
        }
        // Rows with unsaved typing whose node went away stay as orphans;
        // a node coming back (undo) adopts its row again.
        let rows: Vec<Rc<Row>> = self.inner.rows.borrow().values().cloned().collect();
        for row in rows {
            let orphan = !keep.contains(&row.id);
            if row.orphan.replace(orphan) != orphan {
                if orphan {
                    row.index.set_label("·");
                    row.error.set_label(
                        "This node was removed. Enter adds the line as a new node, Esc discards it.",
                    );
                    row.error.set_visible(true);
                } else {
                    self.sync_row(&row);
                }
            }
        }
        let mut prev: Option<gtk::Widget> = None;
        for (i, id) in order.iter().enumerate() {
            let row = match self.row(*id) {
                Some(r) => r,
                None => {
                    let r = self.build_row(*id);
                    self.inner.list.append(&r.root);
                    self.inner.rows.borrow_mut().insert(*id, r.clone());
                    self.sync_row(&r);
                    r
                }
            };
            self.inner
                .list
                .reorder_child_after(&row.root, prev.as_ref());
            row.index.set_label(&(i + 1).to_string());
            prev = Some(row.root.clone().upcast());
        }
        self.inner.empty.set_visible(order.is_empty());
        *self.inner.order.borrow_mut() = order;
        self.apply_selection();
    }

    fn build_row(&self, id: NodeId) -> Rc<Row> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["expr-row"])
            .build();
        let line = gtk::Box::builder().spacing(2).build();
        let handle = gtk::Image::builder()
            .icon_name(pick_icon(&[
                "list-drag-handle-symbolic",
                "drag-handle-symbolic",
                "open-menu-symbolic",
            ]))
            .tooltip_text("Drag to reorder")
            .css_classes(["expr-handle"])
            .build();
        handle.set_cursor_from_name(Some("grab"));
        let index = gtk::Label::builder()
            .xalign(1.0)
            .css_classes(["expr-index"])
            .build();
        let swatch = ColorSwatch::new(UiColor::hex(0x808080), "Node Color");
        let sw = swatch.widget();
        sw.set_hexpand(false);
        sw.set_valign(gtk::Align::Center);
        sw.set_size_request(14, 14);
        sw.set_overflow(gtk::Overflow::Hidden);
        sw.add_css_class("expr-swatch");
        let eye = gtk::Button::builder()
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .focusable(false)
            .build();
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .css_classes(["expr-line"])
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Expression")]);
        let delete = gtk::Button::builder()
            .icon_name(pick_icon(&["edit-delete-symbolic", "user-trash-symbolic"]))
            .tooltip_text("Delete node")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .focusable(false)
            .build();
        line.append(&handle);
        line.append(&index);
        line.append(sw);
        line.append(&eye);
        line.append(&entry);
        line.append(&delete);
        let value = gtk::Label::builder()
            .use_markup(true)
            .xalign(0.0)
            .margin_start(62)
            .ellipsize(pango::EllipsizeMode::End)
            .visible(false)
            .css_classes(["expr-value"])
            .build();
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .margin_start(62)
            .visible(false)
            .css_classes(["expr-error"])
            .build();
        root.append(&line);
        root.append(&value);
        root.append(&error);

        let row = Rc::new(Row {
            id,
            root,
            index,
            swatch,
            eye,
            entry,
            value,
            error,
            slider: RefCell::new(None),
            line: RefCell::default(),
            dirty: Cell::new(false),
            focused: Cell::new(false),
            setting: Cell::new(false),
            local_error: RefCell::new(None),
            underline: RefCell::new(None),
            orphan: Cell::new(false),
        });
        self.connect_row(&row, &handle, &delete);
        row
    }

    fn connect_row(&self, row: &Rc<Row>, handle: &gtk::Image, delete: &gtk::Button) {
        let id = row.id;
        let weak = self.downgrade();
        let w = weak.clone();
        row.swatch.connect_changed(move |c| {
            if let Some(ed) = Expressions::from_weak(&w) {
                ed.inner.ctx.engine().edit(|e| {
                    if let Some(mut style) = e.scene().get(id).map(|n| n.style.clone()) {
                        style.color = Some(SceneColor::rgba(c.r, c.g, c.b, c.a));
                        let _ = e.scene_mut().set_style(id, style);
                    }
                });
            }
        });
        let w = weak.clone();
        row.eye.connect_clicked(move |_| {
            if let Some(ed) = Expressions::from_weak(&w) {
                ed.inner.ctx.engine().edit(|e| {
                    if let Some(v) = e.scene().get(id).map(|n| n.visible) {
                        let _ = e.scene_mut().set_visible(id, !v);
                    }
                });
            }
        });
        let w = weak.clone();
        delete.connect_clicked(move |_| {
            if let Some(ed) = Expressions::from_weak(&w) {
                ed.delete(id);
            }
        });

        // Typing marks the row dirty (kept through re-evaluation).
        let w = weak.clone();
        row.entry.connect_changed(move |_| {
            if let Some(ed) = Expressions::from_weak(&w)
                && let Some(row) = ed.row(id)
            {
                if !row.setting.get() {
                    row.set_dirty(true);
                }
                // The span belongs to the text it was reported for.
                ed.apply_underline(&row);
            }
        });
        let w = weak.clone();
        row.entry.connect_activate(move |_| {
            if let Some(ed) = Expressions::from_weak(&w)
                && ed.commit(id)
            {
                ed.focus_after(id, 1);
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let w = weak.clone();
        focus.connect_enter(move |_| {
            if let Some(ed) = Expressions::from_weak(&w)
                && let Some(row) = ed.row(id)
            {
                row.focused.set(true);
                let sel = ed.inner.ctx.selection();
                if !sel.selected().contains(&id) {
                    sel.set_selected([id]);
                }
            }
        });
        let w = weak.clone();
        focus.connect_leave(move |_| {
            if let Some(ed) = Expressions::from_weak(&w)
                && let Some(row) = ed.row(id)
            {
                row.focused.set(false);
                if row.dirty.get() {
                    ed.commit(id);
                }
            }
        });
        row.entry.add_controller(focus);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = weak.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(ed) = Expressions::from_weak(&w) else {
                return glib::Propagation::Proceed;
            };
            ed.on_row_key(id, key, mods)
        });
        row.entry.add_controller(keys);

        // Hover and click selection.
        let motion = gtk::EventControllerMotion::new();
        let w = weak.clone();
        motion.connect_enter(move |_, _, _| {
            if let Some(ed) = Expressions::from_weak(&w) {
                ed.inner.ctx.selection().set_hovered(Some(id));
            }
        });
        let w = weak.clone();
        motion.connect_leave(move |_| {
            if let Some(ed) = Expressions::from_weak(&w) {
                let sel = ed.inner.ctx.selection();
                if sel.hovered() == Some(id) {
                    sel.set_hovered(None);
                }
            }
        });
        row.root.add_controller(motion);
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = weak.clone();
        click.connect_pressed(move |g, _, _, _| {
            let Some(ed) = Expressions::from_weak(&w) else {
                return;
            };
            let sel = ed.inner.ctx.selection();
            if g.current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                let mut ids = sel.selected();
                if let Some(i) = ids.iter().position(|x| *x == id) {
                    ids.remove(i);
                } else {
                    ids.push(id);
                }
                sel.set_selected(ids);
            } else if sel.selected() != [id] {
                sel.set_selected([id]);
            }
        });
        row.root.add_controller(click);

        // Drag to reorder.
        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        let w = weak.clone();
        drag.connect_prepare(move |src, x, y| {
            let ed = Expressions::from_weak(&w)?;
            if let Some(row) = ed.row(id) {
                let icon = gtk::WidgetPaintable::new(Some(&row.root));
                src.set_icon(Some(&icon), x as i32, y as i32);
            }
            Some(gdk::ContentProvider::for_value(
                &format!("{DRAG_PREFIX}{}", id.0).to_value(),
            ))
        });
        handle.add_controller(drag);

        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let place_at = |root: &gtk::Box, y: f64| {
            if y < f64::from(root.height()) / 2.0 {
                Place::Before
            } else {
                Place::After
            }
        };
        let w = weak.clone();
        drop.connect_motion(move |_, _, y| {
            if let Some(ed) = Expressions::from_weak(&w)
                && let Some(row) = ed.row(id)
            {
                let before = place_at(&row.root, y) == Place::Before;
                row.root
                    .remove_css_class(if before { "drop-after" } else { "drop-before" });
                row.root
                    .add_css_class(if before { "drop-before" } else { "drop-after" });
            }
            gdk::DragAction::MOVE
        });
        let w = weak.clone();
        drop.connect_leave(move |_| {
            if let Some(ed) = Expressions::from_weak(&w)
                && let Some(row) = ed.row(id)
            {
                row.root.remove_css_class("drop-before");
                row.root.remove_css_class("drop-after");
            }
        });
        drop.connect_drop(move |_, value, _, y| {
            let Some(ed) = Expressions::from_weak(&weak) else {
                return false;
            };
            let Some(row) = ed.row(id) else {
                return false;
            };
            row.root.remove_css_class("drop-before");
            row.root.remove_css_class("drop-after");
            let Some(dragged) = value
                .get::<String>()
                .ok()
                .and_then(|s| s.strip_prefix(DRAG_PREFIX)?.parse::<u64>().ok())
                .map(NodeId)
            else {
                return false;
            };
            let place = place_at(&row.root, y);
            ed.inner.ctx.engine().edit(|e| {
                match logic::drop_target(e.scene(), dragged, id, place) {
                    Some((parent, index)) => {
                        e.scene_mut().move_node(dragged, parent, index).is_ok()
                    }
                    None => false,
                }
            })
        });
        row.root.add_controller(drop);
    }

    fn on_row_key(&self, id: NodeId, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let Some(row) = self.row(id) else {
            return glib::Propagation::Proceed;
        };
        match key {
            Key::Up | Key::Down if !ctrl => {
                if row.dirty.get() && !self.commit(id) {
                    return glib::Propagation::Stop;
                }
                self.focus_after(id, if key == Key::Up { -1 } else { 1 });
                glib::Propagation::Stop
            }
            Key::BackSpace if ctrl && row.entry.text().is_empty() => {
                // The emptied text is not "unsaved typing": a dirty row
                // would survive its node's removal as a ghost orphan (the
                // first row keeps focus, so no focus-out clears it).
                row.set_dirty(false);
                self.focus_after(id, -1);
                if row.orphan.get() {
                    self.drop_row(id);
                } else {
                    self.delete(id);
                    let exists = self.inner.ctx.engine().with(|e| e.scene().contains(id));
                    if exists {
                        // Refused (locked): show the node's line again.
                        self.sync_row(&row);
                    }
                }
                glib::Propagation::Stop
            }
            Key::Escape if row.orphan.get() => {
                self.drop_row(id);
                glib::Propagation::Stop
            }
            Key::Escape if row.dirty.get() || row.local_error.borrow().is_some() => {
                row.set_dirty(false);
                *row.local_error.borrow_mut() = None;
                self.sync_row(&row);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// Focuses the row `delta` rows away from `id` (past the end: the
    /// "+" row; before the start: stays).
    fn focus_after(&self, id: NodeId, delta: isize) {
        let order = self.inner.order.borrow().clone();
        let Some(pos) = order.iter().position(|x| *x == id) else {
            return;
        };
        let target = pos as isize + delta;
        if target >= order.len() as isize {
            self.inner.add_entry.grab_focus();
        } else if target >= 0
            && let Some(row) = self.row(order[target as usize])
        {
            row.entry.grab_focus();
            row.entry.set_position(-1);
        }
    }

    /// Commits every row with unsaved typing into its node (save, close,
    /// the editor being hidden). Orphan rows stay as they are: adding
    /// them back is the user's call. Returns the 1-based positions of
    /// rows whose text could not be committed (shown with their error).
    fn commit_dirty(&self) -> Vec<usize> {
        let dirty: Vec<NodeId> = self
            .inner
            .rows
            .borrow()
            .values()
            .filter(|r| r.dirty.get() && !r.orphan.get())
            .map(|r| r.id)
            .collect();
        let failed: Vec<NodeId> = dirty.into_iter().filter(|id| !self.commit(*id)).collect();
        let order = self.inner.order.borrow();
        let mut positions: Vec<usize> = failed
            .iter()
            .filter_map(|id| order.iter().position(|x| x == id).map(|p| p + 1))
            .collect();
        positions.sort_unstable();
        positions
    }

    /// Removes a row that has no node any more.
    fn drop_row(&self, id: NodeId) {
        if let Some(row) = self.inner.rows.borrow_mut().remove(&id) {
            self.inner.list.remove(&row.root);
        }
    }

    fn delete(&self, id: NodeId) {
        self.inner.ctx.engine().edit(|e| {
            let _ = e.scene_mut().remove(id);
        });
    }

    /// Commits the row's text into its node; `false` (and an inline
    /// error) when the line does not parse or the scene refuses it.
    fn commit(&self, id: NodeId) -> bool {
        let Some(row) = self.row(id) else {
            return false;
        };
        let text = row.entry.text().to_string();
        if row.orphan.get() {
            if text.trim().is_empty() {
                self.drop_row(id);
                return true;
            }
            let added = self.inner.ctx.engine().edit(|e| {
                e.scene_mut().seal_history();
                e.add_line(None, &text)
            });
            return match added {
                Ok(_) => {
                    self.drop_row(id);
                    true
                }
                Err(err) => {
                    *row.local_error.borrow_mut() = Some((err.to_string(), None));
                    self.show_error(&row, &text);
                    false
                }
            };
        }
        if !row.dirty.get() || text == *row.line.borrow() {
            row.set_dirty(false);
            return true;
        }
        if text.trim().is_empty() {
            // Emptied: Ctrl+Backspace deletes; otherwise keep the node.
            row.set_dirty(false);
            self.sync_row(&row);
            return true;
        }
        let result = self.inner.ctx.engine().edit(|e| {
            let parsed = e.line_content(&text).map_err(|errs| {
                let err = &errs[0];
                (err.message.clone(), Some(err.span))
            })?;
            let old = e
                .scene()
                .get(id)
                .map(|n| n.content())
                .ok_or_else(|| ("node is gone".to_owned(), None))?;
            let content = logic::merged_content(&old, parsed);
            let scene = e.scene_mut();
            scene.seal_history();
            scene
                .set_content(id, content, None, "Edit expression")
                .map_err(|err| (err.to_string(), None))
        });
        match result {
            Ok(()) => {
                row.set_dirty(false);
                *row.local_error.borrow_mut() = None;
                true
            }
            Err(err) => {
                *row.local_error.borrow_mut() = Some(err);
                self.show_error(&row, &text);
                false
            }
        }
    }

    fn read_row(&self, id: NodeId) -> Option<RowData> {
        self.inner.ctx.engine().with(|e| {
            let node = e.scene().get(id)?;
            let content = node.content();
            let line = viscas_scene::definition_line_with(&content, e.settings().parse_options())
                .unwrap_or_default();
            let mut error = None;
            let mut side_error = None;
            if let Status::Error {
                message,
                field,
                span,
            } = e.scene().status(id)
            {
                let field = field.unwrap_or(SourceField::Main);
                if field == SourceField::Main {
                    let span = span.and_then(|s| logic::line_span(&line, &node.kind, field, s));
                    error = Some((message.clone(), span));
                } else {
                    side_error = Some(format!("{field}: {message}"));
                }
            }
            let value = logic::shows_value(&node.kind)
                .then(|| e.value(id))
                .flatten()
                .map(|v| (v.exact.clone(), v.approx.clone()));
            Some(RowData {
                kind: node.kind.clone(),
                line,
                visible: node.visible,
                color: e.node_color(id).unwrap_or(SceneColor::rgb(128, 128, 128)),
                error,
                side_error,
                value,
            })
        })
    }

    /// Refreshes one row from the engine, keeping the user's typing.
    fn sync_row(&self, row: &Rc<Row>) {
        if row.orphan.get() {
            return;
        }
        let Some(data) = self.read_row(row.id) else {
            return;
        };
        let editing = row.dirty.get();
        *row.line.borrow_mut() = data.line.clone();
        if !editing && row.entry.text() != data.line {
            let pos = row.entry.position();
            row.setting.set(true);
            row.entry.set_text(&data.line);
            row.setting.set(false);
            if row.focused.get() {
                row.entry
                    .set_position(pos.min(data.line.chars().count() as i32));
            }
        }
        let c = data.color;
        row.swatch.set_color(UiColor {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        });
        row.eye.set_icon_name(pick_icon(if data.visible {
            &["view-reveal-symbolic", "eye-open-negative-filled-symbolic"]
        } else {
            &["view-conceal-symbolic", "eye-not-looking-symbolic"]
        }));
        row.eye
            .set_tooltip_text(Some(if data.visible { "Hide" } else { "Show" }));
        if data.visible {
            row.root.remove_css_class("hidden-node");
        } else {
            row.root.add_css_class("hidden-node");
        }
        match &data.value {
            Some((exact, approx)) if data.error.is_none() && !exact.is_empty() => {
                let mut markup = format!("= {}", render::math_markup(exact));
                if render::shows_approx(exact, approx) {
                    markup.push_str(&format!(
                        "   <span alpha=\"60%\">≈ {}</span>",
                        render::escape(approx.trim_start_matches('≈'))
                    ));
                }
                row.value.set_markup(&markup);
                row.value.set_visible(true);
            }
            _ => row.value.set_visible(false),
        }
        self.sync_slider(row, &data);
        if !editing {
            *row.local_error.borrow_mut() = None;
        }
        // A commit error refers to the typed text, a node error to the
        // model's line.
        let local = row.local_error.borrow().clone();
        match local {
            Some(err) => {
                let text = row.entry.text().to_string();
                self.render_error(row, &text, Some(err), data.side_error);
            }
            None => self.render_error(row, &data.line, data.error.clone(), data.side_error),
        }
    }

    fn show_error(&self, row: &Rc<Row>, text: &str) {
        let err = row.local_error.borrow().clone();
        self.render_error(row, text, err, None);
    }

    /// Shows `err` (whose span refers to `text`) and `side` under the row.
    fn render_error(
        &self,
        row: &Row,
        text: &str,
        err: Option<(String, Option<Span>)>,
        side: Option<String>,
    ) {
        *row.underline.borrow_mut() = match &err {
            Some((_, Some(span))) => Some((text.to_owned(), *span)),
            _ => None,
        };
        let message = match (&err, side) {
            (Some((m, _)), _) => Some(m.clone()),
            (None, s) => s,
        };
        match message {
            Some(m) => {
                row.error.set_label(&m);
                row.error.set_visible(true);
            }
            None => row.error.set_visible(false),
        }
        self.apply_underline(row);
    }

    /// Wavy underline of the error span inside the entry, only while it
    /// shows the text the span was reported for.
    fn apply_underline(&self, row: &Row) {
        let attrs = pango::AttrList::new();
        let current = row.entry.text();
        let range = row
            .underline
            .borrow()
            .as_ref()
            .and_then(|(text, span)| underline_range(text, current.as_str(), *span));
        if let Some((start, end)) = range {
            let color = self
                .inner
                .ctx
                .preferences()
                .with(|p| p.effective_theme().console.error);
            let mut u = pango::AttrInt::new_underline(pango::Underline::Error);
            u.set_start_index(start as u32);
            u.set_end_index(end as u32);
            attrs.insert(u);
            let scale = |v: u8| u16::from(v) * 257;
            let mut uc = pango::AttrColor::new_underline_color(
                scale(color.r),
                scale(color.g),
                scale(color.b),
            );
            uc.set_start_index(start as u32);
            uc.set_end_index(end as u32);
            attrs.insert(uc);
        }
        row.entry.set_attributes(&attrs);
    }

    fn sync_slider(&self, row: &Rc<Row>, data: &RowData) {
        let NodeKind::Slider {
            value,
            min,
            max,
            step,
            ..
        } = &data.kind
        else {
            if let Some(s) = row.slider.borrow_mut().take() {
                row.root.remove(&s.root);
            }
            return;
        };
        let (lo, hi, st) = logic::slider_range(min, max, step);
        let current = data
            .value
            .as_ref()
            .and_then(|(_, a)| a.trim_start_matches('≈').parse::<f64>().ok())
            .or_else(|| value.trim().parse::<f64>().ok())
            .unwrap_or(lo);
        if row.slider.borrow().is_none() {
            let ui = self.build_slider(row.id);
            row.root.insert_child_after(&ui.root, Some(&row.value));
            ui.root.set_visible(!self.inner.sliders_collapsed.get());
            *row.slider.borrow_mut() = Some(ui);
        }
        let slider = row.slider.borrow();
        let Some(ui) = slider.as_ref() else {
            return;
        };
        ui.syncing.set(true);
        ui.scale.set_range(lo, hi);
        ui.scale.set_increments(st, st * 10.0);
        ui.scale.set_digits(logic::step_digits(st) as i32);
        ui.scale.set_value(current.clamp(lo, hi));
        let num = |s: &str, fallback: f64| s.trim().parse::<f64>().unwrap_or(fallback);
        ui.min.set_value(num(min, lo));
        ui.max.set_value(num(max, hi));
        ui.step.set_value(num(step, 0.0));
        ui.syncing.set(false);
    }

    fn build_slider(&self, id: NodeId) -> SliderUi {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_start(62)
            .margin_end(22)
            .css_classes(["expr-slider"])
            .build();
        let scale = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .draw_value(false)
            .focusable(false)
            .build();
        let spec = ScrubSpec::new(-1e9, 1e9, 1.0, 3);
        let bounds = gtk::Box::builder().spacing(4).homogeneous(true).build();
        let field = |label: &str, spec: ScrubSpec| {
            let f = ScrubField::new(spec, 0.0);
            f.widget().add_css_class("expr-bound");
            f.set_tooltip(&format!("Slider {label}"));
            let cell = gtk::Box::builder().spacing(4).build();
            cell.append(
                &gtk::Label::builder()
                    .label(label.split(' ').next().unwrap_or(label))
                    .css_classes(["expr-slider-label"])
                    .build(),
            );
            cell.append(f.widget());
            bounds.append(&cell);
            f
        };
        let min = field("min", spec);
        let max = field("max", spec);
        let step = field("step (0 = continuous)", ScrubSpec::new(0.0, 1e9, 0.1, 3));
        root.append(&scale);
        root.append(&bounds);
        let ui = SliderUi {
            root,
            scale,
            min,
            max,
            step,
            syncing: Cell::new(false),
        };
        let weak = self.downgrade();
        let set_field = move |field: SourceField, text: String| {
            let Some(ed) = Expressions::from_weak(&weak) else {
                return;
            };
            // Model → widget updates must not write back.
            let syncing = ed.row(id).is_none_or(|row| {
                row.slider
                    .borrow()
                    .as_ref()
                    .is_none_or(|ui| ui.syncing.get())
            });
            if !syncing {
                ed.inner.ctx.engine().edit(|e| {
                    let _ = e.scene_mut().set_source(id, field, &text);
                });
            }
        };
        let set = Rc::new(set_field);
        let s = set.clone();
        let weak = self.downgrade();
        ui.scale.connect_value_changed(move |scale| {
            let Some(ed) = Expressions::from_weak(&weak) else {
                return;
            };
            let Some(row) = ed.row(id) else {
                return;
            };
            let slider = row.slider.borrow();
            let Some(ui) = slider.as_ref() else {
                return;
            };
            if ui.syncing.get() {
                return;
            }
            let adj = scale.adjustment();
            let text = logic::slider_text(scale.value(), adj.lower(), adj.step_increment());
            drop(slider);
            s(SourceField::Main, text);
        });
        for (f, field) in [(&ui.min, SourceField::Min), (&ui.max, SourceField::Max)] {
            let s = set.clone();
            f.connect_changed(move |v| s(field, trim_float(v, 6)));
        }
        let s = set;
        ui.step.connect_changed(move |v| {
            s(
                SourceField::Step,
                if v > 0.0 {
                    trim_float(v, 6)
                } else {
                    String::new()
                },
            )
        });
        ui
    }

    // ------------------------------------------------------- add row

    fn connect_add_row(&self) {
        let weak = self.downgrade();
        self.inner.add_entry.connect_activate(move |entry| {
            let Some(ed) = Expressions::from_weak(&weak) else {
                return;
            };
            let text = entry.text().to_string();
            if text.trim().is_empty() {
                return;
            }
            let result = ed.inner.ctx.engine().edit(|e| {
                e.scene_mut().seal_history();
                e.add_line(None, &text)
            });
            match result {
                Ok(id) => {
                    entry.set_text("");
                    ed.inner.add_error.set_visible(false);
                    ed.inner.ctx.selection().set_selected([id]);
                }
                Err(err) => {
                    ed.inner.add_error.set_label(&err.to_string());
                    ed.inner.add_error.set_visible(true);
                }
            }
        });
        let weak = self.downgrade();
        self.inner.add_entry.connect_changed(move |entry| {
            // A draft is uncommitted typing: Ctrl+Z undoes it first.
            if entry.text().is_empty() {
                entry.remove_css_class("text-dirty");
            } else {
                entry.add_css_class("text-dirty");
            }
            if let Some(ed) = Expressions::from_weak(&weak) {
                ed.inner.add_error.set_visible(false);
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Up
                && let Some(ed) = Expressions::from_weak(&weak)
                && let Some(last) = ed.inner.order.borrow().last().copied()
                && let Some(row) = ed.row(last)
            {
                row.entry.grab_focus();
                row.entry.set_position(-1);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.inner.add_entry.add_controller(keys);
    }

    // ------------------------------------------------------- updates

    /// Restyles every row from the shared selection (rows were added or
    /// rebuilt).
    fn apply_selection(&self) {
        let sel = self.inner.ctx.selection();
        let (selected, hovered) = (sel.selected(), sel.hovered());
        for (id, row) in self.inner.rows.borrow().iter() {
            Self::style_row(row, selected.contains(id), hovered == Some(*id));
        }
        *self.inner.styled.borrow_mut() = (selected, hovered);
    }

    /// Restyles only the rows whose selected/hovered state changed since
    /// the last styling (hover moves touch two rows, not every row).
    fn update_selection_styles(&self) {
        let sel = self.inner.ctx.selection();
        let (selected, hovered) = (sel.selected(), sel.hovered());
        let changed = {
            let (old_sel, old_hover) = &*self.inner.styled.borrow();
            logic::restyle_ids(old_sel, *old_hover, &selected, hovered)
        };
        for id in changed {
            if let Some(row) = self.row(id) {
                Self::style_row(&row, selected.contains(&id), hovered == Some(id));
            }
        }
        *self.inner.styled.borrow_mut() = (selected, hovered);
    }

    fn style_row(row: &Row, selected: bool, hovered: bool) {
        if selected {
            row.root.add_css_class("selected");
        } else {
            row.root.remove_css_class("selected");
        }
        if hovered {
            row.root.add_css_class("hovered");
        } else {
            row.root.remove_css_class("hovered");
        }
    }

    fn sync_all(&self) {
        let rows: Vec<Rc<Row>> = self.inner.rows.borrow().values().cloned().collect();
        for row in rows {
            self.sync_row(&row);
        }
    }

    fn on_scene(&self, events: &[SceneEvent]) {
        let mut structural = false;
        let mut touched: HashSet<NodeId> = HashSet::new();
        for ev in events {
            match ev {
                SceneEvent::NodeAdded(_)
                | SceneEvent::NodeRemoved(_)
                | SceneEvent::Moved { .. } => {
                    structural = true;
                }
                SceneEvent::Changed { id, what } => {
                    if *what == viscas_scene::Change::Kind || *what == viscas_scene::Change::Name {
                        // The kind may enter or leave the list.
                        let listed = self.inner.rows.borrow().contains_key(id);
                        let now =
                            self.inner.ctx.engine().with(|e| {
                                e.scene().get(*id).is_some_and(|n| logic::listed(&n.kind))
                            });
                        structural |= listed != now;
                    }
                    touched.insert(*id);
                }
                _ => {}
            }
        }
        if structural {
            self.reconcile();
        }
        for id in touched {
            if let Some(row) = self.row(id) {
                self.sync_row(&row);
            }
        }
    }

    fn connect_updates(&self) {
        let weak = self.downgrade();
        let sub = self.inner.ctx.engine().subscribe(move |ev| {
            let Some(ed) = Expressions::from_weak(&weak) else {
                return;
            };
            match ev {
                DocEvent::Scene(events) => ed.on_scene(events),
                DocEvent::Settings(_) => {
                    ed.build_empty_state();
                    ed.sync_all();
                }
                DocEvent::Colors => ed.sync_all(),
                DocEvent::CommitEdits => {
                    for n in ed.commit_dirty() {
                        ed.inner.ctx.engine().report_commit_failure(format!(
                            "Expression row {n} has an error — fix it or press Esc to discard it before saving."
                        ));
                    }
                }
                DocEvent::Replaced => {
                    let rows: Vec<Rc<Row>> =
                        ed.inner.rows.borrow_mut().drain().map(|(_, r)| r).collect();
                    for r in rows {
                        ed.inner.list.remove(&r.root);
                    }
                    ed.reconcile();
                }
                _ => {}
            }
        });
        self.inner.subs.borrow_mut().push(sub);
        // Hidden (workspace switch, area change) with unsaved typing: the
        // row would otherwise stay uncommitted and a save would write the
        // old value.
        let weak = self.downgrade();
        self.inner.root.connect_unmap(move |_| {
            if let Some(ed) = Expressions::from_weak(&weak) {
                // Failures stay visible on their rows; the save path asks
                // again (CommitEdits) and refuses to drop them silently.
                let _ = ed.commit_dirty();
            }
        });
        self.inner
            .ctx
            .selection()
            .connect_changed_for(self.downgrade(), |inner, _| {
                Expressions { inner }.update_selection_styles();
            });
    }
}
