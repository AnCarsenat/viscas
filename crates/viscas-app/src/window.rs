//! Main window: top bar, area tree, status bar and command palette.

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::areas::{AreaCommands, Areas};
use crate::commands::WindowCommands;
use crate::ctx::AppCtx;
use crate::layout::{Dir, EditorKind};
use crate::palette::Palette;
use crate::workspace::Workspace;

/// Builds and returns the main window for `app`.
pub fn build(app: &gtk::Application, ctx: &AppCtx) -> gtk::ApplicationWindow {
    let areas = Areas::new(ctx, Workspace::Graph);
    let (statusbar, selection_info, area_info, status_widgets) = build_statusbar();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(areas.widget());
    body.append(&statusbar);
    let overlay = gtk::Overlay::builder().child(&body).build();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Viscas")
        .default_width(1440)
        .default_height(880)
        .width_request(480)
        .height_request(320)
        .titlebar(&build_topbar())
        .child(&overlay)
        .build();

    let palette = Palette::new(&overlay, ctx);
    palette.contain_focus(&window);
    install_space_key(&window);
    install_actions(&window, &areas, &palette);
    crate::doc_ui::install(&window, ctx, &areas, status_widgets);
    ctx.add_command_provider(WindowCommands::new(&window));
    ctx.add_command_provider(AreaCommands::new(&areas));
    ctx.add_command_provider(crate::editors::scene_tree::ops::NodeCommands::new(ctx));

    if let Some(notice) = ctx.preferences().load_notice() {
        let label = gtk::Label::builder()
            .label(&notice)
            .tooltip_text(
                "Details were logged; open Preferences ▸ Save & Load to see the file path",
            )
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["status-notice"])
            .build();
        statusbar.insert_child_after(
            &label,
            statusbar
                .first_child()
                .and_then(|c| c.next_sibling())
                .as_ref(),
        );
    }

    let engine = ctx.engine().clone();
    let update_selection = move |sel: &crate::ctx::Selection| {
        let name = |id: crate::ctx::NodeId| {
            engine
                .with(|e| e.scene().get(id).and_then(|n| n.name.clone()))
                .unwrap_or_else(|| format!("node {id}"))
        };
        let selected = sel.selected();
        let text = match selected.len() {
            0 => "Nothing selected".to_owned(),
            1 => name(selected[0]),
            n => format!("{n} nodes selected"),
        };
        let text = match sel.hovered() {
            Some(id) => format!("{text}  ·  hover {}", name(id)),
            None => text,
        };
        selection_info.set_label(&text);
    };
    update_selection(ctx.selection());
    ctx.selection().connect_changed(update_selection);

    let update_info = move |areas: &Areas| {
        let layout = areas.layout();
        let total = layout.leaves().len();
        let text = match (areas.active_area(), areas.active_editor()) {
            (Some(id), Some(kind)) => {
                let pos = layout
                    .leaves()
                    .iter()
                    .position(|(a, _)| *a == id)
                    .map_or(0, |p| p + 1);
                let max = if layout.maximized().is_some() {
                    "  ·  maximized"
                } else {
                    ""
                };
                format!("{}  ·  area {pos}/{total}{max}", kind.title())
            }
            _ => String::new(),
        };
        area_info.set_label(&text);
    };
    update_info(&areas);
    areas.connect_changed(update_info);

    window
}

fn build_topbar() -> gtk::HeaderBar {
    let header = gtk::HeaderBar::builder()
        .show_title_buttons(true)
        .css_classes(["topbar"])
        .build();

    // App menu: logo + name.
    let logo = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    logo.append(
        &gtk::Label::builder()
            .label("∿")
            .css_classes(["app-logo"])
            .build(),
    );
    logo.append(
        &gtk::Label::builder()
            .label("Viscas")
            .css_classes(["app-name"])
            .build(),
    );
    let app_menu = gtk::MenuButton::builder()
        .child(&logo)
        .menu_model(&app_menu())
        .tooltip_text("Viscas")
        .css_classes(["app-menu", "flat"])
        .build();
    header.pack_start(&app_menu);
    header.pack_start(&gtk::PopoverMenuBar::from_model(Some(&menubar())));

    // Workspace tabs, radio-like through the stateful `win.workspace` action.
    let tabs = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(1)
        .valign(gtk::Align::Center)
        .css_classes(["workspace-tabs"])
        .build();
    for ws in Workspace::ALL {
        let tab = gtk::ToggleButton::builder()
            .label(ws.title())
            .action_name("win.workspace")
            .action_target(&ws.id().to_variant())
            .tooltip_text(format!(
                "{} workspace ({})",
                ws.title(),
                crate::shortcuts::label_for(&format!("win.workspace::{}", ws.id()))
                    .unwrap_or_default()
            ))
            .build();
        tabs.append(&tab);
    }
    header.set_title_widget(Some(&tabs));

    // Command palette button with keycap hint.
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    content.append(&gtk::Image::from_icon_name(crate::editors::pick_icon(&[
        "system-search-symbolic",
        "edit-find-symbolic",
    ])));
    content.append(
        &gtk::Label::builder()
            .label("Search commands…")
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(crate::shortcuts::palette_hint())
            .css_classes(["keycap"])
            .build(),
    );
    let palette_button = gtk::Button::builder()
        .child(&content)
        .action_name("win.palette")
        .tooltip_text("Command Palette")
        .valign(gtk::Align::Center)
        .css_classes(["palette-button"])
        .build();
    header.pack_end(&palette_button);
    header
}

fn app_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let prefs = gio::Menu::new();
    prefs.append(Some("_Preferences…"), Some("app.preferences"));
    let help = gio::Menu::new();
    help.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
    help.append(Some("_About Viscas"), Some("app.about"));
    let quit = gio::Menu::new();
    quit.append(Some("_Quit"), Some("app.quit"));
    menu.append_section(None, &prefs);
    menu.append_section(None, &help);
    menu.append_section(None, &quit);
    menu
}

fn menubar() -> gio::Menu {
    // Items pointing at unregistered actions render insensitive; they are
    // placeholders for the document milestone (M8).
    let file = gio::Menu::new();
    let doc = gio::Menu::new();
    doc.append(Some("_New"), Some("win.new"));
    doc.append(Some("_Open…"), Some("win.open"));
    doc.append(Some("_Save"), Some("win.save"));
    doc.append(Some("Save _As…"), Some("win.save-as"));
    let export = gio::Menu::new();
    export.append(Some("_Export Viewport…"), Some("win.export"));
    let quit = gio::Menu::new();
    quit.append(Some("_Quit"), Some("app.quit"));
    file.append_section(None, &doc);
    file.append_section(None, &export);
    file.append_section(None, &quit);

    let edit = gio::Menu::new();
    let history = gio::Menu::new();
    history.append(Some("_Undo"), Some("win.undo"));
    history.append(Some("_Redo"), Some("win.redo"));
    let tools = gio::Menu::new();
    tools.append(Some("_Command Palette"), Some("win.palette"));
    let prefs = gio::Menu::new();
    prefs.append(Some("_Preferences…"), Some("app.preferences"));
    edit.append_section(None, &history);
    edit.append_section(None, &tools);
    edit.append_section(None, &prefs);

    let view = gio::Menu::new();
    let area = gio::Menu::new();
    area.append(
        Some("Split Area _Horizontally"),
        Some("win.split-area::horizontal"),
    );
    area.append(
        Some("Split Area _Vertically"),
        Some("win.split-area::vertical"),
    );
    area.append(Some("_Close Area"), Some("win.close-area"));
    area.append(Some("_Maximize Area"), Some("win.maximize-area"));
    let editors = gio::Menu::new();
    for kind in EditorKind::ALL {
        editors.append(
            Some(kind.title()),
            Some(&format!("win.editor::{}", kind.id())),
        );
    }
    let workspaces = gio::Menu::new();
    for ws in Workspace::ALL {
        workspaces.append(
            Some(ws.title()),
            Some(&format!("win.workspace::{}", ws.id())),
        );
    }
    let subs = gio::Menu::new();
    subs.append_submenu(Some("_Editor Type"), &editors);
    subs.append_submenu(Some("_Workspace"), &workspaces);
    view.append_section(None, &area);
    view.append_section(None, &subs);

    let help = gio::Menu::new();
    help.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
    help.append(Some("_About Viscas"), Some("app.about"));

    let bar = gio::Menu::new();
    bar.append_submenu(Some("_File"), &file);
    bar.append_submenu(Some("_Edit"), &edit);
    bar.append_submenu(Some("_View"), &view);
    bar.append_submenu(Some("_Help"), &help);
    bar
}

/// Status bar; returns it with the selection and active-area labels and
/// the widgets the document code updates (clickable mode chips, status).
fn build_statusbar() -> (
    gtk::Box,
    gtk::Label,
    gtk::Label,
    crate::doc_ui::StatusWidgets,
) {
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .css_classes(["statusbar"])
        .build();
    let chip = |text: &str, tip: &str, accent: bool| {
        let label = gtk::Label::builder()
            .label(text)
            .tooltip_text(tip)
            .valign(gtk::Align::Center)
            .css_classes(["chip"])
            .build();
        if accent {
            label.add_css_class("accent");
        }
        label
    };
    // Labels and tooltips follow the engine settings (see `doc_ui`).
    let mode_chip = chip("EXACT", "Results stay exact", true);
    let angle_chip = chip("RAD", "Angle unit: radians", false);
    bar.append(&mode_chip);
    bar.append(&angle_chip);
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    let selection_info = gtk::Label::builder().css_classes(["dim"]).build();
    bar.append(&selection_info);
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    let area_info = gtk::Label::builder().css_classes(["faint"]).build();
    bar.append(&area_info);

    let spacer = gtk::Box::builder().hexpand(true).build();
    bar.append(&spacer);

    let status = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(5)
        .build();
    status.append(
        &gtk::Label::builder()
            .label("●")
            .css_classes(["status-dot"])
            .build(),
    );
    let status_label = gtk::Label::builder()
        .label("ready")
        .css_classes(["mono"])
        .build();
    status.append(&status_label);
    bar.append(&status);
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    bar.append(
        &gtk::Label::builder()
            .label(concat!("v", env!("CARGO_PKG_VERSION")))
            .css_classes(["mono", "faint"])
            .build(),
    );
    let widgets = crate::doc_ui::StatusWidgets {
        mode_chip,
        angle_chip,
        status: status_label,
    };
    (bar, selection_info, area_info, widgets)
}

/// Space opens the palette like Blender's search, unless focus is in a
/// text field (where Space types a space). The controller runs in the
/// bubble phase, so a focused button, check box or row that uses Space
/// itself gets it first. Holding Space (key repeat) opens it only once.
fn install_space_key(window: &gtk::ApplicationWindow) {
    let keys = gtk::EventControllerKey::new();
    let held = std::rc::Rc::new(std::cell::Cell::new(false));
    keys.connect_key_released(glib::clone!(
        #[strong]
        held,
        move |_, key, _, _| {
            if key == gtk::gdk::Key::space {
                held.set(false);
            }
        }
    ));
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, mods| {
            if key == gtk::gdk::Key::space && held.replace(true) {
                return glib::Propagation::Stop; // auto-repeat
            }
            let plain = !mods.intersects(
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::ALT_MASK
                    | gtk::gdk::ModifierType::SUPER_MASK
                    | gtk::gdk::ModifierType::SHIFT_MASK,
            );
            if key != gtk::gdk::Key::space || !plain || focus_is_text(&window) {
                return glib::Propagation::Proceed;
            }
            match WidgetExt::activate_action(&window, "win.palette", None) {
                Ok(()) => glib::Propagation::Stop,
                Err(_) => glib::Propagation::Proceed,
            }
        }
    ));
    window.add_controller(keys);
}

/// The focused text widget whose own text undo should take Ctrl+Z
/// (`redo`: Ctrl+Shift+Z) instead of the document: a text view with
/// something to undo/redo, or a text field with uncommitted changes (its
/// entry marked `text-dirty`, or inside a scrub field being edited).
pub fn text_undo_target(window: &gtk::Window, redo: bool) -> Option<gtk::Widget> {
    let focus = gtk::prelude::GtkWindowExt::focus(window)?;
    if let Some(view) = focus.downcast_ref::<gtk::TextView>() {
        let b = view.buffer();
        let can = if redo { b.can_redo() } else { b.can_undo() };
        return can.then_some(focus);
    }
    if !focus.is::<gtk::Text>() && focus.dynamic_cast_ref::<gtk::Editable>().is_none() {
        return None;
    }
    let mut w = Some(focus.clone());
    while let Some(cur) = w {
        if cur.has_css_class("text-dirty")
            || (cur.has_css_class("scrub") && cur.has_css_class("editing"))
        {
            return Some(focus);
        }
        w = cur.parent();
    }
    None
}

/// Whether the focused widget takes text input.
fn focus_is_text(window: &gtk::ApplicationWindow) -> bool {
    let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else {
        return false;
    };
    focus.is::<gtk::Text>()
        || focus.is::<gtk::TextView>()
        || focus.dynamic_cast_ref::<gtk::Editable>().is_some()
}

fn install_actions(window: &gtk::ApplicationWindow, areas: &Areas, palette: &Palette) {
    let palette_action = gio::SimpleAction::new("palette", None);
    palette_action.connect_activate(glib::clone!(
        #[strong]
        palette,
        move |_, _| palette.toggle()
    ));
    window.add_action(&palette_action);

    let split = gio::SimpleAction::new("split-area", Some(glib::VariantTy::STRING));
    split.connect_activate(glib::clone!(
        #[strong]
        areas,
        move |_, param| {
            if let Some(dir) = param.and_then(|p| p.str()).and_then(Dir::from_id) {
                areas.split(None, dir);
            }
        }
    ));
    window.add_action(&split);

    let close = gio::SimpleAction::new("close-area", None);
    close.connect_activate(glib::clone!(
        #[strong]
        areas,
        move |_, _| areas.close(None)
    ));
    window.add_action(&close);

    let maximize = gio::SimpleAction::new("maximize-area", None);
    maximize.connect_activate(glib::clone!(
        #[strong]
        areas,
        move |_, _| areas.toggle_maximize(None)
    ));
    window.add_action(&maximize);

    let editor = gio::SimpleAction::new("editor", Some(glib::VariantTy::STRING));
    editor.connect_activate(glib::clone!(
        #[strong]
        areas,
        move |_, param| {
            if let Some(kind) = param.and_then(|p| p.str()).and_then(EditorKind::from_id) {
                areas.set_editor(None, kind);
            }
        }
    ));
    window.add_action(&editor);

    let workspace = gio::SimpleAction::new_stateful(
        "workspace",
        Some(glib::VariantTy::STRING),
        &areas.workspace().id().to_variant(),
    );
    workspace.connect_change_state(glib::clone!(
        #[strong]
        areas,
        move |action, value| {
            let Some(value) = value else {
                return;
            };
            if let Some(ws) = value.str().and_then(Workspace::from_id) {
                areas.set_workspace(ws);
                action.set_state(value);
            }
        }
    ));
    window.add_action(&workspace);
    // Keep the tabs in sync when the workspace changes from elsewhere
    // (e.g. opening a document restores its workspace).
    let weak_action = workspace.downgrade();
    areas.connect_changed(move |areas| {
        if let Some(action) = weak_action.upgrade() {
            let id = areas.workspace().id().to_variant();
            if action.state().as_ref() != Some(&id) {
                action.set_state(&id);
            }
        }
    });
}
