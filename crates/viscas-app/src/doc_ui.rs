//! GTK side of documents: preference → engine wiring, window actions
//! (new/open/save/undo/redo, mode toggles), title and close confirmation.
//!
//! File reading/parsing and serialization/writing run on a blocking
//! worker (`gio::spawn_blocking`); results are applied on the main loop.
//! The document is only replaced once the new one loaded successfully,
//! and the save prompt comes after the file was picked.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use viscas_core::eval::AngleUnit;
use viscas_engine::{Engine, EngineSettings};

use crate::areas::Areas;
use crate::ctx::AppCtx;
use crate::doc::{DocEvent, palette_for_theme, settings_from_prefs};

/// File extension of documents.
pub const EXTENSION: &str = "viscas";

thread_local! {
    /// A save prompt is showing (no stacked prompts).
    static ASKING: Cell<bool> = const { Cell::new(false) };
    /// The user agreed to close/quit: windows close without asking.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
    /// A background save or open is running.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// Applies the preferences to the engine now and after every change:
/// evaluation settings, plot palette (from the theme) and undo depth.
pub fn connect_prefs(ctx: &AppCtx) {
    let last_palette = Rc::new(RefCell::new(Vec::new()));
    let apply = {
        let engine = ctx.engine().clone();
        move |p: &crate::prefs::Preferences| {
            let palette = palette_for_theme(&p.effective_theme());
            let palette_changed = *last_palette.borrow() != palette;
            if palette_changed {
                *last_palette.borrow_mut() = palette.clone();
            }
            engine.set_history_limit(p.editing.undo_steps as usize);
            engine.set_settings_deferred(settings_from_prefs(p, &engine.effective_settings()));
            if palette_changed {
                engine.edit(|e| e.set_palette(palette));
                engine.events().emit(&DocEvent::Colors);
            }
        }
    };
    // A copy: no preferences borrow is held while listeners run.
    apply(&ctx.preferences().get());
    // The engine handle does not own the store, so capturing it is fine.
    ctx.preferences().connect_changed(move |p| {
        apply(p);
        true
    });
}

/// Exact/approx toggled (applied by the pump; display only).
pub fn toggled_approx(s: EngineSettings) -> EngineSettings {
    EngineSettings {
        approx: !s.approx,
        ..s
    }
}

/// Radians/degrees toggled.
pub fn toggled_angle(s: EngineSettings) -> EngineSettings {
    EngineSettings {
        angle: match s.angle {
            AngleUnit::Radians => AngleUnit::Degrees,
            AngleUnit::Degrees => AngleUnit::Radians,
        },
        ..s
    }
}

/// `path` with the document extension added when it has none.
pub fn with_extension(path: &Path) -> PathBuf {
    if path.extension().is_some() {
        path.to_path_buf()
    } else {
        path.with_extension(EXTENSION)
    }
}

/// What a file dialog returned.
enum Picked {
    Path(PathBuf),
    /// Closed without choosing.
    Dismissed,
    Failed(String),
}

fn picked(res: Result<gio::File, glib::Error>) -> Picked {
    match res {
        Ok(file) => match file.path() {
            Some(p) => Picked::Path(p),
            None => Picked::Failed(format!(
                "“{}” is not a local file; only local files are supported.",
                file.uri()
            )),
        },
        Err(e)
            if e.matches(gtk::DialogError::Dismissed)
                || e.matches(gtk::DialogError::Cancelled)
                || e.matches(gio::IOErrorEnum::Cancelled) =>
        {
            Picked::Dismissed
        }
        Err(e) => Picked::Failed(e.to_string()),
    }
}

/// Window state shared by the document actions.
#[derive(Clone)]
struct DocUi {
    ctx: AppCtx,
    window: glib::WeakRef<gtk::ApplicationWindow>,
    areas: Areas,
    status: glib::WeakRef<gtk::Label>,
}

impl DocUi {
    fn window(&self) -> Option<gtk::ApplicationWindow> {
        self.window.upgrade()
    }

    /// While a text field has uncommitted edits, Ctrl+Z / Ctrl+Shift+Z
    /// undo the typing (the widget's own text undo), not the document.
    /// Returns whether it was handled that way.
    fn text_undo(&self, redo: bool) -> bool {
        let Some(target) = self
            .window()
            .and_then(|w| crate::window::text_undo_target(w.upcast_ref(), redo))
        else {
            return false;
        };
        let action = if redo { "text.redo" } else { "text.undo" };
        WidgetExt::activate_action(&target, action, None).is_ok()
    }

    fn set_status(&self, text: &str) {
        if let Some(l) = self.status.upgrade() {
            l.set_label(text);
        }
    }

    fn error(&self, message: &str, detail: &str) {
        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .detail(detail)
            .buttons(["Close"])
            .modal(true)
            .build();
        dialog.show(self.window().as_ref());
    }

    fn new_document(&self) {
        self.ctx.engine().replace_with(|e| e.new_document());
        self.areas.reset_default();
    }

    /// Reads and parses `path` off the UI thread, then replaces the
    /// document (only on success).
    fn load(&self, path: PathBuf) {
        if BUSY.replace(true) {
            return;
        }
        self.set_status("opening…");
        let this = self.clone();
        glib::spawn_future_local(async move {
            let p = path.clone();
            let res = gio::spawn_blocking(move || Engine::load_file(&p)).await;
            BUSY.set(false);
            this.set_status("ready");
            match res {
                Ok(Ok(scene)) => {
                    let layout = scene.layout().cloned();
                    this.ctx
                        .engine()
                        .replace_with(|e| e.install_document(scene, Some(path)));
                    let restored = layout.is_some_and(|l| this.areas.restore_blob(&l));
                    if !restored {
                        this.areas.reset_default();
                    }
                }
                Ok(Err(err)) => this.error("Could not open the document", &err.to_string()),
                Err(_) => this.error("Could not open the document", "the loader crashed"),
            }
        });
    }

    /// Serializes and writes the document off the UI thread; `then`
    /// runs with the outcome.
    fn save_to(&self, path: &Path, then: impl FnOnce(bool) + 'static) {
        if BUSY.replace(true) {
            then(false);
            return;
        }
        let path = with_extension(path);
        let engine = self.ctx.engine().clone();
        engine.flush();
        let layout = self.areas.to_blob();
        engine.edit(|e| e.set_layout(Some(layout)));
        let depth = engine.begin_save();
        let task = engine.with(|e| e.save_task(path.clone()));
        self.set_status("saving…");
        let this = self.clone();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(task).await;
            BUSY.set(false);
            this.set_status("ready");
            match res {
                Ok(Ok(())) => {
                    engine.edit(|e| e.set_document_path(Some(path)));
                    engine.finish_save(depth);
                    then(true);
                }
                Ok(Err(err)) => {
                    this.error("Could not save the document", &err.to_string());
                    then(false);
                }
                Err(_) => {
                    this.error("Could not save the document", "the writer crashed");
                    then(false);
                }
            }
        });
    }

    /// Saves (asking for a path when there is none); `then` runs with the
    /// outcome.
    fn save(&self, then: impl FnOnce(bool) + 'static) {
        match self.ctx.engine().path() {
            Some(path) => self.save_to(&path, then),
            None => self.save_as(then),
        }
    }

    fn save_as(&self, then: impl FnOnce(bool) + 'static) {
        let dialog = file_dialog("Save Document");
        let name = format!(
            "{}.{EXTENSION}",
            crate::doc::document_name(self.ctx.engine().path().as_deref())
        );
        dialog.set_initial_name(Some(&name));
        let this = self.clone();
        dialog.save(
            self.window().as_ref(),
            gio::Cancellable::NONE,
            move |res| match picked(res) {
                Picked::Path(path) => this.save_to(&path, then),
                Picked::Dismissed => then(false),
                Picked::Failed(err) => {
                    this.error("Could not save the document", &err);
                    then(false);
                }
            },
        );
    }

    fn open(&self) {
        let dialog = file_dialog("Open Document");
        let this = self.clone();
        dialog.open(
            self.window().as_ref(),
            gio::Cancellable::NONE,
            move |res| match picked(res) {
                Picked::Path(path) => {
                    let me = this.clone();
                    this.confirm_discard(move |ok| {
                        if ok {
                            me.load(path);
                        }
                    });
                }
                Picked::Dismissed => {}
                Picked::Failed(err) => this.error("Could not open the document", &err),
            },
        );
    }

    /// Runs `then(true)` when the document may be replaced or closed:
    /// unmodified, saved, or the user chose to discard. While a prompt is
    /// showing, further requests are answered `false`.
    fn confirm_discard(&self, then: impl FnOnce(bool) + 'static) {
        self.ctx.engine().flush();
        if !self.ctx.engine().is_modified() {
            then(true);
            return;
        }
        if ASKING.replace(true) {
            then(false);
            return;
        }
        let name = crate::doc::document_name(self.ctx.engine().path().as_deref());
        let dialog = gtk::AlertDialog::builder()
            .message(format!("Save changes to “{name}”?"))
            .detail("Your changes will be lost if you don't save them.")
            .buttons(["Cancel", "Discard", "Save"])
            .cancel_button(0)
            .default_button(2)
            .modal(true)
            .build();
        let this = self.clone();
        dialog.choose(self.window().as_ref(), gio::Cancellable::NONE, move |res| {
            ASKING.set(false);
            match res {
                Ok(1) => then(true),
                Ok(2) => this.save(then),
                _ => then(false),
            }
        });
    }

    /// Asks once, then closes every window (the app exits with the last).
    /// Cancel aborts the whole quit.
    fn quit(&self) {
        let this = self.clone();
        self.confirm_discard(move |ok| {
            if !ok {
                return;
            }
            CLOSING.set(true);
            let app = this.window().and_then(|w| w.application());
            if let Some(app) = app {
                for w in app.windows() {
                    w.close();
                }
            }
        });
    }
}

fn file_dialog(title: &str) -> gtk::FileDialog {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Viscas documents"));
    filter.add_suffix(EXTENSION);
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    gtk::FileDialog::builder()
        .title(title)
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build()
}

/// Widgets of the status bar the document code updates.
pub struct StatusWidgets {
    pub mode_chip: gtk::Label,
    pub angle_chip: gtk::Label,
    pub status: gtk::Label,
}

/// Installs the document actions on `window` and keeps its title, the
/// undo/redo enable state and the mode chips in sync with the document.
pub fn install(
    window: &gtk::ApplicationWindow,
    ctx: &AppCtx,
    areas: &Areas,
    widgets: StatusWidgets,
) {
    let ui = DocUi {
        ctx: ctx.clone(),
        window: window.downgrade(),
        areas: areas.clone(),
        status: widgets.status.downgrade(),
    };

    let add = |name: &str, f: Rc<dyn Fn(&DocUi)>| {
        let action = gio::SimpleAction::new(name, None);
        let ui = ui.clone();
        action.connect_activate(move |_, _| f(&ui));
        window.add_action(&action);
        action
    };
    add(
        "new",
        Rc::new(|ui: &DocUi| {
            let me = ui.clone();
            ui.confirm_discard(move |ok| {
                if ok {
                    me.new_document();
                }
            });
        }),
    );
    add("open", Rc::new(|ui: &DocUi| ui.open()));
    add("save", Rc::new(|ui: &DocUi| ui.save(|_| {})));
    add("save-as", Rc::new(|ui: &DocUi| ui.save_as(|_| {})));
    add("quit-request", Rc::new(|ui: &DocUi| ui.quit()));
    let undo = add(
        "undo",
        Rc::new(|ui: &DocUi| {
            if !ui.text_undo(false) {
                ui.ctx.engine().undo();
            }
        }),
    );
    let redo = add(
        "redo",
        Rc::new(|ui: &DocUi| {
            if !ui.text_undo(true) {
                ui.ctx.engine().redo();
            }
        }),
    );
    add(
        "toggle-approx",
        Rc::new(|ui: &DocUi| {
            let e = ui.ctx.engine();
            e.set_settings_deferred(toggled_approx(e.effective_settings()));
        }),
    );
    add(
        "toggle-angle",
        Rc::new(|ui: &DocUi| {
            let e = ui.ctx.engine();
            e.set_settings_deferred(toggled_angle(e.effective_settings()));
        }),
    );

    // Title, undo/redo and chips follow the document.
    let StatusWidgets {
        mode_chip,
        angle_chip,
        ..
    } = widgets;
    let refresh = {
        let engine = ctx.engine().clone();
        let window = window.downgrade();
        let undo = undo.downgrade();
        let redo = redo.downgrade();
        let mode_chip = mode_chip.downgrade();
        let angle_chip = angle_chip.downgrade();
        move || {
            let (can_undo, can_redo, settings) = engine.with(|e| {
                let s = e.scene();
                (s.can_undo(), s.can_redo(), *e.settings())
            });
            if let Some(a) = undo.upgrade() {
                a.set_enabled(can_undo);
            }
            if let Some(a) = redo.upgrade() {
                a.set_enabled(can_redo);
            }
            if let Some(w) = window.upgrade() {
                w.set_title(Some(&engine.title()));
            }
            if let (Some(m), Some(a)) = (mode_chip.upgrade(), angle_chip.upgrade()) {
                update_chips(&m, &a, &settings);
            }
        }
    };
    refresh();
    let sub = ctx.engine().subscribe(move |ev| match ev {
        DocEvent::Scene(_) | DocEvent::Document | DocEvent::Settings(_) | DocEvent::Replaced => {
            refresh()
        }
        _ => {}
    });
    // Disconnect with the window.
    let sub = RefCell::new(Some(sub));
    window.connect_destroy(move |_| drop(sub.take()));

    for (chip, action) in [
        (&mode_chip, "win.toggle-approx"),
        (&angle_chip, "win.toggle-angle"),
    ] {
        let click = gtk::GestureClick::new();
        let w = window.downgrade();
        click.connect_released(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(w) = w.upgrade() {
                let _ = WidgetExt::activate_action(&w, action, None);
            }
        });
        chip.add_controller(click);
        chip.set_cursor_from_name(Some("pointer"));
    }

    let close_ui = ui.clone();
    window.connect_close_request(move |w| {
        if CLOSING.get() {
            return glib::Propagation::Proceed;
        }
        close_ui.ctx.engine().flush();
        if !close_ui.ctx.engine().is_modified() {
            return glib::Propagation::Proceed;
        }
        let w = w.downgrade();
        close_ui.confirm_discard(move |ok| {
            if ok && let Some(w) = w.upgrade() {
                CLOSING.set(true);
                w.close();
            }
        });
        glib::Propagation::Stop
    });
}

/// Updates the status-bar mode chips for `settings`.
fn update_chips(mode: &gtk::Label, angle: &gtk::Label, settings: &EngineSettings) {
    let (text, tip, accent) = if settings.approx {
        (
            "APPROX",
            "Results shown as decimals — click for exact",
            false,
        )
    } else {
        ("EXACT", "Results stay exact — click for decimals", true)
    };
    mode.set_label(text);
    mode.set_tooltip_text(Some(tip));
    if accent {
        mode.add_css_class("accent");
    } else {
        mode.remove_css_class("accent");
    }
    let (text, tip) = match settings.angle {
        AngleUnit::Radians => ("RAD", "Angle unit: radians — click for degrees"),
        AngleUnit::Degrees => ("DEG", "Angle unit: degrees — click for radians"),
    };
    angle.set_label(text);
    angle.set_tooltip_text(Some(tip));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_added_once() {
        assert_eq!(with_extension(Path::new("a")), PathBuf::from("a.viscas"));
        assert_eq!(
            with_extension(Path::new("a.viscas")),
            PathBuf::from("a.viscas")
        );
    }

    #[test]
    fn toggles_flip_one_setting() {
        let s = EngineSettings::default();
        let a = toggled_approx(s);
        assert!(a.approx && a.angle == s.angle);
        assert_eq!(toggled_approx(a), s);
        let d = toggled_angle(s);
        assert_eq!(d.angle, AngleUnit::Degrees);
        assert_eq!(toggled_angle(d), s);
    }
}
