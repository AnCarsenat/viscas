//! The live keymap: resolved bindings of the current preferences, key
//! event matching, the window key dispatcher and menu accelerator labels.
//!
//! Every window installs [`install`] (capture phase), which runs Window
//! context actions. Editors handle their own context's keys with
//! [`action_for`] on a widget marked by [`mark`]; while such an editor has
//! focus and the key is bound in its context, the dispatcher lets the key
//! through instead. Rebinding in Preferences updates all of this at once.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::model::{ActionSpec, Chord, Context, KeymapPrefs, Mods};
use crate::ctx::{AppCtx, WeakCtx};

/// One resolved binding.
struct Entry {
    context: Context,
    chord: Chord,
    id: String,
    repeat: bool,
}

struct Live {
    prefs: KeymapPrefs,
    specs: Vec<ActionSpec>,
    table: Vec<Entry>,
}

impl Live {
    fn new(prefs: KeymapPrefs) -> Self {
        let mut live = Live {
            prefs,
            specs: super::registry::specs(),
            table: Vec::new(),
        };
        live.rebuild();
        live
    }

    /// Known actions first, then overrides of ids no action knows (bound
    /// as Window commands: a provider may supply them at run time).
    fn rebuild(&mut self) {
        self.table.clear();
        for s in &self.specs {
            for chord in self.prefs.bindings_of(s) {
                self.table.push(Entry {
                    context: s.context,
                    chord,
                    id: s.id.clone(),
                    repeat: s.repeat,
                });
            }
        }
        for (id, keys) in &self.prefs.bindings {
            if self.specs.iter().any(|s| &s.id == id) {
                continue;
            }
            for chord in keys.iter().filter_map(|k| Chord::parse(k).ok()) {
                self.table.push(Entry {
                    context: Context::Window,
                    chord,
                    id: id.clone(),
                    repeat: true,
                });
            }
        }
    }
}

type Listener = Rc<dyn Fn() -> bool>;

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
}

fn with<R>(f: impl FnOnce(&mut Live) -> R) -> R {
    LIVE.with_borrow_mut(|slot| f(slot.get_or_insert_with(|| Live::new(KeymapPrefs::default()))))
}

fn notify() {
    let listeners = LISTENERS.with_borrow(Clone::clone);
    let alive: Vec<bool> = listeners.iter().map(|f| f()).collect();
    LISTENERS.with_borrow_mut(|list| {
        let mut i = 0;
        list.retain(|_| {
            let keep = alive.get(i).copied().unwrap_or(true);
            i += 1;
            keep
        });
    });
}

/// Applies the keymap of the preferences; listeners run on a change.
pub fn set_prefs(prefs: &KeymapPrefs) {
    let changed = with(|l| {
        if &l.prefs == prefs {
            return false;
        }
        l.prefs = prefs.clone();
        l.rebuild();
        true
    });
    if changed {
        notify();
    }
}

/// Re-reads the registry (an action was registered).
pub fn specs_changed() {
    let initialized = LIVE.with_borrow(Option::is_some);
    if initialized {
        with(|l| {
            l.specs = super::registry::specs();
            l.rebuild();
        });
        notify();
    }
}

/// Keeps the live keymap in sync with `ctx`'s preferences.
pub fn bind(ctx: &AppCtx) {
    set_prefs(&ctx.preferences().with(|p| p.keymap.clone()));
    ctx.preferences().connect_changed(|p| {
        set_prefs(&p.keymap);
        true
    });
}

/// Calls `f` after every keymap change while `obj` is alive.
pub fn connect_changed_for<W: IsA<glib::Object>>(obj: &W, f: impl Fn(&W) + 'static) {
    let weak = obj.downgrade();
    LISTENERS.with_borrow_mut(|list| {
        list.push(Rc::new(move || match weak.upgrade() {
            Some(o) => {
                f(&o);
                true
            }
            None => false,
        }));
    });
}

/// Every registered action.
pub fn specs() -> Vec<ActionSpec> {
    with(|l| l.specs.clone())
}

/// Current bindings of action `id`.
pub fn bindings(id: &str) -> Vec<Chord> {
    with(|l| {
        l.table
            .iter()
            .filter(|e| e.id == id)
            .map(|e| e.chord.clone())
            .collect()
    })
}

/// Label of `id`'s bindings (`Ctrl+P / Space`), if it has any.
pub fn label(id: &str) -> Option<String> {
    let list = bindings(id);
    (!list.is_empty()).then(|| {
        list.iter()
            .map(Chord::label)
            .collect::<Vec<_>>()
            .join(" / ")
    })
}

/// Shortcut label for a palette command: the keymap's label when the
/// keymap knows the id (`Some(None)` when it is unbound), else `None`.
pub fn palette_label(id: &str) -> Option<Option<String>> {
    let known = with(|l| l.specs.iter().any(|s| s.id == id) || l.prefs.bindings.contains_key(id));
    known.then(|| label(id))
}

/// GTK accelerator of `id`'s first binding, for menus.
pub fn accel(id: &str) -> Option<String> {
    bindings(id).first().map(Chord::to_accel)
}

/// The action bound to one of `chords` in `context`, with the chord and
/// whether it repeats.
fn lookup(context: Context, chords: &[Chord]) -> Option<(String, Chord, bool)> {
    with(|l| {
        chords.iter().find_map(|c| {
            l.table
                .iter()
                .find(|e| e.context == context && &e.chord == c)
                .map(|e| (e.id.clone(), c.clone(), e.repeat))
        })
    })
}

/// Suspends dispatching (the keymap editor is capturing a chord).
pub fn set_capturing(on: bool) {
    CAPTURING.set(on);
}

fn mods_of(state: gdk::ModifierType) -> Mods {
    let mut m = Mods::NONE;
    for (mask, bit) in [
        (gdk::ModifierType::CONTROL_MASK, Mods::CTRL),
        (gdk::ModifierType::SHIFT_MASK, Mods::SHIFT),
        (gdk::ModifierType::ALT_MASK, Mods::ALT),
        (gdk::ModifierType::SUPER_MASK, Mods::SUPER),
    ] {
        if state.contains(mask) {
            m = m.with(bit);
        }
    }
    m
}

/// The chords a key press can match, layout-aware: the keyval the key
/// typed (letters lowercased, Shift dropped for symbols it selected);
/// plus, for a symbol key that carries a digit on another level of the
/// same layout group (AZERTY `&`/`1`), that digit, so `Ctrl+1` works
/// without Shift as on such layouts' own conventions. Dead keys and
/// lone modifiers give nothing.
pub fn event_chords(key: gdk::Key, keycode: u32, state: gdk::ModifierType) -> Vec<Chord> {
    let mods = mods_of(state);
    let lower = key.to_lower();
    let cased = lower != key.to_upper();
    let printable = !cased
        && key
            .to_unicode()
            .is_some_and(|c| !c.is_control() && !c.is_whitespace());
    let Some(name) = lower.name() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(c) = Chord::from_event(&name, mods, printable) {
        out.push(c);
    }
    if printable
        && keycode != 0
        && !name.chars().all(|c| c.is_ascii_digit())
        && let Some(digit) = digit_on_key(key, keycode)
        && let Some(c) = Chord::from_event(&digit, mods, true)
    {
        out.push(c);
    }
    out
}

/// The digit another level of `keycode` types in the layout group where
/// it typed `key`.
fn digit_on_key(key: gdk::Key, keycode: u32) -> Option<String> {
    let entries = gdk::Display::default()?.map_keycode(keycode)?;
    let group = entries.iter().find(|(_, k)| *k == key)?.0.group();
    entries
        .iter()
        .filter(|(m, _)| m.group() == group)
        .find(|(_, k)| k.to_unicode().is_some_and(|c| c.is_ascii_digit()))
        .and_then(|(_, k)| k.name())
        .map(|n| n.to_string())
}

/// The action of `context` bound to this key press, if any.
pub fn action_for(
    context: Context,
    key: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
) -> Option<String> {
    lookup(context, &event_chords(key, keycode, state)).map(|(id, _, _)| id)
}

/// Whether this key press is bound to action `id`.
#[allow(dead_code, reason = "matching hook for text-editing features")]
pub fn matches(id: &str, key: gdk::Key, keycode: u32, state: gdk::ModifierType) -> bool {
    let chords = event_chords(key, keycode, state);
    bindings(id).iter().any(|b| chords.contains(b))
}

fn class_of(context: Context) -> String {
    format!("keymap-{}", context.id())
}

/// Marks `widget` as the root of an editor whose `context` keys it
/// handles itself.
pub fn mark(widget: &impl IsA<gtk::Widget>, context: Context) {
    widget.add_css_class(&class_of(context));
}

/// Local contexts around the focused widget, innermost first.
fn focus_contexts(focus: Option<&gtk::Widget>) -> Vec<Context> {
    let mut out = Vec::new();
    let mut w = focus.cloned();
    while let Some(cur) = w {
        for c in Context::ALL {
            if c != Context::Window && !out.contains(&c) && cur.has_css_class(&class_of(c)) {
                out.push(c);
            }
        }
        w = cur.parent();
    }
    out
}

fn is_text(w: &gtk::Widget) -> bool {
    w.is::<gtk::Text>()
        || w.is::<gtk::TextView>()
        || w.dynamic_cast_ref::<gtk::Editable>().is_some()
}

/// Whether `chord` types in a text field rather than being a shortcut:
/// no command modifier (function keys excepted), or Ctrl+Alt (AltGr on
/// some layouts).
pub fn types_text(chord: &Chord) -> bool {
    let function_key = chord.key.len() > 1
        && chord.key.starts_with('F')
        && chord.key[1..].chars().all(|c| c.is_ascii_digit());
    (!chord.has_command_mod() && !function_key)
        || (chord.mods.contains(Mods::CTRL) && chord.mods.contains(Mods::ALT))
}

/// Runs command `id` on `window`: an action, else a palette command.
fn run(window: &gtk::Window, ctx: &WeakCtx, id: &str) -> bool {
    if id.starts_with("win.") || id.starts_with("app.") {
        return match gio::Action::parse_detailed_name(id) {
            Ok((name, target)) => {
                WidgetExt::activate_action(window, &name, target.as_ref()).is_ok()
            }
            Err(_) => false,
        };
    }
    let Some(ctx) = ctx.upgrade() else {
        return false;
    };
    match ctx.commands().into_iter().find(|c| c.id == id) {
        Some(cmd) => {
            (cmd.run)();
            true
        }
        None => false,
    }
}

/// Installs the keymap dispatcher on `window`.
pub fn install(window: &impl IsA<gtk::Window>, ctx: &AppCtx) {
    let window: &gtk::Window = window.upcast_ref();
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let held: Rc<RefCell<HashSet<u32>>> = Rc::default();
    keys.connect_key_released(glib::clone!(
        #[strong]
        held,
        move |_, _, keycode, _| {
            held.borrow_mut().remove(&keycode);
        }
    ));
    let weak_ctx = ctx.downgrade();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, keycode, state| {
            let repeated = !held.borrow_mut().insert(keycode);
            dispatch(&window, &weak_ctx, key, keycode, state, repeated)
        }
    ));
    window.add_controller(keys);
}

fn dispatch(
    window: &gtk::Window,
    ctx: &WeakCtx,
    key: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    repeated: bool,
) -> glib::Propagation {
    let proceed = glib::Propagation::Proceed;
    if CAPTURING.get() {
        return proceed;
    }
    let chords = event_chords(key, keycode, state);
    if chords.is_empty() {
        return proceed;
    }
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    let text = focus.as_ref().is_some_and(is_text);
    // The focused editor's own bindings (and text-editing features)
    // win over Window bindings.
    let mut local = focus_contexts(focus.as_ref());
    if text {
        local.push(Context::TextEditing);
    }
    if local.into_iter().any(|c| lookup(c, &chords).is_some()) {
        return proceed;
    }
    let Some((id, chord, repeat)) = lookup(Context::Window, &chords) else {
        return proceed;
    };
    if text && types_text(&chord) {
        return proceed;
    }
    if repeated && !repeat {
        return glib::Propagation::Stop;
    }
    if run(window, ctx, &id) {
        glib::Propagation::Stop
    } else {
        proceed
    }
}

/// Menu item attribute naming the keymap action whose binding it shows.
pub const KEYMAP_ID: &str = "keymap-id";

/// Sets every item's `accel` attribute of `menu` (and its sections and
/// submenus) to its action's current binding.
pub fn refresh_menu_accels(menu: &gio::Menu) {
    for i in 0..menu.n_items() {
        for link in ["section", "submenu"] {
            if let Some(sub) = menu.item_link(i, link).and_downcast::<gio::Menu>() {
                refresh_menu_accels(&sub);
            }
        }
        let string = |name: &str| {
            menu.item_attribute_value(i, name, Some(glib::VariantTy::STRING))
                .and_then(|v| v.str().map(str::to_owned))
        };
        // `keymap-id` names the keymap action of an item whose GAction
        // is a local one (e.g. `vp.reset` is `viewport.reset-view`).
        let detailed = match (string(KEYMAP_ID), string("action")) {
            (Some(id), _) => id,
            (None, Some(action)) => match menu.item_attribute_value(i, "target", None) {
                Some(t) => gio::Action::print_detailed_name(&action, Some(&t)).to_string(),
                None => action,
            },
            (None, None) => continue,
        };
        let want = accel(&detailed);
        let have = menu
            .item_attribute_value(i, "accel", Some(glib::VariantTy::STRING))
            .and_then(|v| v.str().map(str::to_owned));
        if want == have {
            continue;
        }
        let item = gio::MenuItem::from_model(menu, i);
        item.set_attribute_value("accel", want.map(|a| a.to_variant()).as_ref());
        menu.remove(i);
        menu.insert_item(i, &item);
    }
}

/// Plain-text overview of the current bindings, one per line.
pub fn overview() -> String {
    let specs = with(|l| l.specs.clone());
    let mut lines = Vec::new();
    let mut section = String::new();
    for s in &specs {
        let Some(keys) = label(&s.id) else {
            continue;
        };
        if s.category != section {
            section.clone_from(&s.category);
            lines.push(format!("— {section} —"));
        }
        lines.push(format!("{keys:<22}{}", s.title));
    }
    lines.push("— Viewport mouse —".to_owned());
    for (keys, what) in [
        ("Scroll", "Zoom graph at cursor"),
        ("Drag / Middle-drag", "Pan graph (see Preferences ▸ Input)"),
        ("Shift+Drag", "Box zoom"),
        ("Pinch", "Zoom graph"),
        ("Click curve", "Select curve, show roots/extrema"),
        ("Click dot", "Pin / unpin its label"),
    ] {
        lines.push(format!("{keys:<22}{what}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    #[test]
    fn rebinding_takes_effect_live() {
        let mut prefs = KeymapPrefs::default();
        set_prefs(&prefs);
        assert_eq!(
            lookup(Context::Window, &[c("Ctrl+S")]).unwrap().0,
            "win.save"
        );
        assert_eq!(label("win.palette").as_deref(), Some("Ctrl+P / Space"));
        assert_eq!(accel("win.save").as_deref(), Some("<Control>s"));

        let fired = Rc::new(Cell::new(0));
        let owner = glib::Object::new::<glib::Object>();
        let f = fired.clone();
        connect_changed_for(&owner, move |_| f.set(f.get() + 1));

        prefs
            .bindings
            .insert("win.save".into(), vec!["Ctrl+eacute".into()]);
        prefs
            .bindings
            .insert("future.cmd".into(), vec!["F9".into()]);
        set_prefs(&prefs);
        assert_eq!(fired.get(), 1);
        assert!(lookup(Context::Window, &[c("Ctrl+S")]).is_none());
        assert_eq!(
            lookup(Context::Window, &[c("Ctrl+é")]).unwrap().0,
            "win.save"
        );
        assert_eq!(label("win.save").as_deref(), Some("Ctrl+É"));
        assert_eq!(palette_label("win.save"), Some(Some("Ctrl+É".to_owned())));
        assert_eq!(palette_label("node.goto::1"), None);
        // Unknown ids still resolve (a provider may run them).
        assert_eq!(lookup(Context::Window, &[c("F9")]).unwrap().0, "future.cmd");
        // Same keymap again: no notification.
        set_prefs(&prefs);
        assert_eq!(fired.get(), 1);
        // Local contexts are separate.
        assert_eq!(
            lookup(Context::SceneTree, &[c("Delete")]).unwrap().0,
            "scene.delete"
        );
        assert!(lookup(Context::Window, &[c("Delete")]).is_none());
        drop(owner);
        set_prefs(&KeymapPrefs::default());
        assert_eq!(fired.get(), 1, "dead listeners are dropped");
    }

    #[test]
    fn key_events_map_to_layout_chords() {
        use gdk::{Key, ModifierType as M};
        let cs = M::CONTROL_MASK | M::SHIFT_MASK;
        assert_eq!(event_chords(Key::Z, 0, cs), vec![c("Ctrl+Shift+Z")]);
        assert_eq!(event_chords(Key::z, 0, M::CONTROL_MASK), vec![c("Ctrl+Z")]);
        // AZERTY: é is a key of its own; Shift+é is É.
        assert_eq!(
            event_chords(Key::eacute, 0, M::CONTROL_MASK),
            vec![c("Ctrl+eacute")]
        );
        assert_eq!(
            event_chords(Key::Eacute, 0, cs),
            vec![c("Ctrl+Shift+eacute")]
        );
        // Shift selecting a symbol is not part of the chord.
        assert_eq!(event_chords(Key::question, 0, cs), vec![c("Ctrl+question")]);
        assert_eq!(event_chords(Key::_1, 0, cs), vec![c("Ctrl+1")]);
        assert_eq!(
            event_chords(Key::ISO_Left_Tab, 0, cs),
            vec![c("Ctrl+Shift+Tab")]
        );
        assert_eq!(
            event_chords(Key::KP_Enter, 0, M::empty()),
            vec![c("Return")]
        );
        // Dead keys and lone modifiers are never chords.
        assert!(event_chords(Key::dead_circumflex, 0, M::empty()).is_empty());
        assert!(event_chords(Key::Shift_L, 0, M::SHIFT_MASK).is_empty());
        assert!(event_chords(Key::ISO_Level3_Shift, 0, M::empty()).is_empty());
    }

    #[test]
    fn text_fields_keep_typing_keys() {
        assert!(types_text(&c("space")));
        assert!(types_text(&c("Shift+a")));
        assert!(types_text(&c("Ctrl+Alt+z")));
        assert!(!types_text(&c("Ctrl+z")));
        assert!(!types_text(&c("Alt+x")));
        assert!(!types_text(&c("F3")));
    }
}
