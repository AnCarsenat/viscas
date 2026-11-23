//! GTK side of documents: preference → engine wiring, window actions
//! (new/open/save/undo/redo, mode toggles), title and close confirmation.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use viscas_core::eval::AngleUnit;
use viscas_engine::EngineSettings;

use crate::areas::Areas;
use crate::ctx::AppCtx;
use crate::doc::{DocEvent, palette_for_theme, settings_from_prefs};

/// File extension of documents.
pub const EXTENSION: &str = "viscas";

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
            let limit = p.editing.undo_steps as usize;
            engine.edit(|e| {
                let s = settings_from_prefs(p, e.settings());
                e.set_settings(s);
                if palette_changed {
                    e.set_palette(palette);
                }
                e.scene_mut().set_history_limit(limit);
            });
            if palette_changed {
                engine.events().emit(&DocEvent::Colors);
            }
        }
    };
    ctx.preferences().with(&apply);
    // The engine outlives every listener (both live in the context), so
    // capturing a handle clone is fine: it does not own the store.
    ctx.preferences().connect_changed(move |p| {
        apply(p);
        true
    });
}

/// Toggles exact/approx display.
pub fn toggle_approx(ctx: &AppCtx) {
    ctx.engine().edit(|e| {
        let s = EngineSettings {
            approx: !e.settings().approx,
            ..*e.settings()
        };
        e.set_settings(s);
    });
}

/// Toggles radians/degrees.
pub fn toggle_angle(ctx: &AppCtx) {
    ctx.engine().edit(|e| {
        let angle = match e.settings().angle {
            AngleUnit::Radians => AngleUnit::Degrees,
            AngleUnit::Degrees => AngleUnit::Radians,
        };
        let s = EngineSettings {
            angle,
            ..*e.settings()
        };
        e.set_settings(s);
    });
}

/// `path` with the document extension added when it has none.
pub fn with_extension(path: &Path) -> PathBuf {
    if path.extension().is_some() {
        path.to_path_buf()
    } else {
        path.with_extension(EXTENSION)
    }
}

/// Window state shared by the document actions.
#[derive(Clone)]
struct DocUi {
    ctx: AppCtx,
    window: glib::WeakRef<gtk::ApplicationWindow>,
    areas: Areas,
    /// Set once the user agreed to close (skips the confirmation).
    closing: Rc<Cell<bool>>,
}

impl DocUi {
    fn window(&self) -> Option<gtk::ApplicationWindow> {
        self.window.upgrade()
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
        self.apply_prefs_after_replace();
    }

    fn open_path(&self, path: &Path) {
        let result = self.ctx.engine().replace_with(|e| {
            e.open_path(path)
                .map(|()| e.layout().cloned())
                .map_err(|err| err.to_string())
        });
        match result {
            Ok(layout) => {
                if let Some(layout) = layout
                    && !self.areas.restore_blob(&layout)
                {
                    glib::g_warning!("viscas", "ignoring the document's invalid layout");
                }
                self.apply_prefs_after_replace();
            }
            Err(err) => self.error("Could not open the document", &err),
        }
    }

    /// A fresh scene resets the undo depth; re-apply the preferences.
    fn apply_prefs_after_replace(&self) {
        let limit = self.ctx.preferences().with(|p| p.editing.undo_steps) as usize;
        self.ctx
            .engine()
            .edit(|e| e.scene_mut().set_history_limit(limit));
    }

    /// Saves to `path`; returns success.
    fn save_to(&self, path: &Path) -> bool {
        let path = with_extension(path);
        let layout = self.areas.to_blob();
        let result = self.ctx.engine().edit(|e| {
            e.set_layout(Some(layout));
            e.save_path(&path).map_err(|err| err.to_string())
        });
        match result {
            Ok(()) => {
                self.ctx.engine().set_modified(false);
                // The path may have changed even if the flag did not.
                self.ctx.engine().events().emit(&DocEvent::Document);
                true
            }
            Err(err) => {
                self.error("Could not save the document", &err);
                false
            }
        }
    }

    /// Saves (asking for a path when there is none); `then` runs with the
    /// outcome.
    fn save(&self, then: impl FnOnce(bool) + 'static) {
        match self.ctx.engine().path() {
            Some(path) => then(self.save_to(&path)),
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
            move |res| match res.ok().and_then(|f| f.path()) {
                Some(path) => then(this.save_to(&path)),
                None => then(false),
            },
        );
    }

    fn open(&self) {
        let this = self.clone();
        self.confirm_discard(move |ok| {
            if !ok {
                return;
            }
            let dialog = file_dialog("Open Document");
            let me = this.clone();
            dialog.open(this.window().as_ref(), gio::Cancellable::NONE, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    me.open_path(&path);
                }
            });
        });
    }

    /// Runs `then(true)` when the document may be replaced: unmodified,
    /// saved, or the user chose to discard.
    fn confirm_discard(&self, then: impl FnOnce(bool) + 'static) {
        if !self.ctx.engine().is_modified() {
            then(true);
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
        dialog.choose(
            self.window().as_ref(),
            gio::Cancellable::NONE,
            move |res| match res {
                Ok(1) => then(true),
                Ok(2) => this.save(then),
                _ => then(false),
            },
        );
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

/// Installs the document actions on `window` and keeps its title, the
/// undo/redo enable state and the mode chips in sync with the document.
pub fn install(
    window: &gtk::ApplicationWindow,
    ctx: &AppCtx,
    areas: &Areas,
    chips: (gtk::Label, gtk::Label),
) {
    let ui = DocUi {
        ctx: ctx.clone(),
        window: window.downgrade(),
        areas: areas.clone(),
        closing: Rc::default(),
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
    let undo = add(
        "undo",
        Rc::new(|ui: &DocUi| {
            ui.ctx.engine().edit(|e| e.undo());
        }),
    );
    let redo = add(
        "redo",
        Rc::new(|ui: &DocUi| {
            ui.ctx.engine().edit(|e| e.redo());
        }),
    );
    add(
        "toggle-approx",
        Rc::new(|ui: &DocUi| toggle_approx(&ui.ctx)),
    );
    add("toggle-angle", Rc::new(|ui: &DocUi| toggle_angle(&ui.ctx)));

    // Title, undo/redo and chips follow the document.
    let (mode_chip, angle_chip) = chips;
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
        if close_ui.closing.get() || !close_ui.ctx.engine().is_modified() {
            return glib::Propagation::Proceed;
        }
        let me = close_ui.clone();
        let w = w.downgrade();
        close_ui.confirm_discard(move |ok| {
            if ok && let Some(w) = w.upgrade() {
                me.closing.set(true);
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
}
