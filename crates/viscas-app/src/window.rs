//! Main application window: header bars, sidebar and viewport.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::panels::{self, PanelCtx};
use crate::viewport::Viewport;

/// Default sidebar width range (the width follows `SIDEBAR_FRACTION` within it).
const SIDEBAR_DEFAULT_MIN: f64 = 380.0;
const SIDEBAR_DEFAULT_MAX: f64 = 440.0;
const SIDEBAR_FRACTION: f64 = 0.3;
/// Limits for a sidebar width chosen by dragging its edge.
const SIDEBAR_MIN_WIDTH: f64 = 300.0;
const SIDEBAR_MAX_WIDTH: f64 = 640.0;
/// A dragged sidebar never takes more than this fraction of the split view.
const SIDEBAR_MAX_FRACTION: f64 = 0.6;

/// Builds and returns the main window for `app`.
pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let stack = panels::build_stack(&PanelCtx::default());
    let split_view = adw::OverlaySplitView::builder()
        .content(&build_content())
        .min_sidebar_width(SIDEBAR_DEFAULT_MIN)
        .max_sidebar_width(SIDEBAR_DEFAULT_MAX)
        .sidebar_width_fraction(SIDEBAR_FRACTION)
        .build();
    let sidebar = resizable_sidebar(&split_view, &build_sidebar(&stack));
    split_view.set_sidebar(Some(&sidebar));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Viscas")
        .default_width(1200)
        .default_height(800)
        .width_request(360)
        .height_request(294)
        .content(&split_view)
        .build();

    // Collapse the sidebar into an overlay on narrow windows.
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        720.0,
        adw::LengthUnit::Sp,
    ));
    breakpoint.add_setter(&split_view, "collapsed", Some(&true.to_value()));
    breakpoint.add_setter(&split_view, "show-sidebar", Some(&false.to_value()));
    window.add_breakpoint(breakpoint);

    install_actions(&window, &split_view, &stack);
    window
}

fn build_sidebar(stack: &adw::ViewStack) -> gtk::Widget {
    let switcher = adw::ViewSwitcher::builder()
        .stack(stack)
        .policy(adw::ViewSwitcherPolicy::Narrow)
        .build();
    let header = adw::HeaderBar::builder()
        .title_widget(&switcher)
        .show_end_title_buttons(false)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(stack));
    toolbar.upcast()
}

fn build_content() -> gtk::Widget {
    let sidebar_button = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Toggle Sidebar (F9)")
        .action_name("win.toggle-sidebar")
        .build();
    let undo_button = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text("Undo (Ctrl+Z)")
        .action_name("win.undo")
        .build();
    let redo_button = gtk::Button::builder()
        .icon_name("edit-redo-symbolic")
        .tooltip_text("Redo (Ctrl+Shift+Z)")
        .action_name("win.redo")
        .build();
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&primary_menu())
        .primary(true)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Untitled", "Viscas"))
        .build();
    header.pack_start(&sidebar_button);
    header.pack_start(&undo_button);
    header.pack_start(&redo_button);
    header.pack_end(&menu_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(Viewport::new().widget()));
    toolbar.upcast()
}

fn primary_menu() -> gio::Menu {
    let help = gio::Menu::new();
    help.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
    help.append(Some("_About Viscas"), Some("app.about"));
    let quit = gio::Menu::new();
    quit.append(Some("_Quit"), Some("app.quit"));

    let menu = gio::Menu::new();
    menu.append_section(None, &help);
    menu.append_section(None, &quit);
    menu
}

/// Lets the user resize the sidebar by dragging its trailing edge.
///
/// `AdwOverlaySplitView` has no built-in resize handle, so a thin strip is
/// overlaid on the sidebar edge. The drag gesture lives on the stationary
/// split view (so the moving handle cannot feed back into the drag), only
/// claims presses on the handle, and derives the width from the absolute
/// pointer position. The chosen width pins min/max sidebar width while the
/// split view is expanded; collapsing restores the defaults.
fn resizable_sidebar(split_view: &adw::OverlaySplitView, sidebar: &gtk::Widget) -> gtk::Widget {
    let handle = gtk::Box::builder()
        .width_request(6)
        .halign(gtk::Align::End)
        .build();
    handle.set_cursor_from_name(Some("col-resize"));
    let overlay = gtk::Overlay::builder().child(sidebar).build();
    overlay.add_overlay(&handle);

    // Width picked by the user, re-applied when the split view expands.
    let chosen = Rc::new(Cell::new(None::<f64>));

    let drag = gtk::GestureDrag::new();
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    drag.connect_drag_begin(glib::clone!(
        #[weak]
        handle,
        move |gesture, x, y| {
            let Some(split_view) = gesture.widget() else {
                return;
            };
            let on_handle = split_view
                .pick(x, y, gtk::PickFlags::DEFAULT)
                .is_some_and(|w| w == handle || w.is_ancestor(&handle));
            let collapsed = split_view
                .downcast_ref::<adw::OverlaySplitView>()
                .is_some_and(|s| s.is_collapsed());
            if on_handle && !collapsed {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            } else {
                gesture.set_state(gtk::EventSequenceState::Denied);
            }
        }
    ));
    drag.connect_drag_update(glib::clone!(
        #[strong]
        chosen,
        move |gesture, ox, _| {
            let Some(split_view) = gesture.widget().and_downcast::<adw::OverlaySplitView>() else {
                return;
            };
            let Some((sx, _)) = gesture.start_point() else {
                return;
            };
            let x = sx + ox;
            let total = f64::from(split_view.width());
            let width = if split_view.direction() == gtk::TextDirection::Rtl {
                total - x
            } else {
                x
            };
            let max = SIDEBAR_MAX_WIDTH.min(total * SIDEBAR_MAX_FRACTION);
            let width = width.max(SIDEBAR_MIN_WIDTH).min(max);
            chosen.set(Some(width));
            split_view.set_min_sidebar_width(width);
            split_view.set_max_sidebar_width(width);
        }
    ));
    split_view.add_controller(drag);

    split_view.connect_collapsed_notify(move |split_view| {
        if split_view.is_collapsed() {
            split_view.set_min_sidebar_width(SIDEBAR_DEFAULT_MIN);
            split_view.set_max_sidebar_width(SIDEBAR_DEFAULT_MAX);
        } else if let Some(width) = chosen.get() {
            let total = f64::from(split_view.width());
            let max = if total > 0.0 {
                SIDEBAR_MAX_WIDTH.min(total * SIDEBAR_MAX_FRACTION)
            } else {
                SIDEBAR_MAX_WIDTH
            };
            let width = width.max(SIDEBAR_MIN_WIDTH).min(max);
            split_view.set_min_sidebar_width(width);
            split_view.set_max_sidebar_width(width);
        }
    });

    overlay.upcast()
}

fn install_actions(
    window: &adw::ApplicationWindow,
    split_view: &adw::OverlaySplitView,
    stack: &adw::ViewStack,
) {
    window.add_action(&gio::PropertyAction::new(
        "toggle-sidebar",
        split_view,
        "show-sidebar",
    ));
    window.add_action(&gio::PropertyAction::new(
        "panel",
        stack,
        "visible-child-name",
    ));

    // Placeholders until the scene model provides an undo stack.
    for name in ["undo", "redo"] {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(false);
        window.add_action(&action);
    }
}
