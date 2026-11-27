//! Keymap data model: key chords, contexts, action specs, the persisted
//! keymap (overrides of a preset's defaults) and conflict detection.
//! Pure logic, no GTK, so it is unit-tested directly.
//!
//! Keys are named by their GDK keyval name (`z`, `eacute`, `comma`,
//! `F2`…), never by keycode: a binding means the key that *types* that
//! symbol on the user's layout (AZERTY `é` is `eacute`).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Version of the keymap section and of exported keymap files.
pub const KEYMAP_VERSION: u32 = 1;
/// Most bindings kept per action.
pub const MAX_BINDINGS: usize = 8;
/// Most overridden actions kept.
pub const MAX_ACTIONS: usize = 1024;
/// Longest action id kept.
pub const MAX_ID_LEN: usize = 128;

/// Modifier set of a chord.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const SHIFT: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    pub const SUPER: Mods = Mods(8);

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn with(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    pub fn without(self, other: Mods) -> Mods {
        Mods(self.0 & !other.0)
    }
}

/// A key with modifiers, e.g. `Ctrl+Shift+Z`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub mods: Mods,
    /// GDK keyval name; single ASCII letters are lowercase.
    pub key: String,
}

/// Modifier-only and layout-switching keys: never bindable on their own.
const MODIFIER_KEYS: [&str; 22] = [
    "Shift_L",
    "Shift_R",
    "Control_L",
    "Control_R",
    "Alt_L",
    "Alt_R",
    "Meta_L",
    "Meta_R",
    "Super_L",
    "Super_R",
    "Hyper_L",
    "Hyper_R",
    "Caps_Lock",
    "Shift_Lock",
    "Num_Lock",
    "Scroll_Lock",
    "Multi_key",
    "Mode_switch",
    "ISO_Level3_Shift",
    "ISO_Level5_Shift",
    "ISO_Group_Shift",
    "ISO_Next_Group",
];

/// Whether `name` is a key a binding may use: not a modifier, not a dead
/// key (those compose accents and must keep typing), a plain keyval name.
pub fn is_bindable_key(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("dead_")
        && !name.starts_with("ISO_Level")
        && !name.starts_with("ISO_Group")
        && !MODIFIER_KEYS.contains(&name)
        && name != "VoidSymbol"
}

/// Friendly spellings accepted when parsing, mapped to keyval names.
fn alias(token: &str) -> Option<&'static str> {
    Some(match token {
        "," => "comma",
        "." => "period",
        "?" => "question",
        "/" => "slash",
        "\\" => "backslash",
        "+" => "plus",
        "-" => "minus",
        "=" => "equal",
        ";" => "semicolon",
        ":" => "colon",
        "!" => "exclam",
        "*" => "asterisk",
        "^" => "asciicircum",
        "`" => "grave",
        "'" => "apostrophe",
        "[" => "bracketleft",
        "]" => "bracketright",
        "<" => "less",
        ">" => "greater",
        "&" => "ampersand",
        "é" => "eacute",
        "è" => "egrave",
        "à" => "agrave",
        "ç" => "ccedilla",
        "ù" => "ugrave",
        _ => match token.to_ascii_lowercase().as_str() {
            "enter" | "return" => "Return",
            "esc" | "escape" => "Escape",
            "space" => "space",
            "del" | "delete" => "Delete",
            "backspace" => "BackSpace",
            "tab" => "Tab",
            "home" => "Home",
            "end" => "End",
            "pgup" | "pageup" | "page_up" => "Page_Up",
            "pgdn" | "pagedown" | "page_down" => "Page_Down",
            "up" => "Up",
            "down" => "Down",
            "left" => "Left",
            "right" => "Right",
            "insert" | "ins" => "Insert",
            _ => return None,
        },
    })
}

/// Keypad and other keys folded onto their main-keyboard twin, so one
/// binding covers both.
fn fold_key(name: &str) -> &str {
    match name {
        "KP_Enter" => "Return",
        "KP_Delete" => "Delete",
        "KP_Home" => "Home",
        "KP_End" => "End",
        "KP_Up" => "Up",
        "KP_Down" => "Down",
        "KP_Left" => "Left",
        "KP_Right" => "Right",
        "KP_Page_Up" => "Page_Up",
        "KP_Page_Down" => "Page_Down",
        "KP_Insert" => "Insert",
        "KP_Space" => "space",
        "KP_Tab" => "Tab",
        other => other,
    }
}

impl Chord {
    /// A chord from a modifier set and a key name, normalized: letters
    /// lowercase, keypad twins folded, `ISO_Left_Tab` → `Shift+Tab`.
    /// `None` for keys that cannot be bound (modifiers, dead keys).
    pub fn new(mods: Mods, key: &str) -> Option<Chord> {
        let (mods, key) = if key == "ISO_Left_Tab" {
            (mods.with(Mods::SHIFT), "Tab")
        } else {
            (mods, fold_key(key))
        };
        let key = if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() {
            key.to_ascii_lowercase()
        } else {
            key.to_owned()
        };
        is_bindable_key(&key).then_some(Chord { mods, key })
    }

    /// The chord of a key event, given the keyval name (already
    /// lowercased by the caller for cased letters) and whether the key
    /// is a printable symbol that is not a letter. For such symbols Shift
    /// only selects the symbol (AZERTY `1` is Shift+`&`, US `?` is
    /// Shift+`/`), so it is dropped: the chord is what the key typed.
    pub fn from_event(name: &str, mods: Mods, printable_symbol: bool) -> Option<Chord> {
        let mods = if printable_symbol {
            mods.without(Mods::SHIFT)
        } else {
            mods
        };
        Chord::new(mods, name)
    }

    /// Parses `Ctrl+Shift+Z`, `ctrl+eacute`, `F2` or GTK's `<Control>z`.
    pub fn parse(text: &str) -> Result<Chord, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("empty key binding".to_owned());
        }
        let mut mods = Mods::NONE;
        let key = if text.starts_with('<') {
            let mut rest = text;
            while let Some(after) = rest.strip_prefix('<') {
                let end = after
                    .find('>')
                    .ok_or_else(|| format!("unclosed modifier in `{text}`"))?;
                mods =
                    mods.with(modifier(&after[..end]).ok_or_else(|| {
                        format!("unknown modifier `{}` in `{text}`", &after[..end])
                    })?);
                rest = &after[end + 1..];
            }
            rest.to_owned()
        } else {
            // A trailing "+" is the plus key ("Ctrl++").
            let (body, plus) = match text.strip_suffix("++") {
                Some(b) => (b, true),
                None => (text, text == "+"),
            };
            let mut parts: Vec<&str> = if plus && body == "+" {
                Vec::new()
            } else {
                body.split('+').map(str::trim).collect()
            };
            let key = if plus {
                "+".to_owned()
            } else {
                parts.pop().unwrap_or_default().to_owned()
            };
            for part in parts {
                mods = mods.with(
                    modifier(part)
                        .ok_or_else(|| format!("unknown modifier `{part}` in `{text}`"))?,
                );
            }
            key
        };
        let key = key.trim();
        let name = alias(key).unwrap_or(key);
        Chord::new(mods, name).ok_or_else(|| format!("`{key}` cannot be bound"))
    }

    /// GTK accelerator string, e.g. `<Control><Shift>z`.
    pub fn to_accel(&self) -> String {
        let mut s = String::new();
        for (m, tag) in [
            (Mods::CTRL, "<Control>"),
            (Mods::SHIFT, "<Shift>"),
            (Mods::ALT, "<Alt>"),
            (Mods::SUPER, "<Super>"),
        ] {
            if self.mods.contains(m) {
                s.push_str(tag);
            }
        }
        s.push_str(&self.key);
        s
    }

    /// Human-readable label, e.g. `Ctrl+Shift+Z`, `Ctrl+É`, `Ctrl+,`.
    pub fn label(&self) -> String {
        let mut s = self.mods_prefix();
        s.push_str(&key_label(&self.key));
        s
    }

    fn mods_prefix(&self) -> String {
        let mut s = String::new();
        for (m, name) in [
            (Mods::CTRL, "Ctrl+"),
            (Mods::SHIFT, "Shift+"),
            (Mods::ALT, "Alt+"),
            (Mods::SUPER, "Super+"),
        ] {
            if self.mods.contains(m) {
                s.push_str(name);
            }
        }
        s
    }

    /// Whether the chord has a command modifier (Ctrl, Alt or Super).
    pub fn has_command_mod(&self) -> bool {
        self.mods.contains(Mods::CTRL)
            || self.mods.contains(Mods::ALT)
            || self.mods.contains(Mods::SUPER)
    }
}

/// Stored form: modifiers plus the keyval name, letters uppercase for
/// readability (`Ctrl+Shift+Z`, `Ctrl+eacute`). Parses back to itself.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = if self.key.len() == 1 {
            self.key.to_ascii_uppercase()
        } else {
            self.key.clone()
        };
        write!(f, "{}{key}", self.mods_prefix())
    }
}

fn modifier(token: &str) -> Option<Mods> {
    match token.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "primary" | "ctl" => Some(Mods::CTRL),
        "shift" => Some(Mods::SHIFT),
        "alt" | "mod1" => Some(Mods::ALT),
        "super" | "meta" | "cmd" | "win" => Some(Mods::SUPER),
        _ => None,
    }
}

/// Display label of a key name.
pub fn key_label(name: &str) -> String {
    let fixed = match name {
        "space" => "Space",
        "Return" => "Enter",
        "Escape" => "Esc",
        "BackSpace" => "Backspace",
        "Delete" => "Del",
        "Page_Up" => "PgUp",
        "Page_Down" => "PgDn",
        "comma" => ",",
        "period" => ".",
        "question" => "?",
        "slash" => "/",
        "backslash" => "\\",
        "plus" => "+",
        "minus" => "-",
        "equal" => "=",
        "semicolon" => ";",
        "colon" => ":",
        "exclam" => "!",
        "asterisk" => "*",
        "asciicircum" => "^",
        "grave" => "`",
        "apostrophe" => "'",
        "bracketleft" => "[",
        "bracketright" => "]",
        "less" => "<",
        "greater" => ">",
        "ampersand" => "&",
        "quotedbl" => "\"",
        "parenleft" => "(",
        "parenright" => ")",
        "eacute" => "É",
        "egrave" => "È",
        "agrave" => "À",
        "ccedilla" => "Ç",
        "ugrave" => "Ù",
        "section" => "§",
        "twosuperior" => "²",
        "dollar" => "$",
        "underscore" => "_",
        "Up" => "↑",
        "Down" => "↓",
        "Left" => "←",
        "Right" => "→",
        _ => "",
    };
    if !fixed.is_empty() {
        fixed.to_owned()
    } else if name.len() == 1 {
        name.to_ascii_uppercase()
    } else {
        name.replace('_', " ")
    }
}

/// Where a binding is active. Bindings conflict only within one context
/// (or two contexts active at once, see [`Context::overlaps`]); a local
/// context wins over Window while its editor has focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    Window,
    Viewport,
    SceneTree,
    Console,
    TextEditing,
}

impl Context {
    pub const ALL: [Context; 5] = [
        Context::Window,
        Context::Viewport,
        Context::SceneTree,
        Context::Console,
        Context::TextEditing,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Context::Window => "window",
            Context::Viewport => "viewport",
            Context::SceneTree => "scene-tree",
            Context::Console => "console",
            Context::TextEditing => "text-editing",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Context::Window => "Window",
            Context::Viewport => "Viewport",
            Context::SceneTree => "Scene Tree",
            Context::Console => "Console",
            Context::TextEditing => "Text Editing",
        }
    }

    /// Whether bindings of the two contexts can compete for the same key
    /// press: the same context; Window and any context (a Window key is
    /// shadowed where a local one is active); text editing and the
    /// contexts that accept text (the console).
    pub fn overlaps(self, other: Context) -> bool {
        self == other
            || self == Context::Window
            || other == Context::Window
            || matches!(
                (self, other),
                (Context::Console, Context::TextEditing) | (Context::TextEditing, Context::Console)
            )
    }

    /// Whether a binding of `self` deliberately wins over the same key in
    /// `other`: text-editing keys override Window keys while typing (e.g.
    /// Ctrl+Space completes instead of maximizing the area). Such pairs
    /// are overrides, not conflicts.
    pub fn shadows(self, other: Context) -> bool {
        self == Context::TextEditing && other == Context::Window
    }
}

/// Keymap preset: the set of default bindings overrides apply to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Viscas,
    Blender,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Viscas, Preset::Blender];

    pub fn id(self) -> &'static str {
        match self {
            Preset::Viscas => "viscas",
            Preset::Blender => "blender",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Preset::Viscas => "Viscas Default",
            Preset::Blender => "Blender-like",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == id)
    }
}

/// One bindable action.
#[derive(Clone, Debug)]
pub struct ActionSpec {
    /// Command id (`win.save`, `scene.delete`…). Unique.
    pub id: String,
    pub title: String,
    /// Group shown in the keymap editor.
    pub category: String,
    pub context: Context,
    /// Default bindings of the Viscas preset.
    pub defaults: Vec<Chord>,
    /// Default bindings of the Blender-like preset, when they differ.
    pub blender: Option<Vec<Chord>>,
    /// Whether holding the key repeats the action.
    pub repeat: bool,
}

impl ActionSpec {
    /// A spec with defaults parsed from `keys` (invalid ones dropped).
    pub fn new(id: &str, title: &str, category: &str, context: Context, keys: &[&str]) -> Self {
        ActionSpec {
            id: id.to_owned(),
            title: title.to_owned(),
            category: category.to_owned(),
            context,
            defaults: parse_all(keys),
            blender: None,
            repeat: true,
        }
    }

    pub fn blender(mut self, keys: &[&str]) -> Self {
        self.blender = Some(parse_all(keys));
        self
    }

    pub fn no_repeat(mut self) -> Self {
        self.repeat = false;
        self
    }

    pub fn defaults_for(&self, preset: Preset) -> &[Chord] {
        match (preset, &self.blender) {
            (Preset::Blender, Some(b)) => b,
            _ => &self.defaults,
        }
    }
}

fn parse_all(keys: &[&str]) -> Vec<Chord> {
    keys.iter().filter_map(|k| Chord::parse(k).ok()).collect()
}

/// The persisted keymap: a preset plus per-action overrides (only the
/// actions whose bindings differ from the preset, like Blender's keymap
/// diffs). Ids no registered action knows are kept but ignored, so a
/// keymap survives a build that lacks some command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapPrefs {
    pub version: u32,
    /// Preset id (`viscas`, `blender`).
    pub preset: String,
    /// Action id → bindings in stored form. An empty list unbinds.
    pub bindings: BTreeMap<String, Vec<String>>,
}

impl Default for KeymapPrefs {
    fn default() -> Self {
        Self {
            version: KEYMAP_VERSION,
            preset: Preset::Viscas.id().to_owned(),
            bindings: BTreeMap::new(),
        }
    }
}

/// Two actions of overlapping contexts bound to the same chord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub chord: Chord,
    pub ids: Vec<String>,
}

/// Id prefixes of per-item palette commands (one per node, function or
/// setting) that are never bindable.
pub const UNBINDABLE_PREFIXES: [&str; 3] = ["node.goto::", "function.insert::", "setting::"];

/// Whether `id` names a per-item command that cannot be bound.
pub fn is_unbindable_id(id: &str) -> bool {
    UNBINDABLE_PREFIXES.iter().any(|p| id.starts_with(p))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && !id.chars().any(char::is_control)
}

impl KeymapPrefs {
    pub fn preset(&self) -> Preset {
        Preset::from_id(&self.preset).unwrap_or(Preset::Viscas)
    }

    /// Repairs the keymap: known preset, valid ids, parseable bindings in
    /// canonical form, no duplicates, bounded sizes.
    pub fn validate(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        self.version = KEYMAP_VERSION;
        if Preset::from_id(&self.preset).is_none() {
            warnings.push(format!("unknown keymap preset `{}`", self.preset));
            self.preset = Preset::Viscas.id().to_owned();
        }
        let old = std::mem::take(&mut self.bindings);
        for (id, keys) in old {
            if !valid_id(&id) {
                warnings.push("ignoring a keymap entry with an invalid id".to_owned());
                continue;
            }
            if is_unbindable_id(&id) {
                warnings.push(format!("ignoring `{id}`: this command cannot be bound"));
                continue;
            }
            if self.bindings.len() >= MAX_ACTIONS {
                warnings.push("too many keymap entries; the rest are ignored".to_owned());
                break;
            }
            let mut chords: Vec<Chord> = Vec::new();
            for k in keys {
                match Chord::parse(&k) {
                    Ok(c) if !chords.contains(&c) => chords.push(c),
                    Ok(_) => {}
                    Err(e) => warnings.push(format!("keymap `{id}`: {e}")),
                }
            }
            chords.truncate(MAX_BINDINGS);
            self.bindings
                .insert(id, chords.iter().map(Chord::to_string).collect());
        }
        warnings
    }

    /// Current bindings of `spec`: its override, else the preset default.
    pub fn bindings_of(&self, spec: &ActionSpec) -> Vec<Chord> {
        match self.bindings.get(&spec.id) {
            Some(keys) => keys.iter().filter_map(|k| Chord::parse(k).ok()).collect(),
            None => spec.defaults_for(self.preset()).to_vec(),
        }
    }

    /// Whether `spec` has an override that differs from its default.
    pub fn is_overridden(&self, spec: &ActionSpec) -> bool {
        self.bindings
            .get(&spec.id)
            .is_some_and(|_| self.bindings_of(spec) != spec.defaults_for(self.preset()))
    }

    /// Sets `spec`'s bindings; an override equal to the default is
    /// dropped.
    pub fn set(&mut self, spec: &ActionSpec, chords: Vec<Chord>) {
        let mut unique: Vec<Chord> = Vec::new();
        for c in chords {
            if !unique.contains(&c) {
                unique.push(c);
            }
        }
        unique.truncate(MAX_BINDINGS);
        if unique == spec.defaults_for(self.preset()) {
            self.bindings.remove(&spec.id);
        } else {
            self.bindings.insert(
                spec.id.clone(),
                unique.iter().map(Chord::to_string).collect(),
            );
        }
    }

    /// Replaces binding `index` of `spec` (or appends when `index` is
    /// past the end) with `chord`; `None` removes it.
    pub fn assign(&mut self, spec: &ActionSpec, index: Option<usize>, chord: Option<Chord>) {
        let mut list = self.bindings_of(spec);
        match (index.filter(|i| *i < list.len()), chord) {
            (Some(i), Some(c)) => list[i] = c,
            (Some(i), None) => {
                list.remove(i);
            }
            (None, Some(c)) => list.push(c),
            (None, None) => {}
        }
        self.set(spec, list);
    }

    pub fn reset(&mut self, id: &str) {
        self.bindings.remove(id);
    }

    /// Resets every known action; unknown ids are kept.
    pub fn reset_all(&mut self, specs: &[ActionSpec]) {
        for s in specs {
            self.bindings.remove(&s.id);
        }
    }

    /// Switches preset, dropping the overrides of known actions.
    pub fn set_preset(&mut self, preset: Preset, specs: &[ActionSpec]) {
        self.preset = preset.id().to_owned();
        self.reset_all(specs);
    }

    /// Ids of the actions other than `id` whose context overlaps
    /// `context` and that are bound to `chord`.
    pub fn conflicts_with(
        &self,
        specs: &[ActionSpec],
        id: &str,
        context: Context,
        chord: &Chord,
    ) -> Vec<String> {
        specs
            .iter()
            .filter(|s| s.id != id && s.context.overlaps(context))
            .filter(|s| !s.context.shadows(context) && !context.shadows(s.context))
            .filter(|s| self.bindings_of(s).contains(chord))
            .map(|s| s.id.clone())
            .collect()
    }

    /// Ids of the actions bound to `chord` that `id` (of `context`)
    /// overrides, or that override it ([`Context::shadows`]): intentional,
    /// shown as a note rather than a conflict.
    pub fn overrides_with(
        &self,
        specs: &[ActionSpec],
        id: &str,
        context: Context,
        chord: &Chord,
    ) -> Vec<String> {
        specs
            .iter()
            .filter(|s| s.id != id && (s.context.shadows(context) || context.shadows(s.context)))
            .filter(|s| self.bindings_of(s).contains(chord))
            .map(|s| s.id.clone())
            .collect()
    }

    /// Every chord bound more than once within overlapping contexts.
    pub fn conflicts(&self, specs: &[ActionSpec]) -> Vec<Conflict> {
        let mut out: Vec<Conflict> = Vec::new();
        for s in specs {
            for chord in self.bindings_of(s) {
                if out
                    .iter()
                    .any(|c| c.chord == chord && c.ids.contains(&s.id))
                {
                    continue;
                }
                let others = self.conflicts_with(specs, &s.id, s.context, &chord);
                if !others.is_empty() {
                    let mut ids = vec![s.id.clone()];
                    ids.extend(others);
                    out.push(Conflict { chord, ids });
                }
            }
        }
        out
    }

    /// Resolves a conflict on `chord` in favour of `id`: removes `chord`
    /// from every conflicting action.
    pub fn replace_conflict(&mut self, specs: &[ActionSpec], id: &str, chord: &Chord) {
        let Some(spec) = specs.iter().find(|s| s.id == id) else {
            return;
        };
        for other in self.conflicts_with(specs, id, spec.context, chord) {
            if let Some(o) = specs.iter().find(|s| s.id == other) {
                let list = self.bindings_of(o).into_iter().filter(|c| c != chord);
                self.set(o, list.collect());
            }
        }
    }

    /// Resolves a conflict on `chord` by swapping: every conflicting
    /// action gets `previous` (the chord `id` had before) in its place,
    /// or loses `chord` when there was none.
    pub fn swap_conflict(
        &mut self,
        specs: &[ActionSpec],
        id: &str,
        chord: &Chord,
        previous: Option<&Chord>,
    ) {
        let Some(spec) = specs.iter().find(|s| s.id == id) else {
            return;
        };
        for other in self.conflicts_with(specs, id, spec.context, chord) {
            if let Some(o) = specs.iter().find(|s| s.id == other) {
                let mut list: Vec<Chord> = Vec::new();
                for c in self.bindings_of(o) {
                    if &c == chord {
                        if let Some(p) = previous {
                            list.push(p.clone());
                        }
                    } else {
                        list.push(c);
                    }
                }
                self.set(o, list);
            }
        }
    }

    /// Serializes to a standalone keymap file.
    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Parses a keymap file; invalid entries become warnings.
    pub fn from_toml(text: &str) -> Result<(KeymapPrefs, Vec<String>), String> {
        let mut km: KeymapPrefs = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut warnings = Vec::new();
        if km.version > KEYMAP_VERSION {
            warnings.push(format!(
                "keymap version {} is newer than {KEYMAP_VERSION}",
                km.version
            ));
        }
        warnings.extend(km.validate());
        Ok((km, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    fn specs() -> Vec<ActionSpec> {
        vec![
            ActionSpec::new("win.save", "Save", "File", Context::Window, &["Ctrl+S"]),
            ActionSpec::new("win.undo", "Undo", "Edit", Context::Window, &["Ctrl+Z"]),
            ActionSpec::new(
                "win.palette",
                "Palette",
                "General",
                Context::Window,
                &["Ctrl+P", "Space"],
            )
            .blender(&["F3"]),
            ActionSpec::new(
                "scene.delete",
                "Delete",
                "Scene",
                Context::SceneTree,
                &["Delete"],
            ),
            ActionSpec::new(
                "viewport.reset",
                "Reset",
                "View",
                Context::Viewport,
                &["Home"],
            ),
            ActionSpec::new(
                "console.clear",
                "Clear",
                "Console",
                Context::Console,
                &["Ctrl+L"],
            ),
            ActionSpec::new("text.x", "X", "Text", Context::TextEditing, &[]),
        ]
    }

    #[test]
    fn parses_and_formats_chords() {
        let z = c("Ctrl+Shift+Z");
        assert_eq!(z.mods, Mods::CTRL.with(Mods::SHIFT));
        assert_eq!(z.key, "z");
        assert_eq!(z.to_string(), "Ctrl+Shift+Z");
        assert_eq!(z.label(), "Ctrl+Shift+Z");
        assert_eq!(z.to_accel(), "<Control><Shift>z");
        assert_eq!(c("<Control><Shift>z"), z);
        assert_eq!(c("<Primary>comma"), c("ctrl+,"));
        assert_eq!(c("Ctrl+comma").label(), "Ctrl+,");
        assert_eq!(c("F2").to_string(), "F2");
        assert_eq!(c("Ctrl++").key, "plus");
        assert_eq!(c("+").key, "plus");
        assert_eq!(c("Ctrl+Enter").key, "Return");
        assert_eq!(c("Ctrl+KP_Enter"), c("Ctrl+Return"));
        assert_eq!(c("<Control>ISO_Left_Tab"), c("Ctrl+Shift+Tab"));
        assert_eq!(c("Space").label(), "Space");
        assert!(Chord::parse("").is_err());
        assert!(Chord::parse("Hyper+x").is_err());
        assert!(Chord::parse("Ctrl+").is_err());
        assert!(Chord::parse("Ctrl+not a key").is_err());
        assert!(Chord::parse("<Control z").is_err());
    }

    #[test]
    fn azerty_keyvals_round_trip_and_dead_keys_are_excluded() {
        let e = c("Ctrl+eacute");
        assert_eq!(e.to_string(), "Ctrl+eacute");
        assert_eq!(e.label(), "Ctrl+É");
        assert_eq!(Chord::parse(&e.to_string()).unwrap(), e);
        assert_eq!(c("Ctrl+é"), e);
        assert_eq!(c("Alt+ugrave").label(), "Alt+Ù");
        assert_eq!(c("Ctrl+twosuperior").label(), "Ctrl+²");
        for dead in [
            "dead_circumflex",
            "dead_diaeresis",
            "Shift_L",
            "ISO_Level3_Shift",
        ] {
            assert!(!is_bindable_key(dead), "{dead}");
            assert!(Chord::parse(&format!("Ctrl+{dead}")).is_err(), "{dead}");
            assert!(Chord::from_event(dead, Mods::NONE, false).is_none());
        }
    }

    #[test]
    fn event_chords_drop_shift_only_for_symbols() {
        let ctrl_shift = Mods::CTRL.with(Mods::SHIFT);
        // US `?` is Shift+`/`; AZERTY `1` is Shift+`&`.
        assert_eq!(
            Chord::from_event("question", ctrl_shift, true),
            Some(c("Ctrl+question"))
        );
        assert_eq!(Chord::from_event("1", ctrl_shift, true), Some(c("Ctrl+1")));
        // Letters and named keys keep Shift.
        assert_eq!(
            Chord::from_event("z", ctrl_shift, false),
            Some(c("Ctrl+Shift+Z"))
        );
        assert_eq!(
            Chord::from_event("Return", ctrl_shift, false),
            Some(c("Ctrl+Shift+Return"))
        );
        assert_eq!(
            Chord::from_event("ISO_Left_Tab", ctrl_shift, false),
            Some(c("Ctrl+Shift+Tab"))
        );
        assert_eq!(
            Chord::from_event("eacute", Mods::CTRL, false),
            Some(c("Ctrl+eacute"))
        );
    }

    #[test]
    fn bindings_resolve_from_preset_and_overrides() {
        let s = specs();
        let mut km = KeymapPrefs::default();
        assert_eq!(km.bindings_of(&s[2]), vec![c("Ctrl+P"), c("Space")]);
        km.set_preset(Preset::Blender, &s);
        assert_eq!(km.bindings_of(&s[2]), vec![c("F3")]);
        assert_eq!(km.bindings_of(&s[0]), vec![c("Ctrl+S")]);
        km.assign(&s[0], None, Some(c("F5")));
        assert_eq!(km.bindings_of(&s[0]), vec![c("Ctrl+S"), c("F5")]);
        assert!(km.is_overridden(&s[0]));
        km.assign(&s[0], Some(1), None);
        // Back to the default: no override kept.
        assert!(!km.bindings.contains_key("win.save"));
        km.assign(&s[0], Some(0), None);
        assert_eq!(km.bindings["win.save"], Vec::<String>::new());
        assert!(km.bindings_of(&s[0]).is_empty());
        km.reset("win.save");
        assert_eq!(km.bindings_of(&s[0]), vec![c("Ctrl+S")]);
        km.assign(&s[1], Some(0), Some(c("Ctrl+Y")));
        km.bindings.insert("future.cmd".into(), vec!["F9".into()]);
        km.reset_all(&s);
        assert_eq!(km.bindings.len(), 1, "unknown ids survive Reset All");
    }

    #[test]
    fn conflicts_are_found_within_overlapping_contexts_only() {
        let s = specs();
        let mut km = KeymapPrefs::default();
        assert!(km.conflicts(&s).is_empty());
        // Same chord in two unrelated local contexts: no conflict.
        km.assign(&s[3], Some(0), Some(c("Home")));
        assert!(km.conflicts(&s).is_empty());
        // A local key shadows a Window key: conflict.
        km.assign(&s[4], None, Some(c("Ctrl+P")));
        assert_eq!(
            km.conflicts_with(&s, "viewport.reset", Context::Viewport, &c("Ctrl+P")),
            vec!["win.palette".to_owned()]
        );
        km.reset("viewport.reset");
        // Same context: conflict.
        km.assign(&s[1], Some(0), Some(c("Ctrl+S")));
        let found = km.conflicts(&s);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].chord, c("Ctrl+S"));
        assert_eq!(
            found[0].ids,
            vec!["win.save".to_owned(), "win.undo".to_owned()]
        );
        // A text-editing key over a Window key is an override.
        km.assign(&s[6], None, Some(c("Ctrl+P")));
        assert!(
            km.conflicts_with(&s, "text.x", Context::TextEditing, &c("Ctrl+P"))
                .is_empty()
        );
        assert_eq!(
            km.overrides_with(&s, "text.x", Context::TextEditing, &c("Ctrl+P")),
            vec!["win.palette".to_owned()]
        );
        assert_eq!(
            km.overrides_with(&s, "win.palette", Context::Window, &c("Ctrl+P")),
            vec!["text.x".to_owned()]
        );
        km.reset("text.x");
        // Console and text editing are active together.
        km.assign(&s[6], None, Some(c("Ctrl+L")));
        assert_eq!(
            km.conflicts_with(&s, "text.x", Context::TextEditing, &c("Ctrl+L")),
            vec!["console.clear".to_owned()]
        );
    }

    #[test]
    fn replace_and_swap_resolve_conflicts() {
        let s = specs();
        let mut km = KeymapPrefs::default();
        // Undo takes Ctrl+S (it had Ctrl+Z): swap gives Save Ctrl+Z.
        km.assign(&s[1], Some(0), Some(c("Ctrl+S")));
        let mut swapped = km.clone();
        swapped.swap_conflict(&s, "win.undo", &c("Ctrl+S"), Some(&c("Ctrl+Z")));
        assert_eq!(swapped.bindings_of(&s[0]), vec![c("Ctrl+Z")]);
        assert!(swapped.conflicts(&s).is_empty());
        km.replace_conflict(&s, "win.undo", &c("Ctrl+S"));
        assert!(km.bindings_of(&s[0]).is_empty());
        assert_eq!(km.bindings_of(&s[1]), vec![c("Ctrl+S")]);
    }

    #[test]
    fn validation_and_toml_round_trip() {
        let text = r#"
            version = 1
            preset = "blender"
            [bindings]
            "win.save" = ["ctrl+s", "Ctrl+S", "F5", "Ctrl+dead_acute", "Bogus+x"]
            "never.heard.of" = ["F9"]
            "node.goto::3" = ["F8"]
        "#;
        let (km, warnings) = KeymapPrefs::from_toml(text).unwrap();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(!km.bindings.contains_key("node.goto::3"));
        assert_eq!(km.preset(), Preset::Blender);
        assert_eq!(km.bindings["win.save"], vec!["Ctrl+S", "F5"]);
        assert_eq!(km.bindings["never.heard.of"], vec!["F9"]);
        let again = KeymapPrefs::from_toml(&km.to_toml().unwrap()).unwrap();
        assert_eq!(again.0, km);
        assert!(again.1.is_empty());

        let (km, w) = KeymapPrefs::from_toml("preset = \"emacs\"").unwrap();
        assert_eq!(km.preset(), Preset::Viscas);
        assert_eq!(w.len(), 1);
        assert!(KeymapPrefs::from_toml("bindings = 3").is_err());
    }
}
