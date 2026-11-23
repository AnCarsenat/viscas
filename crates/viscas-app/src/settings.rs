//! Runtime glue for preferences: loading at startup, (auto-)saving, user
//! themes, live theme application and the desktop color scheme.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::ctx::AppCtx;
use crate::prefs::{self, Preferences};
use crate::theme;

/// Delay between the last change and an automatic save, so scrubbing a
/// value does not write the file on every motion event.
const AUTO_SAVE_DELAY: Duration = Duration::from_millis(600);

thread_local! {
    /// An automatic save is scheduled.
    static SAVE_PENDING: Cell<bool> = const { Cell::new(false) };
    /// A theme application is scheduled on idle.
    static APPLY_PENDING: Cell<bool> = const { Cell::new(false) };
    /// The settings portal proxy; kept alive so its signal subscription
    /// (`SettingChanged`) keeps working.
    static PORTAL: RefCell<Option<gio::DBusProxy>> = const { RefCell::new(None) };
}

/// Loads preferences and user themes into `ctx`, applies the theme and
/// keeps applying/saving it on every change. Call once, after the display
/// exists.
pub fn init(ctx: &AppCtx) {
    let store = ctx.preferences();
    reload_user_themes(ctx);
    load_into(ctx);
    theme::apply(&store.get());

    let weak = ctx.downgrade();
    store.connect_changed(move |p: &Preferences| {
        // Coalesce bursts (scrubbing, revert) into one stylesheet reload.
        if !APPLY_PENDING.replace(true) {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                APPLY_PENDING.set(false);
                if let Some(ctx) = weak.upgrade() {
                    theme::apply(&ctx.preferences().get());
                }
            });
        }
        if p.auto_save && !SAVE_PENDING.replace(true) {
            let weak = weak.clone();
            glib::timeout_add_local_once(AUTO_SAVE_DELAY, move || {
                if let Some(ctx) = weak.upgrade() {
                    flush(&ctx);
                }
            });
        }
        true
    });
    watch_color_scheme(ctx);
}

/// Writes a scheduled automatic save now (e.g. at shutdown).
pub fn flush(ctx: &AppCtx) {
    if SAVE_PENDING.replace(false)
        && ctx.preferences().with(|p| p.auto_save)
        && let Err(e) = save_now(ctx)
    {
        glib::g_warning!("viscas", "auto-save preferences: {e}");
    }
}

/// Loads the preferences file into `ctx`. When part of the file had to be
/// discarded, the original is backed up first and a notice is recorded.
fn load_into(ctx: &AppCtx) {
    let Some(path) = prefs::preferences_path() else {
        return;
    };
    let loaded = prefs::load(&path);
    log_all(&loaded.warnings);
    let notice = if loaded.lossy {
        Some(match prefs::backup(&path) {
            Ok(bak) => format!(
                "Some preferences were invalid and reset; original saved as {}",
                bak.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ),
            Err(e) => format!("Some preferences were invalid and reset (backup failed: {e})"),
        })
    } else {
        None
    };
    ctx.preferences().set_load_notice(notice);
    ctx.preferences().replace(loaded.prefs);
}

fn log_all(warnings: &[String]) {
    for w in warnings {
        glib::g_warning!("viscas", "preferences: {w}");
    }
}

/// Writes the current preferences now; returns an error message on failure.
pub fn save_now(ctx: &AppCtx) -> Result<(), String> {
    SAVE_PENDING.set(false);
    let path = prefs::preferences_path().ok_or("no config directory ($HOME unset)")?;
    prefs::save(&path, &ctx.preferences().get())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Reloads the saved preferences (defaults when there is no file).
pub fn revert(ctx: &AppCtx) {
    load_into(ctx);
}

/// Rescans the user themes directory.
pub fn reload_user_themes(ctx: &AppCtx) {
    if let Some(dir) = prefs::themes_dir() {
        let (themes, warnings) = prefs::load_user_themes(&dir);
        log_all(&warnings);
        ctx.preferences().set_user_themes(themes);
    }
}

/// Maps the freedesktop `color-scheme` value to dark (`Some(true)`),
/// light (`Some(false)`) or no preference.
pub fn scheme_is_dark(value: u32) -> Option<bool> {
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// The `u32` inside a portal reply, unwrapping tuples and variants.
fn portal_u32(mut v: glib::Variant) -> Option<u32> {
    for _ in 0..8 {
        if let Some(n) = v.get::<u32>() {
            return Some(n);
        }
        v = if v.type_() == glib::VariantTy::VARIANT {
            v.as_variant()?
        } else if v.is_container() && v.n_children() > 0 {
            v.child_value(0)
        } else {
            return None;
        };
    }
    None
}

/// Tracks the desktop light/dark preference through the settings portal.
/// Without a portal the scheme stays unknown and following does nothing.
fn watch_color_scheme(ctx: &AppCtx) {
    let weak = ctx.downgrade();
    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        None::<&gio::Cancellable>,
        move |result| {
            let proxy = match result {
                Ok(p) => p,
                Err(e) => {
                    glib::g_debug!("viscas", "no settings portal: {e}");
                    return;
                }
            };
            PORTAL.with_borrow_mut(|slot| *slot = Some(proxy.clone()));
            let read_weak = weak.clone();
            proxy.call(
                "ReadOne",
                Some(&("org.freedesktop.appearance", "color-scheme").to_variant()),
                gio::DBusCallFlags::NONE,
                2000,
                None::<&gio::Cancellable>,
                move |reply| {
                    let dark = reply.ok().and_then(portal_u32).and_then(scheme_is_dark);
                    if let Some(ctx) = read_weak.upgrade() {
                        ctx.preferences().set_system_dark(dark);
                    }
                },
            );
            proxy.connect_local("g-signal", false, move |args| {
                let name = args.get(2).and_then(|v| v.get::<String>().ok());
                let params = args.get(3).and_then(|v| v.get::<glib::Variant>().ok());
                if let (Some(name), Some(params)) = (name, params) {
                    let key = params.try_child_value(1).and_then(|k| k.get::<String>());
                    if name == "SettingChanged" && key.as_deref() == Some("color-scheme") {
                        let dark = params.try_child_value(2).and_then(portal_u32);
                        if let Some(ctx) = weak.upgrade() {
                            ctx.preferences()
                                .set_system_dark(dark.and_then(scheme_is_dark));
                        }
                    }
                }
                None
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_scheme_values() {
        assert_eq!(scheme_is_dark(0), None);
        assert_eq!(scheme_is_dark(1), Some(true));
        assert_eq!(scheme_is_dark(2), Some(false));
        assert_eq!(scheme_is_dark(7), None);
    }

    #[test]
    fn portal_reply_unwrapping() {
        let inner = 1u32.to_variant();
        let reply = (glib::Variant::from_variant(&inner),).to_variant();
        assert_eq!(portal_u32(reply), Some(1));
        let nested = (glib::Variant::from_variant(&glib::Variant::from_variant(
            &inner,
        )),)
            .to_variant();
        assert_eq!(portal_u32(nested), Some(1));
        assert_eq!(portal_u32("x".to_variant()), None);
    }
}
