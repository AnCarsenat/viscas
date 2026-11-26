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
    let (statusbar, selection_info, status_widgets) = build_statusbar();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(areas.widget());
    body.append(&statusbar);
    let overlay = gtk::Overlay::builder().child(&body).build();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Viscas")
        .icon_name(crate::app::APP_ID)
        .default_width(1440)
        .default_height(880)
        .width_request(480)
        .height_request(320)
        .child(&overlay)
        .build();
    window.set_titlebar(Some(&build_topbar(ctx, &areas)));

    let palette = Palette::new(&overlay, ctx);
    palette.contain_focus(&window);
    crate::keymap::live::install(&window, ctx);
    crate::compose::install(&window);
    install_actions(&window, &areas, &palette);
    crate::doc_ui::install(&window, ctx, &areas, status_widgets);
    ctx.add_command_provider(WindowCommands::new(&window, ctx));
    ctx.add_command_provider(AreaCommands::new(&areas));
    ctx.add_command_provider(crate::editors::scene_tree::ops::NodeCommands::new(ctx));
    crate::palette_sources::install(&window, ctx);

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

    // Selection name · eval status · dirty marker (follows renames).
    crate::status::connect(ctx, &selection_info);
    let tip = selection_info.downgrade();
    areas.connect_changed(move |areas| {
        if let Some(label) = tip.upgrade() {
            let text = areas
                .active_editor()
                .map(|k| format!("Active area: {}", k.title()));
            label.set_tooltip_text(text.as_deref());
        }
    });

    window
}

fn build_topbar(ctx: &AppCtx, areas: &Areas) -> gtk::HeaderBar {
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
    let app_model = app_menu();
    let app_menu = gtk::MenuButton::builder()
        .child(&logo)
        .menu_model(&app_model)
        .tooltip_text("Viscas")
        .css_classes(["app-menu", "flat"])
        .build();
    header.pack_start(&app_menu);
    // Menu items show the current key bindings, updated on rebinding.
    crate::keymap::live::refresh_menu_accels(&app_model);
    crate::keymap::live::connect_changed_for(&app_menu, move |_| {
        crate::keymap::live::refresh_menu_accels(&app_model);
    });
    let show = ctx.preferences().with(|p| p.interface.show_unfinished);
    let (model, section) = menubar(show);
    let bar = gtk::PopoverMenuBar::from_model(Some(&model));
    // The View ▸ area section of the current menu model.
    let area_section = std::rc::Rc::new(std::cell::RefCell::new(section));
    // Area items name the area they act on: "Close Area (Viewport)".
    let last_label = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    // "Show unfinished editors" changes the Editor Type / Workspace menus.
    let last = std::cell::Cell::new(show);
    let (sec, lbl) = (area_section.clone(), last_label.clone());
    ctx.preferences()
        .connect_changed_for(bar.downgrade(), move |bar, p| {
            let show = p.interface.show_unfinished;
            if last.replace(show) != show {
                let (model, section) = menubar(show);
                let name = lbl.borrow().clone().unwrap_or_else(|| "Area".to_owned());
                fill_area_section(&section, &name);
                sec.replace(section);
                crate::keymap::live::refresh_menu_accels(&model);
                bar.set_menu_model(Some(&model));
            }
        });
    crate::keymap::live::refresh_menu_accels(&model);
    crate::keymap::live::connect_changed_for(&bar, |bar| {
        if let Some(model) = bar.menu_model().and_downcast::<gio::Menu>() {
            crate::keymap::live::refresh_menu_accels(&model);
        }
    });
    header.pack_start(&bar);
    let update_area_section = move |areas: &Areas| {
        let name = areas.target_editor().map(|k| k.title()).unwrap_or("Area");
        if last_label.borrow().as_deref() == Some(name) {
            return;
        }
        last_label.replace(Some(name.to_owned()));
        fill_area_section(&area_section.borrow(), name);
    };
    update_area_section(areas);
    areas.connect_changed(update_area_section);

    header.set_title_widget(Some(&workspace_tabs(ctx, areas)));

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
    let hint = gtk::Label::builder().css_classes(["keycap"]).build();
    let sync_hint = |hint: &gtk::Label| {
        let keys = crate::keymap::live::label("win.palette");
        hint.set_visible(keys.is_some());
        hint.set_label(&keys.unwrap_or_default());
    };
    sync_hint(&hint);
    crate::keymap::live::connect_changed_for(&hint, sync_hint);
    content.append(&hint);
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

/// Workspace tabs. Their checked state is driven by the area system
/// rather than by a stateful action (which left the old tab highlighted
/// after a round trip, audit B5). Right-click resets a workspace.
fn workspace_tabs(ctx: &AppCtx, areas: &Areas) -> gtk::Box {
    let tabs = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(1)
        .valign(gtk::Align::Center)
        .css_classes(["workspace-tabs"])
        .build();
    let mut buttons = Vec::new();
    for ws in Workspace::ALL {
        let tab = gtk::ToggleButton::builder().label(ws.title()).build();
        let sync_tip = move |tab: &gtk::ToggleButton| {
            let keys = crate::keymap::live::label(&format!("win.workspace::{}", ws.id()))
                .map(|k| format!(" ({k})"))
                .unwrap_or_default();
            tab.set_tooltip_text(Some(&format!(
                "{} workspace{keys} · right-click to reset its layout",
                ws.title()
            )));
        };
        sync_tip(&tab);
        crate::keymap::live::connect_changed_for(&tab, sync_tip);
        tab.connect_clicked(move |tab| {
            // Undo GTK's own toggle; the listener below sets the state.
            tab.set_active(!tab.is_active());
            let _ = WidgetExt::activate_action(tab, "win.workspace", Some(&ws.id().to_variant()));
        });

        let menu = gio::Menu::new();
        menu.append(Some("Reset Workspace Layout"), Some("tab.reset"));
        menu.append(
            Some("Reset All Workspaces"),
            Some("win.reset-all-workspaces"),
        );
        let group = gio::SimpleActionGroup::new();
        let reset = gio::SimpleAction::new("reset", None);
        // Goes through a window action: capturing `Areas` here would keep
        // it alive through its own listener.
        reset.connect_activate(glib::clone!(
            #[weak]
            tab,
            move |_, _| {
                let _ = WidgetExt::activate_action(
                    &tab,
                    "win.reset-workspace-id",
                    Some(&ws.id().to_variant()),
                );
            }
        ));
        group.add_action(&reset);
        tab.insert_action_group("tab", Some(&group));
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.set_parent(&tab);
        tab.connect_destroy(glib::clone!(
            #[strong]
            popover,
            move |_| popover.unparent()
        ));
        let right = gtk::GestureClick::builder().button(3).build();
        right.connect_pressed(move |_, _, _, _| popover.popup());
        tab.add_controller(right);

        tabs.append(&tab);
        buttons.push((ws, tab));
    }
    // Visible unless hidden through `Areas::set_workspace_visible` or as an
    // unfinished workspace ("Show unfinished editors" off); the current
    // one always shows. The area system re-notifies on preference changes.
    let ctx = ctx.clone();
    let sync = move |areas: &Areas| {
        let current = areas.workspace();
        let p = ctx.preferences().get();
        for (ws, tab) in &buttons {
            if tab.is_active() != (*ws == current) {
                tab.set_active(*ws == current);
            }
            let shown = areas.workspace_visible(*ws) && crate::editors::workspace_shown(*ws, &p);
            tab.set_visible(shown || *ws == current);
        }
    };
    sync(areas);
    areas.connect_changed(sync);
    tabs
}

/// Fills the View menu's area section, naming the target area.
fn fill_area_section(section: &gio::Menu, name: &str) {
    section.remove_all();
    section.append(
        Some(&format!("Split Area _Horizontally ({name})")),
        Some("win.split-area::horizontal"),
    );
    section.append(
        Some(&format!("Split Area _Vertically ({name})")),
        Some("win.split-area::vertical"),
    );
    section.append(
        Some(&format!("_Maximize Area ({name})")),
        Some("win.maximize-area"),
    );
    section.append(
        Some(&format!("Open Area in New _Window ({name})")),
        Some("win.float-area"),
    );
    section.append(
        Some(&format!("_Close Area ({name})")),
        Some("win.close-area"),
    );
    crate::keymap::live::refresh_menu_accels(section);
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

/// The menu bar and its View ▸ area section (relabelled as the target
/// area changes).
fn menubar(show_unfinished: bool) -> (gio::Menu, gio::Menu) {
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
    fill_area_section(&area, "Area");
    let layout_history = gio::Menu::new();
    layout_history.append(Some("_Undo Layout Change"), Some("win.layout-undo"));
    layout_history.append(Some("Redo Layout Change"), Some("win.layout-redo"));
    let editors = gio::Menu::new();
    for kind in EditorKind::ALL
        .into_iter()
        .filter(|k| show_unfinished || !crate::editors::is_unfinished(*k))
    {
        editors.append(
            Some(kind.title()),
            Some(&format!("win.editor::{}", kind.id())),
        );
    }
    let workspaces = gio::Menu::new();
    let list = gio::Menu::new();
    for ws in Workspace::ALL
        .into_iter()
        .filter(|w| show_unfinished || !crate::editors::is_unfinished_workspace(*w))
    {
        list.append(
            Some(ws.title()),
            Some(&format!("win.workspace::{}", ws.id())),
        );
    }
    let reset = gio::Menu::new();
    reset.append(Some("_Reset Workspace Layout"), Some("win.reset-workspace"));
    reset.append(
        Some("Reset _All Workspaces"),
        Some("win.reset-all-workspaces"),
    );
    workspaces.append_section(None, &list);
    workspaces.append_section(None, &reset);
    let subs = gio::Menu::new();
    subs.append_submenu(Some("_Editor Type"), &editors);
    subs.append_submenu(Some("_Workspace"), &workspaces);
    view.append_section(None, &area);
    view.append_section(None, &layout_history);
    view.append_section(None, &subs);

    let help = gio::Menu::new();
    help.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
    help.append(Some("_About Viscas"), Some("app.about"));

    let bar = gio::Menu::new();
    bar.append_submenu(Some("_File"), &file);
    bar.append_submenu(Some("_Edit"), &edit);
    bar.append_submenu(Some("_View"), &view);
    bar.append_submenu(Some("_Help"), &help);
    (bar, area)
}

/// Status bar; returns it with the selection label and the widgets the
/// document code updates (clickable mode chips, status).
fn build_statusbar() -> (gtk::Box, gtk::Label, crate::doc_ui::StatusWidgets) {
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
    (bar, selection_info, widgets)
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

    let simple = |name: &str, f: fn(&Areas)| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(glib::clone!(
            #[strong]
            areas,
            move |_, _| f(&areas)
        ));
        window.add_action(&action);
    };
    simple("float-area", |a| a.tear_off(None));
    simple("layout-undo", Areas::undo_layout);
    simple("layout-redo", Areas::redo_layout);
    simple("next-tab", |a| a.cycle_tab(1));
    simple("prev-tab", |a| a.cycle_tab(-1));
    // Enabled only with something to undo/redo.
    let sync_history = glib::clone!(
        #[weak]
        window,
        move |areas: &Areas| {
            for (name, enabled) in [
                ("layout-undo", areas.can_undo_layout()),
                ("layout-redo", areas.can_redo_layout()),
            ] {
                if let Some(a) = window
                    .lookup_action(name)
                    .and_downcast::<gio::SimpleAction>()
                {
                    a.set_enabled(enabled);
                }
            }
        }
    );
    sync_history(areas);
    areas.connect_changed(sync_history);
    simple("reset-workspace", |a| a.reset_workspace(None));
    simple("reset-all-workspaces", Areas::reset_all_workspaces);
    let reset_id = gio::SimpleAction::new("reset-workspace-id", Some(glib::VariantTy::STRING));
    reset_id.connect_activate(glib::clone!(
        #[strong]
        areas,
        move |_, param| {
            if let Some(ws) = param.and_then(|p| p.str()).and_then(Workspace::from_id) {
                areas.reset_workspace(Some(ws));
            }
        }
    ));
    window.add_action(&reset_id);

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
