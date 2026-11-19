//! Main application window: header bars, sidebar and viewport.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::panels::{self, Panel};
use crate::viewport;

/// Sidebar width limits when resized by dragging its edge, in pixels.
const SIDEBAR_MIN_WIDTH: f64 = 300.0;
const SIDEBAR_MAX_WIDTH: f64 = 640.0;

/// Builds and returns the main window for `app`.
pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let stack = panels::build_stack();
    let split_view = adw::OverlaySplitView::builder()
        .content(&build_content())
        .min_sidebar_width(380.0)
        .max_sidebar_width(440.0)
        .sidebar_width_fraction(0.3)
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
    toolbar.set_content(Some(&viewport::build()));
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
/// overlaid on the sidebar edge and pins min/max width to the dragged value.
fn resizable_sidebar(split_view: &adw::OverlaySplitView, sidebar: &gtk::Widget) -> gtk::Widget {
    let handle = gtk::Box::builder()
        .width_request(6)
        .halign(gtk::Align::End)
        .cursor(&gtk::gdk::Cursor::from_name("col-resize", None).expect("cursor name is valid"))
        .build();
    let overlay = gtk::Overlay::builder().child(sidebar).build();
    overlay.add_overlay(&handle);

    let start_width = Rc::new(Cell::new(0.0));
    let drag = gtk::GestureDrag::new();
    drag.connect_drag_begin(glib::clone!(
        #[strong]
        start_width,
        #[weak]
        overlay,
        move |_, _, _| start_width.set(f64::from(overlay.width()))
    ));
    drag.connect_drag_update(glib::clone!(
        #[weak]
        split_view,
        move |_, dx, _| {
            let rtl = split_view.direction() == gtk::TextDirection::Rtl;
            let dx = if rtl { -dx } else { dx };
            let width = (start_width.get() + dx).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
            split_view.set_min_sidebar_width(width);
            split_view.set_max_sidebar_width(width);
        }
    ));
    handle.add_controller(drag);
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

/// Keyboard accelerators for window actions, as `(detailed action, accels)`.
pub fn accels() -> Vec<(String, Vec<String>)> {
    let mut list = vec![
        ("win.toggle-sidebar".to_owned(), vec!["F9".to_owned()]),
        ("win.undo".to_owned(), vec!["<Control>z".to_owned()]),
        ("win.redo".to_owned(), vec!["<Control><Shift>z".to_owned()]),
    ];
    for (i, panel) in Panel::ALL.iter().enumerate() {
        list.push((
            format!("win.panel::{}", panel.id()),
            vec![format!("<Control>{}", i + 1)],
        ));
    }
    list
}
