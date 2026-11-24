//! The Preferences window, laid out like Blender's: a category list on the
//! left, compact property panels on the right, and a hamburger menu at the
//! bottom left for saving, reverting and factory settings.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::ctx::AppCtx;
use crate::prefs::{self, DefinitionOp, GridStyle, Preferences, Range};
use crate::settings;
use crate::shortcuts;
use crate::theme::model::{Accent, Color, SECTIONS, Theme};
use crate::widgets::{ColorSwatch, Panel, ScrubField, check_row, property_row};

/// Categories in sidebar order: id and title.
const CATEGORIES: [(&str, &str); 7] = [
    ("interface", "Interface"),
    ("themes", "Themes"),
    ("viewport", "Viewport"),
    ("editing", "Editing"),
    ("keymap", "Keymap"),
    ("save-load", "Save & Load"),
    ("system", "System"),
];

thread_local! {
    static OPEN: RefCell<glib::WeakRef<gtk::Window>> = RefCell::default();
}

/// Shows the preferences window, creating it on first use. `page` selects
/// a category by id.
pub fn present(app: &gtk::Application, ctx: &AppCtx, page: Option<&str>) {
    let window = OPEN.with_borrow(|w| w.upgrade()).unwrap_or_else(|| {
        let w = build(app, ctx);
        OPEN.with_borrow_mut(|slot| slot.set(Some(&w)));
        w
    });
    if let Some(page) = page {
        select_page(&window, page);
    }
    window.present();
}

/// The open preferences window, if any.
#[allow(dead_code, reason = "used by the screenshot tooling and tests")]
pub fn window() -> Option<gtk::Window> {
    OPEN.with_borrow(|w| w.upgrade())
}

/// Switches the open window to category `page`.
pub fn select_page(window: &gtk::Window, page: &str) {
    let Some(list) = find_named::<gtk::ListBox>(window.upcast_ref(), "categories") else {
        return;
    };
    if let Some(index) = CATEGORIES.iter().position(|(id, _)| *id == page)
        && let Some(row) = list.row_at_index(index as i32)
    {
        list.select_row(Some(&row));
    }
}

fn find_named<W: IsA<gtk::Widget>>(root: &gtk::Widget, name: &str) -> Option<W> {
    if root.widget_name() == name {
        return root.clone().downcast().ok();
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_named(&c, name) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

type Refresher = Box<dyn Fn(&Preferences)>;

/// Creates widgets bound to preferences. Each widget writes its value
/// through [`AppCtx::preferences`] and is refreshed after every change, so
/// revert/factory/theme presets show up immediately.
///
/// While refreshing, widget signals fire as values are set; `syncing`
/// makes [`Binder::update`] ignore them so a half-updated widget (e.g. a
/// drop-down whose model is being replaced) never writes back.
#[derive(Clone)]
struct Binder {
    ctx: AppCtx,
    refreshers: Rc<RefCell<Vec<Refresher>>>,
    syncing: Rc<Cell<bool>>,
}

impl Binder {
    fn on_refresh(&self, f: impl Fn(&Preferences) + 'static) {
        let prefs = self.ctx.preferences().get();
        self.sync(|| f(&prefs));
        self.refreshers.borrow_mut().push(Box::new(f));
    }

    /// Runs every refresher with `prefs`, ignoring the resulting signals.
    fn refresh_all(&self, prefs: &Preferences) {
        self.sync(|| {
            for f in self.refreshers.borrow().iter() {
                f(prefs);
            }
        });
    }

    fn sync(&self, f: impl FnOnce()) {
        let was = self.syncing.replace(true);
        f();
        self.syncing.set(was);
    }

    fn update(&self, f: impl FnOnce(&mut Preferences)) {
        if !self.syncing.get() {
            self.ctx.preferences().update(f);
        }
    }

    /// A manual theme change (turns "follow system" off).
    fn update_theme(&self, f: impl FnOnce(&mut Preferences)) {
        if !self.syncing.get() {
            self.ctx.preferences().update_theme(f);
        }
    }

    /// Drop-down over the installed theme names, bound to a name field.
    fn theme_choice(
        &self,
        get: fn(&Preferences) -> String,
        set: fn(&mut Preferences, String),
    ) -> gtk::DropDown {
        let names = gtk::StringList::new(&[]);
        let dropdown = gtk::DropDown::builder().model(&names).build();
        let b = self.clone();
        dropdown.connect_selected_notify(move |d| {
            if let Some(name) = d
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .map(|s| s.string().to_string())
            {
                b.update(|p| set(p, name));
            }
        });
        let ctx = self.ctx.clone();
        let d = dropdown.clone();
        self.on_refresh(move |p| {
            let wanted = sync_theme_names(&names, &ctx);
            let index = wanted
                .iter()
                .position(|n| *n == get(p))
                .map_or(gtk::INVALID_LIST_POSITION, |i| i as u32);
            if d.selected() != index {
                d.set_selected(index);
            }
        });
        dropdown
    }

    fn scrub(
        &self,
        range: Range,
        get: fn(&Preferences) -> f64,
        set: fn(&mut Preferences, f64),
    ) -> ScrubField {
        let field = ScrubField::new(range.into(), self.ctx.preferences().with(get));
        let b = self.clone();
        field.connect_changed(move |v| b.update(|p| set(p, v)));
        let f = field.clone();
        self.on_refresh(move |p| f.set_value(get(p)));
        field
    }

    fn check(
        &self,
        label: &str,
        get: fn(&Preferences) -> bool,
        set: fn(&mut Preferences, bool),
    ) -> gtk::Box {
        let (row, check) = check_row(label, self.ctx.preferences().with(get));
        let b = self.clone();
        check.connect_toggled(move |c| {
            let v = c.is_active();
            b.update(|p| set(p, v));
        });
        self.on_refresh(move |p| {
            if check.is_active() != get(p) {
                check.set_active(get(p));
            }
        });
        row
    }

    fn choice(
        &self,
        items: &[&str],
        get: fn(&Preferences) -> u32,
        set: fn(&mut Preferences, u32),
    ) -> gtk::DropDown {
        let dropdown = gtk::DropDown::from_strings(items);
        dropdown.set_selected(self.ctx.preferences().with(get));
        let b = self.clone();
        dropdown.connect_selected_notify(move |d| {
            let i = d.selected();
            if i != gtk::INVALID_LIST_POSITION {
                b.update(|p| set(p, i));
            }
        });
        let d = dropdown.clone();
        self.on_refresh(move |p| {
            if d.selected() != get(p) {
                d.set_selected(get(p));
            }
        });
        dropdown
    }
}

/// Makes `names` list the installed themes; returns the names.
fn sync_theme_names(names: &gtk::StringList, ctx: &AppCtx) -> Vec<String> {
    let wanted: Vec<String> = ctx
        .preferences()
        .themes()
        .into_iter()
        .map(|t| t.name)
        .collect();
    let current: Vec<String> = (0..names.n_items())
        .filter_map(|i| names.string(i).map(|s| s.to_string()))
        .collect();
    if wanted != current {
        let refs: Vec<&str> = wanted.iter().map(String::as_str).collect();
        names.splice(0, names.n_items(), &refs);
    }
    wanted
}

fn build(app: &gtk::Application, ctx: &AppCtx) -> gtk::Window {
    let parent = app.active_window();
    let binder = Binder {
        ctx: ctx.clone(),
        refreshers: Rc::default(),
        syncing: Rc::default(),
    };

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::None)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_named(&interface_page(&binder), Some("interface"));
    stack.add_named(&themes_page(&binder), Some("themes"));
    stack.add_named(&viewport_page(&binder), Some("viewport"));
    stack.add_named(&editing_page(&binder), Some("editing"));
    stack.add_named(&keymap_page(), Some("keymap"));
    stack.add_named(&save_load_page(&binder), Some("save-load"));
    stack.add_named(&system_page(&binder), Some("system"));

    let categories = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Browse)
        .name("categories")
        .css_classes(["prefs-categories"])
        .build();
    for (_, title) in CATEGORIES {
        categories.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["prefs-category"])
                .build(),
        );
    }
    categories.connect_row_selected(glib::clone!(
        #[weak]
        stack,
        move |_, row| {
            if let Some((id, _)) = row.and_then(|r| CATEGORIES.get(r.index() as usize)) {
                stack.set_visible_child_name(id);
            }
        }
    ));
    categories.select_row(categories.row_at_index(0).as_ref());

    let sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .width_request(150)
        .css_classes(["prefs-sidebar"])
        .build();
    sidebar.append(&categories);
    sidebar.append(&gtk::Box::builder().vexpand(true).build());
    sidebar.append(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&prefs_menu())
            .direction(gtk::ArrowType::Up)
            .halign(gtk::Align::Start)
            .tooltip_text("Preferences Menu")
            .css_classes(["prefs-menu", "flat"])
            .build(),
    );

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&stack)
        .css_classes(["prefs-content"])
        .build();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["prefs-body"])
        .build();
    body.append(&sidebar);
    body.append(&scrolled);

    let header = gtk::HeaderBar::builder()
        .css_classes(["topbar"])
        .title_widget(
            &gtk::Label::builder()
                .label("Viscas Preferences")
                .css_classes(["app-name"])
                .build(),
        )
        .build();

    let window = gtk::Window::builder()
        .application(app)
        .title("Viscas Preferences")
        .default_width(760)
        .default_height(600)
        .titlebar(&header)
        .child(&body)
        .css_classes(["prefs-window"])
        .build();
    window.set_transient_for(parent.as_ref());
    install_menu_actions(&window, &binder);

    // Refresh bound widgets after every change; the refreshers live as
    // long as the window (owned by the destroy handler's closure).
    let refreshers = binder.refreshers.clone();
    let weak = Rc::downgrade(&refreshers);
    let syncing = binder.syncing.clone();
    let listener_ctx = ctx.downgrade();
    ctx.preferences()
        .connect_changed(move |p| match (weak.upgrade(), listener_ctx.upgrade()) {
            (Some(refreshers), Some(ctx)) => {
                let b = Binder {
                    ctx,
                    refreshers,
                    syncing: syncing.clone(),
                };
                b.refresh_all(p);
                true
            }
            _ => false,
        });
    window.connect_destroy(move |_| drop(refreshers.take()));
    window
}

fn prefs_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("Save Preferences"), Some("prefs.save"));
    file.append(Some("Revert to Saved Preferences"), Some("prefs.revert"));
    file.append(Some("Load Factory Settings"), Some("prefs.factory"));
    let auto = gio::Menu::new();
    auto.append(Some("Auto-Save Preferences"), Some("prefs.auto-save"));
    menu.append_section(None, &file);
    menu.append_section(None, &auto);
    menu
}

fn install_menu_actions(window: &gtk::Window, binder: &Binder) {
    let group = gio::SimpleActionGroup::new();
    let ctx = binder.ctx.clone();

    let save = gio::SimpleAction::new("save", None);
    save.connect_activate(glib::clone!(
        #[strong]
        ctx,
        #[weak]
        window,
        move |_, _| {
            if let Err(e) = settings::save_now(&ctx) {
                alert(&window, "Could not save preferences", &e);
            }
        }
    ));
    let revert = gio::SimpleAction::new("revert", None);
    revert.connect_activate(glib::clone!(
        #[strong]
        ctx,
        move |_, _| settings::revert(&ctx)
    ));
    let factory = gio::SimpleAction::new("factory", None);
    factory.connect_activate(glib::clone!(
        #[strong]
        ctx,
        move |_, _| ctx.preferences().replace(Preferences::default())
    ));
    let auto = gio::SimpleAction::new_stateful(
        "auto-save",
        None,
        &ctx.preferences().with(|p| p.auto_save).to_variant(),
    );
    auto.connect_activate(glib::clone!(
        #[strong]
        ctx,
        move |_, _| ctx.preferences().update(|p| p.auto_save = !p.auto_save)
    ));
    let a = auto.clone();
    binder.on_refresh(move |p| a.set_state(&p.auto_save.to_variant()));

    group.add_action(&save);
    group.add_action(&revert);
    group.add_action(&factory);
    group.add_action(&auto);
    window.insert_action_group("prefs", Some(&group));
}

fn alert(window: &gtk::Window, message: &str, detail: &str) {
    gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .modal(true)
        .build()
        .show(Some(window));
}

fn page() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["prefs-page"])
        .build()
}

fn interface_page(b: &Binder) -> gtk::Box {
    let page = page();

    let display = Panel::new("Display", true);
    let scale = b.scrub(
        prefs::UI_SCALE,
        |p| p.interface.ui_scale,
        |p, v| p.interface.ui_scale = v,
    );
    scale.set_tooltip("Size of every interface element (drag, click to type)");
    display.append(&property_row("Resolution Scale", scale.widget()));
    let font = b.scrub(
        prefs::FONT_SIZE,
        |p| p.interface.font_size,
        |p, v| p.interface.font_size = v,
    );
    font.set_suffix(" px");
    display.append(&property_row("Font Size", font.widget()));
    display.append(&property_row("Monospace Font", &mono_font_dropdown(b)));
    page.append(display.widget());

    let editors = Panel::new("Editors", true);
    editors.append(&b.check(
        "Tooltips",
        |p| p.interface.tooltips,
        |p, v| p.interface.tooltips = v,
    ));
    let playful = b.check(
        "Playful Microcopy",
        |p| p.interface.playful,
        |p, v| p.interface.playful = v,
    );
    playful.set_tooltip_text(Some("Show the light-hearted one-liners in empty editors"));
    editors.append(&playful);
    page.append(editors.widget());
    page
}

/// Dropdown of the installed monospace families ("Default" first).
fn mono_font_dropdown(b: &Binder) -> gtk::DropDown {
    let mut families: Vec<String> = gtk::Label::new(None)
        .pango_context()
        .font_map()
        .map(|map| {
            map.list_families()
                .iter()
                .filter(|f| f.is_monospace())
                .map(|f| f.name().to_string())
                .collect()
        })
        .unwrap_or_default();
    families.sort_by_key(|f| f.to_lowercase());
    families.dedup();
    let current = b.ctx.preferences().with(|p| p.interface.mono_font.clone());
    if !current.is_empty() && !families.contains(&current) {
        families.push(current);
    }
    let mut items = vec!["Default".to_owned()];
    items.extend(families);
    let model = gtk::StringList::new(&items.iter().map(String::as_str).collect::<Vec<_>>());
    let dropdown = gtk::DropDown::builder()
        .model(&model)
        .enable_search(true)
        .build();
    let index_of = move |name: &str| -> u32 {
        if name.is_empty() {
            return 0;
        }
        items.iter().position(|i| i == name).unwrap_or(0) as u32
    };
    let binder = b.clone();
    dropdown.connect_selected_notify(move |d| {
        let name = match d.selected() {
            0 | gtk::INVALID_LIST_POSITION => String::new(),
            _ => d
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .map(|s| s.string().to_string())
                .unwrap_or_default(),
        };
        binder.update(|p| p.interface.mono_font = name);
    });
    let d = dropdown.clone();
    b.on_refresh(move |p| {
        let i = index_of(&p.interface.mono_font);
        if d.selected() != i {
            d.set_selected(i);
        }
    });
    dropdown
}

fn themes_page(b: &Binder) -> gtk::Box {
    let page = page();

    // Preset, accent and file operations, like Blender's theme header.
    let presets = Panel::new("Presets", true);
    let names = gtk::StringList::new(&[]);
    let preset = gtk::DropDown::builder().model(&names).build();
    let binder = b.clone();
    preset.connect_selected_notify(move |d| {
        let ctx = &binder.ctx;
        let Some(name) = d
            .selected_item()
            .and_downcast::<gtk::StringObject>()
            .map(|s| s.string().to_string())
        else {
            return;
        };
        if ctx.preferences().with(|p| p.themes.preset == name) {
            return;
        }
        if let Some(theme) = ctx
            .preferences()
            .themes()
            .into_iter()
            .find(|t| t.name == name)
        {
            binder.update_theme(|p| {
                p.theme = theme;
                p.themes.preset = name;
            });
        }
    });
    let modified = gtk::Label::builder()
        .label("modified")
        .css_classes(["prefs-badge"])
        .tooltip_text("The active theme differs from its preset (saved with the preferences)")
        .build();
    let preset_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .build();
    preset.set_hexpand(true);
    preset_box.append(&preset);
    preset_box.append(&modified);
    {
        let ctx = b.ctx.clone();
        let (preset, names, modified) = (preset.clone(), names.clone(), modified.clone());
        b.on_refresh(move |p| {
            let themes = ctx.preferences().themes();
            let wanted = sync_theme_names(&names, &ctx);
            let index = wanted.iter().position(|n| *n == p.themes.preset);
            let index = index.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32);
            if preset.selected() != index {
                preset.set_selected(index);
            }
            let base = themes.iter().find(|t| t.name == p.themes.preset);
            modified.set_visible(base.is_none_or(|t| *t != p.theme));
        });
    }
    presets.append(&property_row("Theme", &preset_box));

    let accents: Vec<&str> = Accent::ALL.iter().map(|a| a.title()).collect();
    let accent = b.choice(
        &accents,
        |p| {
            Accent::ALL
                .iter()
                .position(|a| *a == p.themes.accent)
                .unwrap_or(0) as u32
        },
        |p, i| p.themes.accent = Accent::ALL.get(i as usize).copied().unwrap_or_default(),
    );
    presets.append(&property_row("Accent", &accent));

    let files = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .homogeneous(true)
        .build();
    let import = gtk::Button::with_label("Import…");
    import.set_tooltip_text(Some("Install a theme from a .toml file"));
    let export = gtk::Button::with_label("Export…");
    export.set_tooltip_text(Some("Save the active theme as a .toml file"));
    files.append(&import);
    files.append(&export);
    let ctx = b.ctx.clone();
    import.connect_clicked(move |button| import_theme(button, &ctx));
    let ctx = b.ctx.clone();
    export.connect_clicked(move |button| export_theme(button, &ctx));
    presets.append(&property_row("Theme File", &files));
    page.append(presets.widget());

    for (i, (title, section, fields)) in SECTIONS.iter().enumerate() {
        let panel = Panel::new(title, i == 0);
        for field in *fields {
            let swatch = ColorSwatch::new(
                (field.get)(&b.ctx.preferences().get().effective_theme()),
                field.label,
            );
            let set = field.set;
            let is_accent = *section == "ui" && field.key == "accent";
            let binder = b.clone();
            swatch.connect_changed(move |color| {
                binder.update_theme(|p| edit_theme_color(p, set, is_accent, color));
            });
            let get = field.get;
            let s = swatch.clone();
            b.on_refresh(move |p| s.set_color(get(&p.effective_theme())));
            panel.append(&property_row(field.label, swatch.widget()));
        }
        page.append(panel.widget());
    }
    page
}

/// Applies a swatch edit. The accent override is baked into the theme
/// first, so what the user sees is what they edit.
fn edit_theme_color(p: &mut Preferences, set: fn(&mut Theme, Color), accent: bool, c: Color) {
    if p.themes.accent != Accent::Theme {
        p.theme = p.effective_theme();
        p.themes.accent = Accent::Theme;
    }
    if accent {
        p.theme.set_accent(c);
    } else {
        set(&mut p.theme, c);
    }
}

fn toml_filter() -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Viscas themes (*.toml)"));
    filter.add_pattern("*.toml");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    filters
}

fn parent_window(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    widget.root().and_downcast::<gtk::Window>()
}

fn import_theme(button: &gtk::Button, ctx: &AppCtx) {
    let parent = parent_window(button);
    let dialog = gtk::FileDialog::builder()
        .title("Import Theme")
        .filters(&toml_filter())
        .modal(true)
        .build();
    let ctx = ctx.clone();
    let transient = parent.clone();
    dialog.open(
        transient.as_ref(),
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(path) = result.map(|f| f.path()) else {
                return; // cancelled
            };
            let loaded = path
                .as_ref()
                .ok_or_else(|| "not a local file".to_owned())
                .and_then(|p| prefs::read_capped(p).map_err(|e| e.to_string()))
                .and_then(|text| Theme::from_toml(&text));
            let mut theme = match loaded {
                Ok(t) => t,
                Err(e) => {
                    if let Some(w) = &parent {
                        alert(w, "Could not import theme", &e);
                    }
                    return;
                }
            };
            // Never shadow or overwrite an installed theme: rename on clash.
            let taken: Vec<String> = ctx
                .preferences()
                .themes()
                .into_iter()
                .map(|t| t.name)
                .collect();
            theme.name =
                prefs::unique_name(&prefs::clean_name(&theme.name, prefs::MAX_NAME_LEN), &taken);
            // Install a copy so the theme shows up in the preset list.
            if let Some(dir) = prefs::themes_dir() {
                let stem = prefs::theme_file_name(&theme.name);
                let stem = stem.trim_end_matches(".toml");
                let file = std::iter::once(dir.join(format!("{stem}.toml")))
                    .chain((2..).map(|n| dir.join(format!("{stem}_{n}.toml"))))
                    .find(|f| !f.exists())
                    .unwrap_or_else(|| dir.join(format!("{stem}.toml")));
                if let Err(e) = theme
                    .to_toml()
                    .map_err(std::io::Error::other)
                    .and_then(|text| prefs::write_atomic(&file, &text))
                {
                    glib::g_warning!("viscas", "cannot install theme {}: {e}", file.display());
                }
            }
            settings::reload_user_themes(&ctx);
            ctx.preferences().update_theme(|p| {
                p.themes.preset = theme.name.clone();
                p.themes.accent = Accent::Theme;
                p.theme = theme;
            });
        },
    );
}

fn export_theme(button: &gtk::Button, ctx: &AppCtx) {
    let parent = parent_window(button);
    let theme = ctx.preferences().get().effective_theme();
    let dialog = gtk::FileDialog::builder()
        .title("Export Theme")
        .filters(&toml_filter())
        .initial_name(prefs::theme_file_name(&theme.name))
        .modal(true)
        .build();
    let transient = parent.clone();
    dialog.save(
        transient.as_ref(),
        None::<&gio::Cancellable>,
        move |result| {
            let Some(path) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            if let Err(e) = theme
                .to_toml()
                .map_err(std::io::Error::other)
                .and_then(|text| prefs::write_atomic(&path, &text))
                && let Some(w) = &parent
            {
                alert(w, "Could not export theme", &e.to_string());
            }
        },
    );
}

fn viewport_page(b: &Binder) -> gtk::Box {
    let page = page();
    let grid = Panel::new("Grid", true);
    grid.append(&b.check("Show Grid", |p| p.viewport.grid, |p, v| p.viewport.grid = v));
    grid.append(&b.check(
        "Minor Grid",
        |p| p.viewport.minor_grid,
        |p, v| p.viewport.minor_grid = v,
    ));
    grid.append(&b.check(
        "Axis Labels",
        |p| p.viewport.axis_labels,
        |p, v| p.viewport.axis_labels = v,
    ));
    let style = b.choice(
        &["Lines", "Dots"],
        |p| u32::from(p.viewport.grid_style == GridStyle::Dots),
        |p, i| {
            p.viewport.grid_style = if i == 1 {
                GridStyle::Dots
            } else {
                GridStyle::Lines
            }
        },
    );
    grid.append(&property_row("Grid Style", &style));
    page.append(grid.widget());

    let quality = Panel::new("Quality", true);
    let width = b.scrub(
        prefs::LINE_WIDTH,
        |p| p.viewport.line_width,
        |p, v| p.viewport.line_width = v,
    );
    width.set_suffix(" px");
    quality.append(&property_row("Line Width", width.widget()));
    quality.append(&b.check(
        "Antialiasing",
        |p| p.viewport.antialias,
        |p, v| p.viewport.antialias = v,
    ));
    quality.append(&b.check(
        "Lock Aspect Ratio",
        |p| p.viewport.lock_aspect,
        |p, v| p.viewport.lock_aspect = v,
    ));
    page.append(quality.widget());
    page
}

fn editing_page(b: &Binder) -> gtk::Box {
    let page = page();
    let undo = Panel::new("Undo", true);
    let steps = b.scrub(
        prefs::UNDO_STEPS,
        |p| f64::from(p.editing.undo_steps),
        |p, v| p.editing.undo_steps = v as u32,
    );
    undo.append(&property_row("Undo Steps", steps.widget()));
    page.append(undo.widget());

    let exprs = Panel::new("Expressions", true);
    let labels: Vec<String> = DefinitionOp::ALL
        .iter()
        .map(|op| match op {
            DefinitionOp::ColonEquals => format!("{}   (a := 3)", op.symbol()),
            DefinitionOp::Equals => format!("{}    (a = 3)", op.symbol()),
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let op = b.choice(
        &refs,
        |p| u32::from(p.editing.definition_op == DefinitionOp::Equals),
        |p, i| {
            p.editing.definition_op = if i == 1 {
                DefinitionOp::Equals
            } else {
                DefinitionOp::ColonEquals
            }
        },
    );
    op.set_tooltip_text(Some("Operator that defines a named node"));
    exprs.append(&property_row("Definition Operator", &op));
    page.append(exprs.widget());
    page
}

fn keymap_page() -> gtk::Box {
    let page = page();
    let panel = Panel::new("Keyboard", true);
    for (keys, title) in shortcuts::keymap() {
        let keycap = gtk::Label::builder()
            .label(&keys)
            .xalign(0.0)
            .halign(gtk::Align::Start)
            .css_classes(["keycap"])
            .build();
        panel.append(&property_row(&title, &keycap));
    }
    page.append(panel.widget());
    let mouse = Panel::new("Viewport Mouse", true);
    for (keys, title) in [("Scroll", "Zoom at cursor"), ("Drag", "Pan")] {
        let keycap = gtk::Label::builder()
            .label(keys)
            .halign(gtk::Align::Start)
            .css_classes(["keycap"])
            .build();
        mouse.append(&property_row(title, &keycap));
    }
    page.append(mouse.widget());
    let note = gtk::Label::builder()
        .label("Shortcuts are read-only for now; remapping arrives with the keymap editor.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["prefs-note"])
        .build();
    page.append(&note);
    page
}

fn save_load_page(b: &Binder) -> gtk::Box {
    let page = page();
    let auto = Panel::new("Auto Save", true);
    let interval = b.scrub(
        prefs::AUTOSAVE_MINUTES,
        |p| f64::from(p.save_load.autosave_minutes),
        |p, v| p.save_load.autosave_minutes = v as u32,
    );
    interval.set_suffix(" min");
    interval.set_tooltip("Minutes between document auto-saves (0 disables)");
    auto.append(&property_row("Timer", interval.widget()));
    page.append(auto.widget());
    let files = Panel::new("Files", true);
    let recent = b.scrub(
        prefs::RECENT_FILES,
        |p| f64::from(p.save_load.recent_files),
        |p, v| p.save_load.recent_files = v as u32,
    );
    files.append(&property_row("Recent Files", recent.widget()));
    page.append(files.widget());
    let path = prefs::preferences_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unavailable".to_owned());
    page.append(
        &gtk::Label::builder()
            .label(format!("Preferences file: {path}"))
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .css_classes(["prefs-note", "mono"])
            .build(),
    );
    page
}

fn system_page(b: &Binder) -> gtk::Box {
    let page = page();
    let appearance = Panel::new("Appearance", true);
    appearance.append(&b.check(
        "Follow System Light/Dark",
        |p| p.system.follow_system,
        |p, v| p.system.follow_system = v,
    ));
    let scheme = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim"])
        .build();
    let ctx = b.ctx.clone();
    let s = scheme.clone();
    b.on_refresh(move |_| {
        s.set_label(match ctx.preferences().system_dark() {
            Some(true) => "Dark",
            Some(false) => "Light",
            None => "Unknown (no settings portal)",
        });
    });
    appearance.append(&property_row("Desktop Scheme", &scheme));
    let light = b.theme_choice(
        |p| p.system.light_theme.clone(),
        |p, n| p.system.light_theme = n,
    );
    light.set_tooltip_text(Some("Theme used when the desktop is light"));
    appearance.append(&property_row("Light Theme", &light));
    let dark = b.theme_choice(
        |p| p.system.dark_theme.clone(),
        |p, n| p.system.dark_theme = n,
    );
    dark.set_tooltip_text(Some("Theme used when the desktop is dark"));
    appearance.append(&property_row("Dark Theme", &dark));
    page.append(appearance.widget());
    page.append(
        &gtk::Label::builder()
            .label("Choosing a theme by hand turns following the system off.")
            .wrap(true)
            .xalign(0.0)
            .css_classes(["prefs-note"])
            .build(),
    );
    page
}
