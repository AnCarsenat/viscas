//! Inspector editor (REQUIREMENTS §2): Blender-like collapsible panels
//! with the properties of the selected node(s).
//!
//! Panels are built when the selection (or the kinds in it) changes; every
//! field registers a refresher that updates it in place from the engine
//! after scene events, leaving the field the user is editing alone. Edits
//! of a multi-selection apply to every node in one undo step; scrub drags
//! are one undo step each (the group closes on release).

pub mod model;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::pango;
use gtk::prelude::*;
use viscas_engine::{Engine, PlotKind};
use viscas_scene::{
    AnimationMode, GridKind, KindTag, LineStyle, NodeContent, NodeId, NodeKind, PropValue, Scene,
    SceneError, SceneEvent, ScriptLanguage, SourceField,
};

use self::model::{CommonProps, Mixed};
use crate::ctx::AppCtx;
use crate::doc::{DocEvent, Subscription};
use crate::editors::scene_tree::{model as tree_model, ops};
use crate::layout::EditorKind;
use crate::theme::model::Color as UiColor;
use crate::widgets::scrub::ScrubSpec;
use crate::widgets::{ColorSwatch, Panel, ScrubField, check_row, property_row};

type Refresher = Rc<dyn Fn(&Engine)>;
/// Field origin and per-node start values of a scrub interaction.
type ScrubBase = (f64, Vec<(NodeId, f64)>);

struct Inner {
    ctx: AppCtx,
    root: gtk::Box,
    body: gtk::Box,
    refreshers: RefCell<Vec<Refresher>>,
    /// Selected ids and kinds the panels were built for.
    shape: RefCell<Vec<(NodeId, KindTag)>>,
    /// A scrub drag is open as an undo group.
    group_open: Cell<bool>,
    /// Per-node start values of the current scrub interaction (relative
    /// multi-edits) and the field value it started from.
    scrub_base: RefCell<Option<ScrubBase>>,
    /// Undo/redo (or a save) interrupted the current scrub: its further
    /// values are ignored until the interaction ends, so the drag cannot
    /// re-apply itself on top of the moved history.
    scrub_suspended: Cell<bool>,
    sub: RefCell<Option<Subscription>>,
}

/// The inspector editor. Cloning shares it.
#[derive(Clone)]
pub struct Inspector {
    inner: Rc<Inner>,
}

fn ui_color(c: viscas_scene::Color) -> UiColor {
    UiColor {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

fn scene_color(c: UiColor) -> viscas_scene::Color {
    viscas_scene::Color::rgba(c.r, c.g, c.b, c.a)
}

/// Is the user typing in `w` (focus inside it)?
fn focused(w: &impl IsA<gtk::Widget>) -> bool {
    w.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN)
}

impl Inspector {
    pub fn new(ctx: &AppCtx) -> Inspector {
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["inspector-body"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&body)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["inspector"])
            .build();
        root.append(&scrolled);
        let insp = Inspector {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                root,
                body,
                refreshers: RefCell::default(),
                shape: RefCell::default(),
                group_open: Cell::new(false),
                scrub_base: RefCell::default(),
                scrub_suspended: Cell::new(false),
                sub: RefCell::default(),
            }),
        };
        let weak = insp.downgrade();
        let sub = ctx.engine().subscribe(move |ev| {
            Self::with_weak(&weak, |i| i.on_doc_event(ev));
        });
        *insp.inner.sub.borrow_mut() = Some(sub);
        let weak = insp.downgrade();
        ctx.selection()
            .connect_selection_for(weak, |inner, _| Inspector { inner }.update(false));
        // A scrub interrupted by the editor going away (area change, window
        // closing) must not leave its undo group open.
        let weak = insp.downgrade();
        insp.inner.root.connect_unmap(move |_| {
            Self::with_weak(&weak, |i| i.end_scrub());
        });
        let weak = insp.downgrade();
        ctx.preferences().connect_changed_for(weak, |inner, _| {
            let i = Inspector { inner };
            if i.inner.shape.borrow().is_empty() {
                i.rebuild();
            }
        });
        insp.rebuild();
        insp
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.inner)
    }

    fn with_weak(weak: &Weak<Inner>, f: impl FnOnce(&Inspector)) {
        if let Some(inner) = weak.upgrade() {
            f(&Inspector { inner });
        }
    }

    fn selected(&self) -> Vec<NodeId> {
        self.inner.ctx.selection().selected()
    }

    fn current_shape(&self) -> Vec<(NodeId, KindTag)> {
        let ids = self.selected();
        self.inner.ctx.engine().with(|e| {
            ids.iter()
                .filter_map(|id| e.scene().get(*id).map(|n| (*id, n.kind.tag())))
                .collect()
        })
    }

    fn on_doc_event(&self, ev: &DocEvent) {
        match ev {
            DocEvent::Scene(events) => {
                let ids = self.selected();
                let relevant = events.iter().any(|e| match e {
                    SceneEvent::Changed { id, .. }
                    | SceneEvent::NodeAdded(id)
                    | SceneEvent::NodeRemoved(id) => ids.contains(id),
                    SceneEvent::Moved { .. } | SceneEvent::GraphChanged { .. } => true,
                    _ => false,
                });
                if relevant {
                    self.update(false);
                }
            }
            DocEvent::Replaced => self.update(true),
            DocEvent::HistoryNavigating => self.interrupt_scrub(),
            DocEvent::CommitEdits => self.seal_group(),
            DocEvent::Settings(_) | DocEvent::Colors => self.update(false),
            _ => {}
        }
    }

    /// Rebuilds when the selection shape changed, refreshes otherwise.
    fn update(&self, force: bool) {
        let shape = self.current_shape();
        if force || shape != *self.inner.shape.borrow() {
            self.rebuild();
        } else {
            let refreshers = self.inner.refreshers.borrow().clone();
            self.inner.ctx.engine().with(|e| {
                for r in &refreshers {
                    r(e);
                }
            });
        }
    }

    fn rebuild(&self) {
        self.close_group();
        // The fields (and any interrupted drag) are replaced: a drag that
        // never reports its end must not keep new fields suspended.
        self.inner.scrub_suspended.set(false);
        let body = &self.inner.body;
        while let Some(c) = body.first_child() {
            body.remove(&c);
        }
        self.inner.refreshers.borrow_mut().clear();
        let shape = self.current_shape();
        *self.inner.shape.borrow_mut() = shape.clone();
        if shape.is_empty() {
            body.append(&self.empty_state());
            return;
        }
        let ids: Vec<NodeId> = shape.iter().map(|(id, _)| *id).collect();
        let single = (ids.len() == 1).then(|| shape[0]);
        body.append(&self.header(&ids));
        body.append(self.node_panel(&ids).widget());
        // Content panels; insensitive while every selected node is locked.
        let mut content: Vec<Panel> = Vec::new();
        if let Some((id, tag)) = single {
            content.extend(self.expression_panel(id, tag));
            match tag {
                KindTag::Slider => content.push(self.slider_panel(id)),
                KindTag::Plot => {
                    content.push(self.ranges_panel(id));
                    content.push(self.analysis_panel(id));
                }
                KindTag::Viewport => content.push(self.viewport_panel(id)),
                _ => {}
            }
        }
        // Style only where it draws something.
        if shape.iter().any(|(_, t)| style_applies(*t)) {
            content.push(self.style_panel(&ids));
        }
        for p in &content {
            body.append(p.widget());
        }
        let ids_c = ids.clone();
        self.add_refresher(move |e| {
            let locked = ids_c
                .iter()
                .all(|i| e.scene().get(*i).is_some_and(|n| n.locked));
            for p in &content {
                p.widget().set_sensitive(!locked);
            }
        });
        if let Some((id, _)) = single {
            body.append(self.deps_panel(id).widget());
        }
        self.update(false);
    }

    fn empty_state(&self) -> gtk::Widget {
        let playful = self.inner.ctx.preferences().with(|p| p.interface.playful);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .margin_top(32)
            .css_classes(["placeholder"])
            .build();
        column.append(
            &gtk::Image::builder()
                .icon_name(crate::editors::icon_name(EditorKind::Inspector))
                .css_classes(["placeholder-icon"])
                .build(),
        );
        column.append(
            &gtk::Label::builder()
                .label("Nothing selected")
                .css_classes(["placeholder-title"])
                .build(),
        );
        let text = if playful {
            EditorKind::Inspector.quip()
        } else {
            "Select a node in the scene tree or the viewport to edit its properties."
        };
        column.append(
            &gtk::Label::builder()
                .label(text)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .css_classes([if playful {
                    "placeholder-quip"
                } else {
                    "placeholder-desc"
                }])
                .build(),
        );
        column.upcast()
    }

    fn header(&self, ids: &[NodeId]) -> gtk::Widget {
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["inspector-title"])
            .build();
        let glyph = gtk::Label::builder().css_classes(["kind-glyph"]).build();
        let row = gtk::Box::builder()
            .spacing(6)
            .css_classes(["inspector-header"])
            .build();
        row.append(&glyph);
        row.append(&label);
        let ids = ids.to_vec();
        self.add_refresher(move |e| {
            let nodes: Vec<_> = ids.iter().filter_map(|i| e.scene().get(*i)).collect();
            if let [n] = nodes.as_slice() {
                let tag = n.kind.tag();
                glyph.set_label(tree_model::kind_glyph(tag));
                glyph.set_css_classes(&["kind-glyph", &format!("kind-{}", tag.as_str())]);
                let value = e
                    .value(n.id)
                    .map(|v| v.display(e.settings().approx).to_owned());
                label.set_label(&tree_model::row_text_in(e.scene(), n, value.as_deref()).title);
            } else {
                glyph.set_label("≡");
                label.set_label(&model::selection_summary(&nodes));
            }
        });
        row.upcast()
    }

    fn add_refresher(&self, f: impl Fn(&Engine) + 'static) {
        self.inner.refreshers.borrow_mut().push(Rc::new(f));
    }

    // ------------------------------------------------------------------
    // Edits
    // ------------------------------------------------------------------

    /// Applies `f` to every selected, unlocked node in one undo step.
    fn edit_all(
        &self,
        label: &str,
        f: impl Fn(&mut Scene, NodeId) -> Result<(), SceneError>,
    ) -> Result<(), SceneError> {
        self.close_group();
        let ids = self.selected();
        self.inner.ctx.engine().edit(|e| {
            let s = e.scene_mut();
            let ids: Vec<NodeId> = ids
                .into_iter()
                // Locked nodes only take visibility / lock toggles.
                .filter(|i| {
                    matches!(label, "Visible" | "Locked") || s.get(*i).is_some_and(|n| !n.locked)
                })
                .collect();
            let r = s.transaction(label, |s| ids.iter().try_for_each(|id| f(s, *id)));
            s.seal_history();
            r
        })
    }

    /// Edits every selected node's content inside one open undo group,
    /// coalescing by `key` (scrub drags); closed by [`Self::close_group`].
    fn scrub_values(
        &self,
        label: &str,
        key: &str,
        targets: &[(NodeId, f64)],
        f: &dyn Fn(&mut NodeContent, f64),
    ) {
        if self.inner.scrub_suspended.get() {
            return;
        }
        let open = self.inner.group_open.replace(true);
        self.inner.ctx.engine().edit(|e| {
            let s = e.scene_mut();
            if !open {
                s.begin_group(label);
            }
            for &(id, v) in targets {
                if s.get(id).is_some_and(|n| !n.locked) {
                    let _ = s.update_coalesced(id, key, |c| f(c, v));
                }
            }
        });
    }

    /// Mutates the engine for a non-scrub edit: any open scrub group is
    /// closed first, so the edit never joins a drag's undo step.
    fn edit<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        self.close_group();
        self.inner.ctx.engine().edit(f)
    }

    /// Ends a scrub interaction (release, arrow click, typed value, the
    /// editor going away): closes its undo group and lifts a suspension.
    fn end_scrub(&self) {
        self.close_group();
        if self.inner.scrub_suspended.replace(false) {
            // Show the values the history moved to.
            self.update(false);
        }
    }

    /// A save during a scrub: closes the open undo group so the saved
    /// document holds the current values; the drag goes on (its next
    /// value opens a fresh group from the same start values).
    fn seal_group(&self) {
        if self.inner.group_open.replace(false) {
            self.inner.ctx.engine().edit(|e| {
                let s = e.scene_mut();
                s.end_group();
                s.seal_history();
            });
        }
    }

    /// Undo/redo while a scrub holds an undo group open: close the group
    /// now (one complete step) and ignore the rest of the drag.
    fn interrupt_scrub(&self) {
        if self.inner.group_open.get() {
            self.close_group();
            self.inner.scrub_suspended.set(true);
        }
    }

    fn close_group(&self) {
        self.inner.scrub_base.borrow_mut().take();
        self.seal_group();
    }

    /// A scrub field editing a number of every selected node.
    fn scrub(
        &self,
        spec: ScrubSpec,
        key: &'static str,
        get: impl Fn(&viscas_scene::Node) -> f64 + 'static,
        set: impl Fn(&mut NodeContent, f64) + 'static,
    ) -> ScrubField {
        let field = ScrubField::new(spec, 0.0);
        let get = Rc::new(get);
        let weak = self.downgrade();
        let weak_field = field.downgrade();
        let ids = self.selected();
        let (g, ids_c) = (get.clone(), ids.clone());
        field.connect_changed(move |v| {
            let Some(f) = weak_field.upgrade() else {
                return;
            };
            Self::with_weak(&weak, |i| {
                // Dragging or stepping moves every node by the same delta
                // from its own start value (mixed values stay distinct);
                // a typed value is set as is.
                if i.inner.scrub_base.borrow().is_none() {
                    let bases = i.inner.ctx.engine().with(|e| {
                        ids_c
                            .iter()
                            .filter_map(|id| e.scene().get(*id).map(|n| (*id, g(n))))
                            .collect()
                    });
                    *i.inner.scrub_base.borrow_mut() = Some((f.origin(), bases));
                }
                let targets: Vec<(NodeId, f64)> = match &*i.inner.scrub_base.borrow() {
                    Some((origin, bases)) if !f.is_typed() => bases
                        .iter()
                        .map(|(id, b)| (*id, model::shifted(*b, *origin, v, &f.spec())))
                        .collect(),
                    _ => ids_c.iter().map(|id| (*id, v)).collect(),
                };
                i.scrub_values("Edit", key, &targets, &set);
            });
        });
        let weak = self.downgrade();
        field.connect_finished(move || Self::with_weak(&weak, |i| i.end_scrub()));
        let f = field.clone();
        self.add_refresher(move |e| {
            if f.is_active() {
                return;
            }
            let m = model::common(ids.iter().filter_map(|i| e.scene().get(*i)).map(&*get));
            match m {
                Mixed::Same(v) => f.set_value(v),
                _ => f.set_mixed(true),
            }
        });
        field
    }

    /// A check button over a boolean of every selected node.
    fn check(
        &self,
        label: &str,
        get: impl Fn(&viscas_scene::Node) -> bool + 'static,
        set: impl Fn(&mut Scene, NodeId, bool) -> Result<(), SceneError> + 'static,
    ) -> gtk::Box {
        let (row, check) = check_row(label, false);
        let updating = Rc::new(Cell::new(false));
        let weak = self.downgrade();
        let up = updating.clone();
        let title = label.to_owned();
        check.connect_toggled(move |c| {
            if up.get() {
                return;
            }
            let v = c.is_active();
            c.set_inconsistent(false);
            Self::with_weak(&weak, |i| {
                let _ = i.edit_all(&title, |s, id| set(s, id, v));
            });
        });
        let ids = self.selected();
        self.add_refresher(move |e| {
            let m = model::common(ids.iter().filter_map(|i| e.scene().get(*i)).map(&get));
            updating.set(true);
            check.set_inconsistent(m.is_mixed());
            if let Mixed::Same(v) = m {
                check.set_active(v);
            }
            updating.set(false);
        });
        row
    }

    /// A drop-down over a choice of every selected node; a "—" item is
    /// shown while the values differ.
    fn choice<T: PartialEq + Copy + 'static>(
        &self,
        options: &'static [(T, &'static str)],
        get: impl Fn(&viscas_scene::Node) -> Option<T> + 'static,
        set: impl Fn(&mut NodeContent, T) + 'static,
    ) -> gtk::DropDown {
        let names: Vec<&str> = options.iter().map(|(_, n)| *n).collect();
        let list = gtk::StringList::new(&names);
        let dd = gtk::DropDown::builder().model(&list).build();
        let updating = Rc::new(Cell::new(false));
        let weak = self.downgrade();
        let up = updating.clone();
        dd.connect_selected_notify(move |dd| {
            if up.get() {
                return;
            }
            let Some((v, _)) = options.get(dd.selected() as usize) else {
                return;
            };
            Self::with_weak(&weak, |i| {
                let _ = i.edit_all("Edit", |s, id| s.update(id, |c| set(c, *v)));
            });
        });
        let ids = self.selected();
        let d = dd.clone();
        self.add_refresher(move |e| {
            let m = model::common(ids.iter().filter_map(|i| e.scene().get(*i)).map(&get));
            updating.set(true);
            let has_dash = list.n_items() as usize > options.len();
            match m {
                Mixed::Same(Some(v)) => {
                    if has_dash {
                        list.remove(options.len() as u32);
                    }
                    if let Some(p) = options.iter().position(|(o, _)| *o == v) {
                        d.set_selected(p as u32);
                    }
                }
                _ => {
                    if !has_dash {
                        list.append("—");
                    }
                    d.set_selected(options.len() as u32);
                }
            }
            updating.set(false);
        });
        dd
    }

    /// A text entry. `live`: every change is applied (coalesced, sealed
    /// when focus leaves); otherwise applied on Enter / focus out. Errors
    /// of `field` show under it, with their span underlined.
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &self,
        panel: &Panel,
        label: &str,
        live: bool,
        mono: bool,
        get: impl Fn(&Engine) -> Mixed<String> + 'static,
        set: impl Fn(&Inspector, &str) -> Result<(), String> + 'static,
        field: Option<(NodeId, SourceField)>,
    ) -> gtk::Entry {
        let entry = gtk::Entry::builder().hexpand(true).build();
        if mono {
            entry.add_css_class("mono");
        }
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["field-error"])
            .visible(false)
            .build();
        panel.append(&property_row(label, &entry));
        panel.append(&error);
        let updating = Rc::new(Cell::new(false));
        let local_err: Rc<RefCell<Option<String>>> = Rc::default();
        let set = Rc::new(set);
        let apply = {
            let weak = self.downgrade();
            let err = error.clone();
            let local = local_err.clone();
            move |text: &str| {
                Self::with_weak(&weak, |i| match set(i, text) {
                    Ok(()) => {
                        local.borrow_mut().take();
                    }
                    Err(m) => {
                        err.set_label(&m);
                        err.set_visible(true);
                        *local.borrow_mut() = Some(m);
                    }
                });
            }
        };
        let apply = Rc::new(apply);
        if live {
            let up = updating.clone();
            let a = apply.clone();
            entry.connect_changed(move |e| {
                if !up.get() {
                    a(&e.text());
                }
            });
            let weak = self.downgrade();
            let focus = gtk::EventControllerFocus::new();
            focus.connect_leave(move |_| {
                Self::with_weak(&weak, |i| {
                    i.edit(|e| e.scene_mut().seal_history());
                });
            });
            entry.add_controller(focus);
        } else {
            let a = apply.clone();
            entry.connect_activate(move |e| a(&e.text()));
            let a = apply.clone();
            let e2 = entry.clone();
            let focus = gtk::EventControllerFocus::new();
            focus.connect_leave(move |_| a(&e2.text()));
            entry.add_controller(focus);
        }
        let e = entry.clone();
        self.add_refresher(move |eng| {
            let m = get(eng);
            if !focused(&e) {
                updating.set(true);
                let text = m.value().unwrap_or_default();
                if e.text() != text {
                    e.set_text(&text);
                }
                if m.is_mixed() {
                    e.set_placeholder_text(Some("—"));
                } else if e.placeholder_text().as_deref() == Some("—") {
                    e.set_placeholder_text(None);
                }
                updating.set(false);
                if !live {
                    local_err.borrow_mut().take();
                }
            }
            let mut errors = field
                .map(|(id, f)| model::field_errors(eng.scene(), id, f))
                .unwrap_or_default();
            if let Some(m) = local_err.borrow().clone() {
                errors.insert(
                    0,
                    model::FieldError {
                        message: m,
                        span: None,
                    },
                );
            }
            let attrs = pango::AttrList::new();
            let text = e.text();
            for er in &errors {
                if let Some((s, en)) = er.span.and_then(|sp| model::clamp_span(&text, sp)) {
                    let mut a = pango::AttrInt::new_underline(pango::Underline::Error);
                    a.set_start_index(s as u32);
                    a.set_end_index(en as u32);
                    attrs.insert(a);
                }
            }
            e.set_attributes(&attrs);
            if errors.is_empty() {
                error.set_visible(false);
                e.remove_css_class("error");
            } else {
                let msg: Vec<String> = errors.iter().map(|x| x.message.clone()).collect();
                error.set_label(&msg.join("\n"));
                error.set_visible(true);
                e.add_css_class("error");
            }
        });
        entry
    }

    /// A live entry over one source field of node `id`.
    fn source_entry(&self, panel: &Panel, label: &str, id: NodeId, field: SourceField) {
        self.entry(
            panel,
            label,
            true,
            true,
            move |e| match e.scene().get(id).and_then(|n| n.kind.source(field)) {
                Some(t) => Mixed::Same(t.to_owned()),
                None => Mixed::Empty,
            },
            move |i, text| {
                i.edit(|e| e.scene_mut().set_source(id, field, text))
                    .map_err(|e| e.to_string())
            },
            Some((id, field)),
        );
    }

    // ------------------------------------------------------------------
    // Panels
    // ------------------------------------------------------------------

    fn node_panel(&self, ids: &[NodeId]) -> Panel {
        let panel = Panel::new("Node", true);
        let single = (ids.len() == 1).then(|| ids[0]);
        let name = self.entry(
            &panel,
            "Name",
            false,
            false,
            move |e| match single.and_then(|id| e.scene().get(id)) {
                Some(n) => Mixed::Same(n.name.clone().unwrap_or_default()),
                None => Mixed::Different,
            },
            move |i, text| match single {
                Some(id) => {
                    i.close_group();
                    ops::rename(&i.inner.ctx, id, text).map(|_| ())
                }
                None => Ok(()),
            },
            None,
        );
        // Unnamed drawables show the automatic name they display as.
        let n = name.clone();
        self.add_refresher(move |e| {
            if let Some(label) = single.and_then(|id| e.scene().auto_label(id)) {
                n.set_placeholder_text(Some(&label));
            }
        });
        // Locked nodes cannot be renamed (ops::rename refuses too).
        let n = name.clone();
        self.add_refresher(move |e| {
            let locked = single
                .and_then(|id| e.scene().get(id))
                .is_none_or(|n| n.locked);
            n.set_sensitive(!locked);
        });
        let kind = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim"])
            .build();
        panel.append(&property_row("Kind", &kind));
        let path = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Start)
            .css_classes(["dim", "mono"])
            .build();
        panel.append(&property_row("Parent", &path));
        let ids_c = ids.to_vec();
        self.add_refresher(move |e| {
            let nodes: Vec<_> = ids_c.iter().filter_map(|i| e.scene().get(*i)).collect();
            let props = CommonProps::of(&nodes);
            kind.set_label(match props.kind {
                Mixed::Same(t) => tree_model::kind_title(t),
                _ => "—",
            });
            let paths = model::common(ids_c.iter().map(|i| tree_model::parent_path(e.scene(), *i)));
            path.set_label(&match paths {
                Mixed::Same(p) if p.is_empty() => "(top level)".to_owned(),
                Mixed::Same(p) => p,
                _ => "—".to_owned(),
            });
        });
        panel.append(&self.check("Visible", |n| n.visible, |s, id, v| s.set_visible(id, v)));
        panel.append(&self.check("Locked", |n| n.locked, |s, id, v| s.set_locked(id, v)));
        panel
    }

    fn expression_panel(&self, id: NodeId, tag: KindTag) -> Option<Panel> {
        let panel = Panel::new("Expression", true);
        match tag {
            KindTag::Variable | KindTag::Slider => {
                self.source_entry(&panel, "Value", id, SourceField::Main)
            }
            KindTag::Point => self.source_entry(&panel, "Coordinates", id, SourceField::Main),
            KindTag::List => self.source_entry(&panel, "Elements", id, SourceField::Main),
            KindTag::Plot => self.source_entry(&panel, "Relation", id, SourceField::Main),
            KindTag::Action | KindTag::Regression => {
                self.source_entry(&panel, "Statement", id, SourceField::Main)
            }
            KindTag::Function => {
                self.entry(
                    &panel,
                    "Parameters",
                    false,
                    true,
                    move |e| match e.scene().get(id).map(|n| &n.kind) {
                        Some(NodeKind::Function { params, .. }) => Mixed::Same(params.join(", ")),
                        _ => Mixed::Empty,
                    },
                    move |i, text| {
                        let params = model::parse_params(text);
                        i.edit(|e| {
                            e.scene_mut().update(id, |c| {
                                if let NodeKind::Function { params: p, .. } = &mut c.kind {
                                    *p = params;
                                }
                            })
                        })
                        .map_err(|e| e.to_string())
                    },
                    None,
                );
                self.source_entry(&panel, "Body", id, SourceField::Main);
            }
            KindTag::Script => {
                panel.append(&property_row(
                    "Language",
                    &self.choice(
                        &[
                            (ScriptLanguage::Viscas, "Viscas"),
                            (ScriptLanguage::Rhai, "Rhai"),
                        ],
                        |n| match n.kind {
                            NodeKind::Script { language, .. } => Some(language),
                            _ => None,
                        },
                        |c, v| {
                            if let NodeKind::Script { language, .. } = &mut c.kind {
                                *language = v;
                            }
                        },
                    ),
                ));
                self.text_view(&panel, id, true);
            }
            KindTag::Note => self.text_view(&panel, id, false),
            KindTag::Table => {
                let info = gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim"])
                    .build();
                panel.append(&info);
                self.add_refresher(move |e| {
                    if let Some(NodeKind::Table { columns }) = e.scene().get(id).map(|n| &n.kind) {
                        let rows = columns.iter().map(|c| c.cells.len()).max().unwrap_or(0);
                        info.set_label(&format!(
                            "{} columns × {rows} rows — edit cells in the Sheet editor.",
                            columns.len()
                        ));
                    }
                });
            }
            KindTag::Folder | KindTag::Viewport => return None,
        }
        if tree_model::has_status(tag) {
            let value = gtk::Label::builder()
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .css_classes(["value-preview", "mono"])
                .build();
            panel.append(&property_row("Result", &value));
            self.add_refresher(move |e| match tree_model::status_dot(e.scene(), id) {
                tree_model::Dot::Error(m) => {
                    value.set_label(&m);
                    value.add_css_class("error");
                }
                dot => {
                    value.remove_css_class("error");
                    let text = e
                        .value(id)
                        .map(|v| format!("= {}", v.display(e.settings().approx)))
                        .or_else(|| e.plot_of(id).map(|p| format!("{} plot", p.kind)))
                        .unwrap_or_else(|| {
                            if dot == tree_model::Dot::Pending {
                                "…"
                            } else {
                                "ok"
                            }
                            .to_owned()
                        });
                    value.set_label(&text);
                }
            });
        }
        Some(panel)
    }

    /// Multi-line text: script source or note text.
    fn text_view(&self, panel: &Panel, id: NodeId, mono: bool) {
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(4)
            .bottom_margin(4)
            .left_margin(6)
            .right_margin(6)
            .css_classes(["inspector-text"])
            .build();
        view.set_monospace(mono);
        let frame = gtk::ScrolledWindow::builder()
            .child(&view)
            .min_content_height(96)
            .max_content_height(320)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["inspector-text-frame"])
            .build();
        panel.append(&frame);
        let updating = Rc::new(Cell::new(false));
        let weak = self.downgrade();
        let up = updating.clone();
        view.buffer().connect_changed(move |b| {
            if up.get() {
                return;
            }
            let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            Self::with_weak(&weak, |i| {
                let _ = i.edit(|e| e.scene_mut().set_source(id, SourceField::Main, &text));
            });
        });
        let weak = self.downgrade();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            Self::with_weak(&weak, |i| {
                i.edit(|e| e.scene_mut().seal_history());
            });
        });
        view.add_controller(focus);
        self.add_refresher(move |e| {
            if focused(&view) {
                return;
            }
            let text = e
                .scene()
                .get(id)
                .and_then(|n| tree_model::main_source(&n.kind).map(str::to_owned))
                .unwrap_or_default();
            let b = view.buffer();
            if b.text(&b.start_iter(), &b.end_iter(), false) != text {
                updating.set(true);
                b.set_text(&text);
                updating.set(false);
            }
        });
    }

    fn slider_panel(&self, id: NodeId) -> Panel {
        let panel = Panel::new("Slider", true);
        // The scrub follows the *evaluated* bounds and step, updated on
        // every refresh (bounds may be expressions such as `2a`). When they
        // do not evaluate, the scrub is disabled and says why.
        let value = ScrubField::new(ScrubSpec::new(0.0, 1.0, 0.01, 2), 0.0);
        let weak = self.downgrade();
        let weak_field = value.downgrade();
        value.connect_changed(move |v| {
            let Some(f) = weak_field.upgrade() else {
                return;
            };
            let digits = f.spec().digits as usize;
            Self::with_weak(&weak, |i| {
                i.scrub_values("Slide", "source:main", &[(id, v)], &|c, v| {
                    if let Some(slot) = c.kind.source_mut(SourceField::Main) {
                        *slot = crate::theme::model::trim_float(v, digits);
                    }
                });
            });
        });
        let weak = self.downgrade();
        value.connect_finished(move || Self::with_weak(&weak, |i| i.end_scrub()));
        let f = value.clone();
        self.add_refresher(move |e| {
            if f.is_active() {
                return;
            }
            match slider_bounds(e, id).and_then(|(lo, hi, st)| model::slider_spec(lo, hi, st)) {
                Ok(spec) => {
                    f.set_spec(spec);
                    f.widget().set_sensitive(true);
                    f.set_tooltip("Drag to scrub (Shift: fine)");
                    match e
                        .value(id)
                        .and_then(|v| v.value.as_number().map(|_| v.value.to_f64()))
                    {
                        Some(v) if v.is_finite() => f.set_value(v),
                        _ => f.set_mixed(true),
                    }
                }
                Err(msg) => {
                    f.widget().set_sensitive(false);
                    f.set_mixed(true);
                    f.set_tooltip(&format!("Cannot scrub: {msg}"));
                }
            }
        });
        panel.append(&property_row("Value", value.widget()));
        self.source_entry(&panel, "Minimum", id, SourceField::Min);
        self.source_entry(&panel, "Maximum", id, SourceField::Max);
        self.source_entry(&panel, "Step", id, SourceField::Step);
        panel.append(&property_row(
            "Animate",
            &self.choice(
                &[
                    (AnimationMode::Off, "Off"),
                    (AnimationMode::Loop, "Loop"),
                    (AnimationMode::Bounce, "Bounce"),
                    (AnimationMode::Once, "Once"),
                ],
                |n| match &n.kind {
                    NodeKind::Slider { animation, .. } => Some(animation.mode),
                    _ => None,
                },
                |c, v| {
                    if let NodeKind::Slider { animation, .. } = &mut c.kind {
                        animation.mode = v;
                    }
                },
            ),
        ));
        let speed = self.scrub(
            ScrubSpec::new(0.05, 20.0, 0.05, 2),
            "anim-speed",
            |n| match &n.kind {
                NodeKind::Slider { animation, .. } => animation.speed,
                _ => 1.0,
            },
            |c, v| {
                if let NodeKind::Slider { animation, .. } = &mut c.kind {
                    animation.speed = v;
                }
            },
        );
        speed.set_suffix("×");
        panel.append(&property_row("Speed", speed.widget()));
        panel.append(&self.check(
            "Playing",
            |n| matches!(&n.kind, NodeKind::Slider { animation, .. } if animation.playing),
            |s, id, v| {
                s.update(id, |c| {
                    if let NodeKind::Slider { animation, .. } = &mut c.kind {
                        animation.playing = v;
                    }
                })
            },
        ));
        panel
    }

    /// Parameter ranges of parametric (`t`) and polar (`θ`) plots; rows
    /// show for the plot kind the relation currently compiles to.
    fn ranges_panel(&self, id: NodeId) -> Panel {
        let panel = Panel::new("Ranges", true);
        let hint = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim"])
            .label("Explicit and implicit plots follow the viewport.")
            .build();
        panel.append(&hint);
        let mut rows: Vec<(gtk::Widget, PlotKind)> = Vec::new();
        for (prefix, label, kind, default) in [
            ("t", "t", PlotKind::Parametric, ("0", "2π")),
            ("theta", "θ", PlotKind::Polar, ("0", "2π")),
        ] {
            for (end, name, placeholder) in [("min", "Min", default.0), ("max", "Max", default.1)] {
                let key = format!("{prefix}_{end}");
                let k2 = key.clone();
                let entry = self.entry(
                    &panel,
                    &format!("{label} {name}"),
                    false,
                    true,
                    move |e| {
                        Mixed::Same(model::prop_text(
                            e.scene().get(id).and_then(|n| n.props.get(&k2)),
                        ))
                    },
                    move |i, text| {
                        let key = key.clone();
                        let v: Option<PropValue> = model::prop_from_text(text);
                        i.edit(|e| {
                            let s = e.scene_mut();
                            let r = match v {
                                Some(v) => s.set_prop(id, &key, v),
                                None => s.remove_prop(id, &key),
                            };
                            s.seal_history();
                            r
                        })
                        .map_err(|e| e.to_string())
                    },
                    None,
                );
                entry.set_placeholder_text(Some(placeholder));
                if let Some(row) = entry.parent() {
                    rows.push((row, kind));
                }
            }
        }
        let p = panel.clone();
        self.add_refresher(move |e| {
            let kind = e.plot_of(id).map(|p| p.kind).or_else(|| {
                // Not compiled (error): guess from the relation text.
                match e.scene().get(id).map(|n| &n.kind) {
                    Some(NodeKind::Plot { relation }) => {
                        let r = relation.trim_start();
                        if r.starts_with('(') {
                            Some(PlotKind::Parametric)
                        } else if r.starts_with("r ") || r.starts_with("r=") {
                            Some(PlotKind::Polar)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            });
            let mut any = false;
            for (row, k) in &rows {
                let show = Some(*k) == kind;
                row.set_visible(show);
                any |= show;
            }
            hint.set_visible(!any);
            let _ = &p;
        });
        panel
    }

    fn viewport_panel(&self, _id: NodeId) -> Panel {
        let panel = Panel::new("Viewport", true);
        let spec = ScrubSpec::new(-1e6, 1e6, 0.5, 2);
        type Get = fn(&viscas_scene::Bounds) -> f64;
        type Set = fn(&mut viscas_scene::Bounds, f64);
        let fields: [(&str, &'static str, Get, Set); 4] = [
            ("X Min", "bounds:x_min", |b| b.x_min, |b, v| b.x_min = v),
            ("X Max", "bounds:x_max", |b| b.x_max, |b, v| b.x_max = v),
            ("Y Min", "bounds:y_min", |b| b.y_min, |b, v| b.y_min = v),
            ("Y Max", "bounds:y_max", |b| b.y_max, |b, v| b.y_max = v),
        ];
        for (label, key, get, set) in fields {
            let f = self.scrub(
                spec,
                key,
                move |n| match &n.kind {
                    NodeKind::Viewport { bounds, .. } => get(bounds),
                    _ => 0.0,
                },
                move |c, v| {
                    if let NodeKind::Viewport { bounds, .. } = &mut c.kind {
                        set(bounds, v);
                    }
                },
            );
            panel.append(&property_row(label, f.widget()));
        }
        panel.append(&property_row(
            "Grid",
            &self.choice(
                &[
                    (GridKind::Cartesian, "Cartesian"),
                    (GridKind::Polar, "Polar"),
                    (GridKind::None, "None"),
                ],
                |n| match &n.kind {
                    NodeKind::Viewport { grid, .. } => Some(*grid),
                    _ => None,
                },
                |c, v| {
                    if let NodeKind::Viewport { grid, .. } = &mut c.kind {
                        *grid = v;
                    }
                },
            ),
        ));
        type Flag = fn(&mut NodeKind) -> Option<&mut bool>;
        let flags: [(&str, Flag); 4] = [
            ("Minor Grid", |k| match k {
                NodeKind::Viewport { minor_grid, .. } => Some(minor_grid),
                _ => None,
            }),
            ("Axes", |k| match k {
                NodeKind::Viewport { axes, .. } => Some(axes),
                _ => None,
            }),
            ("Axis Numbers", |k| match k {
                NodeKind::Viewport { axis_numbers, .. } => Some(axis_numbers),
                _ => None,
            }),
            ("Lock Aspect", |k| match k {
                NodeKind::Viewport { lock_aspect, .. } => Some(lock_aspect),
                _ => None,
            }),
        ];
        for (label, flag) in flags {
            panel.append(&self.check(
                label,
                move |n| {
                    let mut k = n.kind.clone();
                    flag(&mut k).is_some_and(|b| *b)
                },
                move |s, id, v| {
                    s.update(id, |c| {
                        if let Some(b) = flag(&mut c.kind) {
                            *b = v;
                        }
                    })
                },
            ));
        }
        panel
    }

    fn style_panel(&self, ids: &[NodeId]) -> Panel {
        let folders = self.inner.ctx.engine().with(|e| {
            ids.iter().all(|i| {
                e.scene()
                    .get(*i)
                    .is_some_and(|n| matches!(n.kind, NodeKind::Folder))
            })
        });
        let panel = Panel::new(
            if folders {
                "Style (applies to children)"
            } else {
                "Style"
            },
            true,
        );
        // Color: swatch (auto color shown when unset) and an Auto button.
        let swatch = ColorSwatch::new(UiColor::hex(0x808080), "Node Color");
        let weak = self.downgrade();
        swatch.connect_changed(move |c| {
            Self::with_weak(&weak, |i| {
                let _ = i.edit_all("Change color", |s, id| {
                    s.update(id, |n| n.style.color = Some(scene_color(c)))
                });
            });
        });
        let mixed = gtk::Label::builder()
            .label("—")
            .can_target(false)
            .css_classes(["mixed-mark"])
            .build();
        let overlay = gtk::Overlay::builder()
            .child(swatch.widget())
            .hexpand(true)
            .build();
        overlay.add_overlay(&mixed);
        let auto = gtk::Button::builder()
            .label("Auto")
            .tooltip_text("Use the automatic palette color")
            .css_classes(["flat", "auto-color"])
            .build();
        let weak = self.downgrade();
        auto.connect_clicked(move |_| {
            Self::with_weak(&weak, |i| {
                let _ = i.edit_all("Automatic color", |s, id| {
                    s.update(id, |n| n.style.color = None)
                });
            });
        });
        let color_box = gtk::Box::builder().spacing(4).build();
        color_box.append(&overlay);
        color_box.append(&auto);
        panel.append(&property_row("Color", &color_box));
        let ids_c = ids.to_vec();
        self.add_refresher(move |e| {
            let nodes: Vec<_> = ids_c.iter().filter_map(|i| e.scene().get(*i)).collect();
            let props = CommonProps::of(&nodes);
            mixed.set_visible(props.color.is_mixed());
            auto.set_sensitive(!matches!(props.color, Mixed::Same(None)));
            if let Some(c) = ids_c.last().and_then(|i| e.node_color(*i)) {
                swatch.set_color(ui_color(c));
            }
        });

        let width = self.scrub(
            ScrubSpec::new(0.5, 12.0, 0.5, 1),
            "style:line_width",
            |n| n.style.line_width,
            |c, v| c.style.line_width = v,
        );
        width.set_suffix(" px");
        panel.append(&property_row("Line Width", width.widget()));
        panel.append(&property_row(
            "Line Style",
            &self.choice(
                &[
                    (LineStyle::Solid, "Solid"),
                    (LineStyle::Dashed, "Dashed"),
                    (LineStyle::Dotted, "Dotted"),
                ],
                |n| Some(n.style.line_style),
                |c, v| c.style.line_style = v,
            ),
        ));
        let opacity = self.scrub(
            ScrubSpec::new(0.0, 1.0, 0.05, 2),
            "style:opacity",
            |n| n.style.opacity,
            |c, v| c.style.opacity = v,
        );
        panel.append(&property_row("Opacity", opacity.widget()));
        let ids_c = ids.to_vec();
        self.entry(
            &panel,
            "Label",
            false,
            false,
            move |e| {
                let nodes: Vec<_> = ids_c.iter().filter_map(|i| e.scene().get(*i)).collect();
                CommonProps::of(&nodes).label
            },
            move |i, text| {
                let text = text.trim().to_owned();
                // Unchanged mixed labels stay as they are.
                if text.is_empty()
                    && i.inner.ctx.engine().with(|e| {
                        let nodes: Vec<_> = i
                            .selected()
                            .iter()
                            .filter_map(|id| e.scene().get(*id))
                            .collect();
                        CommonProps::of(&nodes).label.is_mixed()
                    })
                {
                    return Ok(());
                }
                i.edit_all("Label", |s, id| {
                    s.update(id, |n| {
                        n.style.label = (!text.is_empty()).then(|| text.clone());
                    })
                })
                .map_err(|e| e.to_string())
            },
            None,
        );
        panel.append(&self.check(
            "Show Label",
            |n| n.style.show_label,
            |s, id, v| s.update(id, |n| n.style.show_label = v),
        ));
        panel
    }

    /// Roots, extrema and intersections of a plot in the current view:
    /// one toggle per group, click a value to copy it, `+` adds it to the
    /// board as a Point node.
    fn analysis_panel(&self, id: NodeId) -> Panel {
        use viscas_plot::FeatureKind as K;
        let panel = Panel::new("Analysis", true);
        type Group = (&'static str, fn(K) -> bool);
        let groups: [Group; 3] = [
            ("Roots", |k| k == K::Root),
            ("Extrema", |k| matches!(k, K::Minimum | K::Maximum)),
            ("Intersections", |k| k == K::Intersection),
        ];
        let mut lists = Vec::new();
        for (title, _) in groups {
            let list = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(1)
                .css_classes(["analysis-list"])
                .build();
            let check = gtk::CheckButton::builder()
                .label(title)
                .active(true)
                .build();
            let l = list.clone();
            check.connect_toggled(move |c| l.set_visible(c.is_active()));
            panel.append(&check);
            panel.append(&list);
            lists.push(list);
        }
        let hint = gtk::Label::builder()
            .label("Computed in the visible part of the viewport.")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim", "analysis-hint"])
            .build();
        panel.append(&hint);

        let weak = self.downgrade();
        type Key = Vec<(K, u64, u64)>;
        let last: Rc<RefCell<Option<Key>>> = Rc::default();
        self.add_refresher(move |e| {
            let hits = e.node_features(id, None);
            let key: Vec<(K, u64, u64)> = hits
                .iter()
                .map(|h| {
                    (
                        h.feature.kind,
                        h.feature.pos.x.to_bits(),
                        h.feature.pos.y.to_bits(),
                    )
                })
                .collect();
            if last.borrow().as_ref() == Some(&key) {
                return;
            }
            *last.borrow_mut() = Some(key);
            for ((_, wanted), list) in groups.iter().zip(&lists) {
                while let Some(c) = list.first_child() {
                    list.remove(&c);
                }
                let mut any = false;
                for h in hits.iter().filter(|h| wanted(h.feature.kind)) {
                    any = true;
                    list.append(&analysis_row(&weak, h));
                }
                if !any {
                    list.append(
                        &gtk::Label::builder()
                            .label("—")
                            .xalign(0.0)
                            .css_classes(["dim", "deps-none"])
                            .build(),
                    );
                }
            }
        });
        panel
    }

    fn deps_panel(&self, id: NodeId) -> Panel {
        let panel = Panel::new("Dependencies", true);
        let make = |title: &str| {
            let flow = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .column_spacing(4)
                .row_spacing(4)
                .max_children_per_line(12)
                .css_classes(["chips"])
                .build();
            let label = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["deps-title"])
                .build();
            panel.append(&label);
            panel.append(&flow);
            (label, flow)
        };
        let (inputs_title, inputs) = make("Inputs");
        let (dependents_title, dependents) = make("Dependents");
        // Shown instead of both lists when the node is unconnected.
        let alone = gtk::Label::builder()
            .label("No inputs or dependents")
            .xalign(0.0)
            .css_classes(["dim", "deps-none"])
            .build();
        panel.append(&alone);
        type Key = (Vec<NodeId>, Vec<NodeId>, Vec<String>);
        let last: Rc<RefCell<Key>> = Rc::default();
        let weak = self.downgrade();
        self.add_refresher(move |e| {
            let g = e.scene().graph();
            let ups = g.direct_upstream(id);
            let downs = g.direct_downstream(id);
            let titles: Vec<String> = ups
                .iter()
                .chain(&downs)
                .map(|i| tree_model::display_name(e.scene(), *i))
                .collect();
            let key = (ups.clone(), downs.clone(), titles);
            if *last.borrow() == key {
                return;
            }
            *last.borrow_mut() = key;
            let unconnected = ups.is_empty() && downs.is_empty();
            alone.set_visible(unconnected);
            for w in [
                inputs_title.upcast_ref::<gtk::Widget>(),
                inputs.upcast_ref(),
                dependents_title.upcast_ref(),
                dependents.upcast_ref(),
            ] {
                w.set_visible(!unconnected);
            }
            for (flow, list, class) in
                [(&inputs, &ups, "dep-up"), (&dependents, &downs, "dep-down")]
            {
                while let Some(c) = flow.first_child() {
                    flow.remove(&c);
                }
                if list.is_empty() {
                    flow.append(
                        &gtk::Label::builder()
                            .label("—")
                            .xalign(0.0)
                            .css_classes(["dim", "deps-none"])
                            .build(),
                    );
                }
                for dep in list {
                    let Some(n) = e.scene().get(*dep) else {
                        continue;
                    };
                    let title = tree_model::display_name(e.scene(), *dep);
                    flow.append(&chip(&weak, *dep, n, &title, class));
                }
            }
        });
        panel
    }
}

/// Kinds the Style panel applies to: drawables and folders (whose style
/// their children inherit).
fn style_applies(tag: KindTag) -> bool {
    matches!(
        tag,
        KindTag::Plot | KindTag::Point | KindTag::Folder | KindTag::Regression
    )
}

/// Point text `(x, y)` of a feature, in input syntax (ASCII minus).
fn point_text(pos: viscas_plot::Vec2) -> String {
    let c = |v: f64| crate::viewport::format_coord(v).replace('\u{2212}', "-");
    format!("({}, {})", c(pos.x), c(pos.y))
}

/// A clickable node chip: click selects, hover highlights.
fn chip(
    weak: &Weak<Inner>,
    id: NodeId,
    node: &viscas_scene::Node,
    text: &str,
    class: &str,
) -> gtk::Button {
    let content = gtk::Box::builder().spacing(4).build();
    let tag = node.kind.tag();
    content.append(
        &gtk::Label::builder()
            .label(tree_model::kind_glyph(tag))
            .css_classes(["kind-glyph", &format!("kind-{}", tag.as_str())])
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(tree_model::one_line(text, 24))
            .build(),
    );
    let button = gtk::Button::builder()
        .child(&content)
        .css_classes(["node-chip", class])
        .tooltip_text(format!("{} — click to select", tree_model::kind_title(tag)))
        .build();
    let w = weak.clone();
    button.connect_clicked(move |_| {
        Inspector::with_weak(&w, |i| i.inner.ctx.selection().set_selected([id]));
    });
    let motion = gtk::EventControllerMotion::new();
    let w = weak.clone();
    motion.connect_enter(move |_, _, _| {
        Inspector::with_weak(&w, |i| i.inner.ctx.selection().set_hovered(Some(id)));
    });
    let w = weak.clone();
    motion.connect_leave(move |_| {
        Inspector::with_weak(&w, |i| {
            if i.inner.ctx.selection().hovered() == Some(id) {
                i.inner.ctx.selection().set_hovered(None);
            }
        });
    });
    button.add_controller(motion);
    button
}

/// One Analysis value: `min (0, −1)` (click copies `(0, -1)`) and a `+`
/// button adding it to the board as a Point node.
fn analysis_row(weak: &Weak<Inner>, hit: &viscas_engine::FeatureHit) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(4)
        .css_classes(["analysis-row"])
        .build();
    let label = crate::viewport::feature_label(hit);
    let text = point_text(hit.feature.pos);
    let copy = gtk::Button::builder()
        .label(&label)
        .hexpand(true)
        .tooltip_text(format!("Copy {text}"))
        .css_classes(["flat", "mono", "analysis-value"])
        .build();
    if let Some(l) = copy.child().and_downcast::<gtk::Label>() {
        l.set_xalign(0.0);
    }
    let t = text.clone();
    copy.connect_clicked(move |b| b.clipboard().set_text(&t));
    let add = gtk::Button::builder()
        .icon_name(crate::editors::pick_icon(&["list-add-symbolic"]))
        .tooltip_text("Add as point")
        .css_classes(["flat", "area-tool"])
        .build();
    let w = weak.clone();
    add.connect_clicked(move |_| {
        Inspector::with_weak(&w, |i| {
            let mut content = NodeContent::point("P", &text);
            content.name = None;
            let added = i.edit(|e| e.add_node(None, content));
            if let Ok(id) = added {
                i.inner.ctx.selection().set_selected([id]);
            }
        });
    });
    row.append(&copy);
    row.append(&add);
    row
}

/// Evaluated minimum, maximum and step (`None`: continuous) of a slider.
fn slider_bounds(e: &Engine, id: NodeId) -> Result<(f64, f64, Option<f64>), String> {
    let Some(NodeKind::Slider { min, max, step, .. }) = e.scene().get(id).map(|n| &n.kind) else {
        return Err("not a slider".to_owned());
    };
    let lo = eval_number(e, min).map_err(|m| format!("minimum: {m}"))?;
    let hi = eval_number(e, max).map_err(|m| format!("maximum: {m}"))?;
    let st = if step.trim().is_empty() {
        None
    } else {
        Some(eval_number(e, step).map_err(|m| format!("step: {m}"))?)
    };
    Ok((lo, hi, st))
}

/// Evaluates an expression against the scene's current definitions.
fn eval_number(e: &Engine, text: &str) -> Result<f64, String> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use viscas_core::eval::{Env, Resolver, Value, eval_expr};

    struct Defs(HashMap<String, Value>);
    impl Resolver for Defs {
        fn resolve(&self, name: &str) -> Option<Value> {
            self.0.get(name).cloned()
        }
    }
    if text.trim().is_empty() {
        return Err("empty".to_owned());
    }
    let expr = viscas_core::syntax::parse_expr(text).map_err(|errs| {
        errs.first()
            .map_or_else(|| "parse error".to_owned(), |er| er.to_string())
    })?;
    let defs: HashMap<String, Value> = e
        .scene()
        .graph()
        .defined_names()
        .filter_map(|n| e.evaluator().lookup(n).map(|v| (n.to_owned(), v)))
        .collect();
    let mut env = Env::new();
    env.set_resolver(Some(Arc::new(Defs(defs))));
    let opts = e.settings().eval_options(None);
    let v = eval_expr(&expr, &env, &opts).map_err(|er| er.message)?;
    match v.as_number() {
        Some(n) if n.is_real() => Ok(v.to_f64()),
        _ => Err(format!("expected a real number, got a {}", v.type_name())),
    }
}
