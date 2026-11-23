//! 2D graph viewport: a drawing area showing a stack of layers (grid,
//! scene plots) with pan, zoom (scroll, pinch, Shift+drag box), hover
//! highlighting, click-to-select and feature dots.

mod draw;
mod plots;
pub mod view;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};

use self::draw::GridLayer;
use self::plots::{FEATURE_RADIUS_PX, HOVER_PX, PlotState, PlotsLayer, nearest_curve};
use self::view::View;
use crate::ctx::AppCtx;
use crate::doc::DocEvent;

/// Zoom factor applied per discrete scroll-wheel step.
const ZOOM_PER_STEP: f64 = 1.15;
/// Largest scroll delta honored in one event, to keep zoom factors sane.
const MAX_SCROLL_DELTA: f64 = 50.0;
/// Delay coalescing plot requests while the view keeps changing.
const REPLOT_DELAY: Duration = Duration::from_millis(40);

/// Example plots of the header "Add" menu: action target, title, line.
pub const EXAMPLES: &[(&str, &str, &str)] = &[
    ("explicit", "Explicit  y = x²", "y = x^2"),
    (
        "parametric",
        "Parametric  (cos 3t, sin 2t)",
        "(cos(3 t), sin(2 t))",
    ),
    ("polar", "Polar  r = 1 + cos θ", "r = 1 + cos(θ)"),
    (
        "implicit",
        "Implicit  sin x + sin y = 0.3",
        "sin(x) + sin(y) = 0.3",
    ),
    ("inequality", "Inequality  y < sin x", "y < sin(x)"),
];

/// Something drawn in the viewport, in world space described by a [`View`].
///
/// Layers are drawn in insertion order, so the first one is the background.
pub trait Layer {
    /// Draws the layer. `widget` gives access to style colors and Pango.
    fn draw(&self, widget: &gtk::Widget, cr: &cairo::Context, view: &View);
}

type Layers = Rc<RefCell<Vec<Box<dyn Layer>>>>;

/// Header widgets that follow the viewport state.
#[derive(Default)]
struct HeaderRefs {
    errors: glib::WeakRef<gtk::Label>,
}

/// The graph viewport widget and its shared view state.
///
/// Cloning is cheap and yields a handle to the same viewport.
#[derive(Clone)]
pub struct Viewport {
    root: gtk::Overlay,
    area: gtk::DrawingArea,
    hud: gtk::Label,
    coarse: gtk::Label,
    view: Rc<RefCell<View>>,
    layers: Layers,
    plots: Rc<PlotState>,
    actions: gio::SimpleActionGroup,
    header: Rc<RefCell<HeaderRefs>>,
    quick_input: Rc<Cell<bool>>,
}

impl Viewport {
    /// Creates a viewport showing the grid and the scene's plots. It
    /// redraws whenever the preferences or the document change.
    pub fn new(ctx: &AppCtx) -> Self {
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .can_focus(true)
            .css_classes(["viscas-viewport"])
            .build();
        area.update_property(&[gtk::accessible::Property::Label("Graph viewport")]);

        // Cursor readout in the bottom-left corner.
        let hud = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .css_classes(["viewport-hud", "mono"])
            .can_target(false)
            .build();
        // Coarse-sampling indicator in the bottom-right corner.
        let coarse = gtk::Label::builder()
            .label("≈")
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["viewport-hud", "mono"])
            .tooltip_text("Some curves hit the sampling budget and are drawn coarser")
            .visible(false)
            .build();
        let root = gtk::Overlay::builder().child(&area).build();
        root.add_overlay(&hud);
        root.add_overlay(&coarse);

        let viewport = Self {
            root,
            area,
            hud,
            coarse,
            view: Rc::new(RefCell::new(View::default())),
            layers: Rc::default(),
            plots: PlotState::new(ctx),
            actions: gio::SimpleActionGroup::new(),
            header: Rc::default(),
            quick_input: Rc::new(Cell::new(false)),
        };
        viewport.add_layer(GridLayer { ctx: ctx.clone() });
        viewport.add_layer(PlotsLayer {
            state: viewport.plots.clone(),
        });
        let this = viewport.downgrade();
        ctx.preferences()
            .connect_changed(move |p| match this.upgrade() {
                Some(vp) => {
                    if p.viewport.lock_aspect && vp.view.borrow().aspect != 1.0 {
                        vp.update_view(View::lock_aspect);
                    }
                    vp.sync_actions();
                    vp.queue_draw();
                    true
                }
                None => false,
            });
        let this = viewport.downgrade();
        ctx.selection().connect_changed(move |_| {
            if let Some(vp) = this.upgrade() {
                vp.queue_draw();
            }
        });
        viewport.subscribe();
        viewport.install_actions();
        viewport.install_draw();
        viewport.install_drag();
        viewport.install_pointer();
        viewport.install_zoom_gesture();
        viewport.install_keys();
        viewport.plots.refresh();
        viewport.update_hud(None);
        viewport
    }

    /// The widget to put in the widget tree.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    fn ctx(&self) -> &AppCtx {
        &self.plots.ctx
    }

    fn lock_aspect(&self) -> bool {
        self.ctx().preferences().with(|p| p.viewport.lock_aspect)
    }

    /// Recenters on the origin at the default zoom.
    pub fn reset_view(&self) {
        self.update_view(|v| {
            let size = v.size;
            *v = View {
                size,
                ..View::default()
            };
        });
        self.update_hud(None);
    }

    /// Shows the world coordinates under the pointer, or the zoom level when
    /// the pointer is outside.
    fn update_hud(&self, pointer: Option<(f64, f64)>) {
        let text = hud_text(&self.view.borrow(), pointer);
        self.hud.set_label(&text);
    }

    /// A copy of the current view.
    pub fn view(&self) -> View {
        *self.view.borrow()
    }

    /// Modifies the view, schedules a redraw and, when the cached layers no
    /// longer cover it, a plot request.
    pub fn update_view(&self, f: impl FnOnce(&mut View)) {
        f(&mut self.view.borrow_mut());
        self.view_changed();
    }

    fn view_changed(&self) {
        self.queue_draw();
        self.schedule_replot();
    }

    /// Requests plots soon (coalesced while the view keeps changing).
    fn schedule_replot(&self) {
        let view = self.view();
        if self
            .plots
            .requested
            .get()
            .is_some_and(|r| view.covered_by(&r))
        {
            return;
        }
        if self.plots.replot_queued.replace(true) {
            return;
        }
        let this = self.downgrade();
        glib::timeout_add_local_once(REPLOT_DELAY, move || {
            if let Some(vp) = this.upgrade() {
                vp.plots.replot_queued.set(false);
                vp.plots.request(&vp.view(), false);
            }
        });
    }

    /// Appends a layer on top of the existing ones and schedules a redraw.
    pub fn add_layer(&self, layer: impl Layer + 'static) {
        self.layers.borrow_mut().push(Box::new(layer));
        self.queue_draw();
    }

    /// Schedules a redraw, e.g. after a layer's data changed.
    pub fn queue_draw(&self) {
        self.area.queue_draw();
    }

    /// Follows the document: layers, replots, features, errors.
    fn subscribe(&self) {
        let this = self.downgrade();
        let id = self.plots.id;
        let sub = self.ctx().engine().subscribe(move |ev| {
            let Some(vp) = this.upgrade() else {
                return;
            };
            match ev {
                DocEvent::PlotsReady(vps) if vps.contains(&id) => vp.refresh(),
                DocEvent::Scene(_) | DocEvent::Colors => vp.refresh(),
                DocEvent::Replaced => {
                    vp.plots.clear_marks();
                    vp.refresh();
                }
                DocEvent::Replot(vps) if vps.contains(&id) => {
                    vp.plots.request(&vp.view(), true);
                }
                DocEvent::Features(r) if r.viewport == id => {
                    *vp.plots.features.borrow_mut() = r.hits.clone();
                    vp.queue_draw();
                }
                _ => {}
            }
        });
        *self.plots.subscription.borrow_mut() = Some(sub);
    }

    fn refresh(&self) {
        self.plots.refresh();
        self.coarse.set_visible(self.plots.truncated.get() > 0);
        self.update_error_chip();
        self.queue_draw();
    }

    fn update_error_chip(&self) {
        let Some(chip) = self.header.borrow().errors.upgrade() else {
            return;
        };
        let errors = self.plots.errors.borrow();
        chip.set_visible(!errors.is_empty());
        chip.set_label(&format!("⚠ {}", errors.len()));
        chip.set_tooltip_text(Some(&errors.join("\n")));
    }

    /// Widgets for the area header: View and Add menus, the temporary
    /// quick input, the error chip and the reset button.
    pub fn header_widgets(&self) -> Vec<gtk::Widget> {
        let view_menu = gio::Menu::new();
        view_menu.append(Some("Reset View"), Some("vp.reset"));
        view_menu.append(Some("Lock Aspect"), Some("vp.lock-aspect"));
        let add_menu = gio::Menu::new();
        for (target, title, _) in EXAMPLES {
            add_menu.append(Some(title), Some(&format!("vp.add-example::{target}")));
        }
        let menu = |title: &str, model: &gio::Menu| {
            let b = gtk::MenuButton::builder()
                .label(title)
                .menu_model(model)
                .always_show_arrow(false)
                .css_classes(["header-menu", "flat"])
                .build();
            b.insert_action_group("vp", Some(&self.actions));
            b.upcast::<gtk::Widget>()
        };
        let mut widgets = vec![menu("View", &view_menu), menu("Add", &add_menu)];

        // Temporary quick input (to be replaced by Console/Expressions).
        let toggle = gtk::ToggleButton::builder()
            .label("Quick ›")
            .active(self.quick_input.get())
            .tooltip_text(
                "Temporary quick input: type a line such as y = sin(x) and press Enter. \
                 Will be replaced by the Console and Expressions editors.",
            )
            .css_classes(["header-menu", "flat"])
            .build();
        let entry = gtk::Entry::builder()
            .placeholder_text("y = sin(x)   (temporary)")
            .width_chars(22)
            .visible(self.quick_input.get())
            .css_classes(["mono"])
            .build();
        let flag = self.quick_input.clone();
        let weak_entry = entry.downgrade();
        toggle.connect_toggled(move |t| {
            flag.set(t.is_active());
            if let Some(e) = weak_entry.upgrade() {
                e.set_visible(t.is_active());
                if t.is_active() {
                    e.grab_focus();
                }
            }
        });
        let ctx = self.ctx().downgrade();
        entry.connect_activate(move |e| {
            let Some(ctx) = ctx.upgrade() else {
                return;
            };
            let text = e.text().trim().to_owned();
            if text.is_empty() {
                return;
            }
            match ctx.engine().edit(|eng| eng.add_line(None, &text)) {
                Ok(id) => {
                    e.set_text("");
                    e.remove_css_class("error");
                    e.set_tooltip_text(None);
                    ctx.selection().set_selected([id]);
                }
                Err(err) => {
                    e.add_css_class("error");
                    e.set_tooltip_text(Some(&err.to_string()));
                }
            }
        });
        widgets.push(toggle.upcast());
        widgets.push(entry.upcast());

        let errors = gtk::Label::builder()
            .css_classes(["chip", "error"])
            .visible(false)
            .valign(gtk::Align::Center)
            .build();
        widgets.push(errors.clone().upcast());

        let reset = gtk::Button::builder()
            .icon_name(crate::editors::pick_icon(&[
                "zoom-original-symbolic",
                "view-refresh-symbolic",
            ]))
            .tooltip_text("Reset View (Home)")
            .action_name("vp.reset")
            .css_classes(["area-tool"])
            .build();
        reset.insert_action_group("vp", Some(&self.actions));
        widgets.push(reset.upcast());

        *self.header.borrow_mut() = HeaderRefs {
            errors: errors.downgrade(),
        };
        self.update_error_chip();
        widgets
    }

    fn install_actions(&self) {
        let reset = gio::SimpleAction::new("reset", None);
        let this = self.downgrade();
        reset.connect_activate(move |_, _| {
            if let Some(vp) = this.upgrade() {
                vp.reset_view();
            }
        });
        self.actions.add_action(&reset);

        let lock = gio::SimpleAction::new_stateful("lock-aspect", None, &true.to_variant());
        let ctx = self.ctx().downgrade();
        lock.connect_activate(move |action, _| {
            let on = !action.state().and_then(|s| s.get::<bool>()).unwrap_or(true);
            if let Some(ctx) = ctx.upgrade() {
                ctx.preferences().update(|p| p.viewport.lock_aspect = on);
            }
        });
        self.actions.add_action(&lock);

        let add = gio::SimpleAction::new("add-example", Some(glib::VariantTy::STRING));
        let ctx = self.ctx().downgrade();
        add.connect_activate(move |_, param| {
            let (Some(ctx), Some(target)) = (ctx.upgrade(), param.and_then(|p| p.str())) else {
                return;
            };
            let Some((_, _, line)) = EXAMPLES.iter().find(|(t, _, _)| *t == target) else {
                return;
            };
            match ctx.engine().edit(|e| e.add_line(None, line)) {
                Ok(id) => ctx.selection().set_selected([id]),
                Err(err) => glib::g_warning!("viscas", "example {line}: {err}"),
            }
        });
        self.actions.add_action(&add);
        self.sync_actions();
    }

    fn sync_actions(&self) {
        if let Some(a) = self.actions.lookup_action("lock-aspect")
            && let Ok(a) = a.downcast::<gio::SimpleAction>()
        {
            a.set_state(&self.lock_aspect().to_variant());
        }
    }

    fn install_draw(&self) {
        let this = self.downgrade();
        self.area.connect_resize(move |_, w, h| {
            if let Some(vp) = this.upgrade() {
                vp.update_view(|v| v.size = (f64::from(w), f64::from(h)));
            }
        });

        let view = self.view.clone();
        let layers = self.layers.clone();
        self.area.set_draw_func(move |area, cr, w, h| {
            // Use the allocation actually being drawn, even if a resize
            // notification is still pending.
            let mut v = *view.borrow();
            v.size = (f64::from(w), f64::from(h));
            for layer in layers.borrow().iter() {
                layer.draw(area.upcast_ref(), cr, &v);
            }
        });
    }

    fn install_drag(&self) {
        let area = &self.area;
        let drag = gtk::GestureDrag::new();
        let last = Rc::new(Cell::new((0.0, 0.0)));
        let boxing = Rc::new(Cell::new(false));
        let this = self.downgrade();

        drag.connect_drag_begin(glib::clone!(
            #[strong]
            last,
            #[strong]
            boxing,
            #[strong]
            this,
            move |g, x, y| {
                let Some(vp) = this.upgrade() else {
                    return;
                };
                last.set((0.0, 0.0));
                vp.area.grab_focus();
                let shift = g
                    .current_event_state()
                    .contains(gdk::ModifierType::SHIFT_MASK);
                boxing.set(shift);
                if shift {
                    vp.plots.zoom_box.set(Some(((x, y), (x, y))));
                    vp.area.set_cursor_from_name(Some("crosshair"));
                } else {
                    vp.area.set_cursor_from_name(Some("grabbing"));
                }
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[strong]
            last,
            #[strong]
            boxing,
            #[strong]
            this,
            move |g, ox, oy| {
                let Some(vp) = this.upgrade() else {
                    return;
                };
                if boxing.get() {
                    if let Some((sx, sy)) = g.start_point() {
                        vp.plots.zoom_box.set(Some(((sx, sy), (sx + ox, sy + oy))));
                        vp.queue_draw();
                    }
                    return;
                }
                let (lx, ly) = last.replace((ox, oy));
                vp.update_view(|v| v.pan_by_pixels(ox - lx, oy - ly));
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[strong]
            this,
            move |_, _, _| {
                let Some(vp) = this.upgrade() else {
                    return;
                };
                vp.area.set_cursor(None::<&gdk::Cursor>);
                if boxing.replace(false)
                    && let Some((a, b)) = vp.plots.zoom_box.take()
                {
                    let locked = vp.lock_aspect();
                    vp.update_view(|v| {
                        v.fit_screen_rect(a, b, locked);
                    });
                }
                vp.queue_draw();
            }
        ));
        area.add_controller(drag);
    }

    /// Pointer motion (HUD, hover), scroll zoom and clicks.
    fn install_pointer(&self) {
        let area = &self.area;
        // Scroll events carry no position, so track the pointer separately.
        let pointer = Rc::new(Cell::new(None::<(f64, f64)>));
        let motion = gtk::EventControllerMotion::new();
        let this = self.downgrade();
        motion.connect_motion(glib::clone!(
            #[strong]
            pointer,
            #[strong]
            this,
            move |_, x, y| {
                pointer.set(Some((x, y)));
                if let Some(vp) = this.upgrade() {
                    vp.update_hud(Some((x, y)));
                    vp.hover_at(Some((x, y)));
                }
            }
        ));
        motion.connect_leave(glib::clone!(
            #[strong]
            pointer,
            #[strong]
            this,
            move |_| {
                pointer.set(None);
                if let Some(vp) = this.upgrade() {
                    vp.update_hud(None);
                    vp.hover_at(None);
                }
            }
        ));
        area.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(glib::clone!(
            #[strong]
            this,
            #[strong]
            pointer,
            move |_, _, dy| {
                let Some(vp) = this.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                vp.update_view(|v| {
                    let (sx, sy) = pointer.get().unwrap_or((v.size.0 / 2.0, v.size.1 / 2.0));
                    let dy = dy.clamp(-MAX_SCROLL_DELTA, MAX_SCROLL_DELTA);
                    v.zoom_at(ZOOM_PER_STEP.powf(-dy), sx, sy);
                });
                vp.update_hud(pointer.get());
                glib::Propagation::Stop
            }
        ));
        area.add_controller(scroll);

        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(glib::clone!(
            #[strong]
            this,
            move |g, n, x, y| {
                if n != 1
                    || g.current_event_state()
                        .contains(gdk::ModifierType::SHIFT_MASK)
                {
                    return;
                }
                if let Some(vp) = this.upgrade() {
                    vp.click_at(x, y);
                }
            }
        ));
        area.add_controller(click);
    }

    /// Hover: feature dot label, else nearest curve → shared hover model.
    fn hover_at(&self, p: Option<(f64, f64)>) {
        let view = self.view();
        let dot = p.and_then(|(x, y)| self.plots.dot_at(&view, x, y));
        let dot_pos = dot.map(|h| h.feature.pos);
        let dot_changed = self.plots.hover_dot.replace(dot_pos) != dot_pos;
        let node = match (p, dot) {
            (_, Some(h)) => Some(h.node),
            (Some(p), None) => nearest_curve(&self.plots.layers.borrow(), &view, p, HOVER_PX),
            (None, None) => None,
        };
        let sel = self.ctx().selection();
        if sel.hovered() != node {
            sel.set_hovered(node); // redraws through the selection listener
        } else if dot_changed {
            self.queue_draw();
        }
        self.area.set_cursor_from_name(if node.is_some() {
            Some("pointer")
        } else {
            None
        });
    }

    /// Click: pin a dot's label, or select the curve and look for
    /// features near the point, or clear.
    fn click_at(&self, x: f64, y: f64) {
        let view = self.view();
        if let Some(hit) = self.plots.dot_at(&view, x, y) {
            self.plots.toggle_pin(hit);
            self.queue_draw();
            return;
        }
        let node = nearest_curve(&self.plots.layers.borrow(), &view, (x, y), HOVER_PX);
        let sel = self.ctx().selection();
        match node {
            Some(id) => {
                sel.set_selected([id]);
                let (wx, wy) = view.screen_to_world(x, y);
                let radius = FEATURE_RADIUS_PX / view.scale.min(view.scale_y());
                self.ctx().engine().request_features(
                    self.plots.id,
                    viscas_plot::Vec2::new(wx, wy),
                    radius,
                );
            }
            None => {
                sel.set_selected([]);
                self.plots.features.borrow_mut().clear();
                self.queue_draw();
            }
        }
    }

    fn install_zoom_gesture(&self) {
        let zoom = gtk::GestureZoom::new();
        let last = Rc::new(Cell::new(1.0));
        zoom.connect_begin(glib::clone!(
            #[strong]
            last,
            move |_, _| last.set(1.0)
        ));
        let this = self.downgrade();
        zoom.connect_scale_changed(move |g, scale| {
            let Some(vp) = this.upgrade() else {
                return;
            };
            let factor = scale / last.replace(scale);
            let center = g.bounding_box_center();
            vp.update_view(|v| {
                let (sx, sy) = center.unwrap_or((v.size.0 / 2.0, v.size.1 / 2.0));
                v.zoom_at(factor, sx, sy);
            });
        });
        self.area.add_controller(zoom);
    }

    fn install_keys(&self) {
        let keys = gtk::EventControllerKey::new();
        let this = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(vp) = this.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Home | gdk::Key::KP_Home => {
                    vp.reset_view();
                    glib::Propagation::Stop
                }
                gdk::Key::Escape => {
                    vp.plots.clear_marks();
                    vp.plots.zoom_box.set(None);
                    vp.ctx().selection().set_selected([]);
                    vp.queue_draw();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.area.add_controller(keys);
    }

    fn downgrade(&self) -> WeakViewport {
        WeakViewport {
            root: self.root.downgrade(),
            area: self.area.downgrade(),
            hud: self.hud.downgrade(),
            coarse: self.coarse.downgrade(),
            view: Rc::downgrade(&self.view),
            layers: Rc::downgrade(&self.layers),
            plots: Rc::downgrade(&self.plots),
            actions: self.actions.downgrade(),
            header: Rc::downgrade(&self.header),
            quick_input: Rc::downgrade(&self.quick_input),
        }
    }
}

/// Weak handle to a [`Viewport`], for signal closures owned by its widgets.
#[derive(Clone)]
struct WeakViewport {
    root: glib::WeakRef<gtk::Overlay>,
    area: glib::WeakRef<gtk::DrawingArea>,
    hud: glib::WeakRef<gtk::Label>,
    coarse: glib::WeakRef<gtk::Label>,
    view: Weak<RefCell<View>>,
    layers: Weak<RefCell<Vec<Box<dyn Layer>>>>,
    plots: Weak<PlotState>,
    actions: glib::WeakRef<gio::SimpleActionGroup>,
    header: Weak<RefCell<HeaderRefs>>,
    quick_input: Weak<Cell<bool>>,
}

impl WeakViewport {
    fn upgrade(&self) -> Option<Viewport> {
        Some(Viewport {
            root: self.root.upgrade()?,
            area: self.area.upgrade()?,
            hud: self.hud.upgrade()?,
            coarse: self.coarse.upgrade()?,
            view: self.view.upgrade()?,
            layers: self.layers.upgrade()?,
            plots: self.plots.upgrade()?,
            actions: self.actions.upgrade()?,
            header: self.header.upgrade()?,
            quick_input: self.quick_input.upgrade()?,
        })
    }
}

/// Text of the cursor readout: world coordinates under the pointer, or the
/// zoom level when there is no pointer.
fn hud_text(view: &View, pointer: Option<(f64, f64)>) -> String {
    match pointer {
        Some((sx, sy)) => {
            let (x, y) = view.screen_to_world(sx, sy);
            let dx = view::label_digits(view.scale);
            let dy = view::label_digits(view.scale_y());
            format!("x {x:>+10.dx$}   y {y:>+10.dy$}")
        }
        None => format!("{:.3} px/unit", view.scale),
    }
}
