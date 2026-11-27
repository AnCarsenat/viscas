//! User preferences: data model, validation and TOML persistence in
//! `$XDG_CONFIG_HOME/viscas/preferences.toml`. Pure logic, no GTK.
//!
//! Loading never fails: unreadable or invalid files yield defaults plus
//! warnings for the caller to log. Missing fields take their default.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::keymap::KeymapPrefs;
use crate::theme::model::{self, Accent, Theme};

/// Version written to new files. Bump on incompatible changes.
pub const VERSION: u32 = 1;

/// Range, step and default of a numeric preference, shared by validation
/// and the preference widgets.
#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub digits: u32,
    pub default: f64,
}

impl Range {
    /// `v` clamped into range; non-finite values become the default.
    pub fn clamp(&self, v: f64) -> f64 {
        if v.is_finite() {
            v.clamp(self.min, self.max)
        } else {
            self.default
        }
    }

    fn clamp_u32(&self, v: u32) -> u32 {
        self.clamp(f64::from(v)).round() as u32
    }
}

pub const UI_SCALE: Range = Range {
    min: 0.5,
    max: 2.0,
    step: 0.05,
    digits: 2,
    default: 1.0,
};
pub const FONT_SIZE: Range = Range {
    min: 8.0,
    max: 20.0,
    step: 0.5,
    digits: 1,
    default: 12.0,
};
pub const LINE_WIDTH: Range = Range {
    min: 0.5,
    max: 4.0,
    step: 0.25,
    digits: 2,
    default: 1.0,
};
pub const UNDO_STEPS: Range = Range {
    min: 1.0,
    max: 1024.0,
    step: 1.0,
    digits: 0,
    default: 64.0,
};
pub const AUTOSAVE_MINUTES: Range = Range {
    min: 0.0,
    max: 120.0,
    step: 1.0,
    digits: 0,
    default: 2.0,
};
pub const ZOOM_SENSITIVITY: Range = Range {
    min: 0.1,
    max: 4.0,
    step: 0.05,
    digits: 2,
    default: 1.0,
};
pub const RECENT_FILES: Range = Range {
    min: 0.0,
    max: 50.0,
    step: 1.0,
    digits: 0,
    default: 10.0,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interface {
    /// Multiplies every UI length (Blender's "Resolution Scale").
    #[serde(deserialize_with = "lenient::f64")]
    pub ui_scale: f64,
    /// Base font size in px.
    #[serde(deserialize_with = "lenient::f64")]
    pub font_size: f64,
    /// Preferred monospace family; empty for the built-in list.
    pub mono_font: String,
    pub tooltips: bool,
    /// Playful one-liners in empty states.
    pub playful: bool,
    /// How GTK rasterizes text.
    pub font_rendering: FontRendering,
    /// Show the Data/Script workspaces and the editors that are not built
    /// yet (Sheet, Script, Dependency Graph).
    pub show_unfinished: bool,
}

impl Default for Interface {
    fn default() -> Self {
        Self {
            ui_scale: UI_SCALE.default,
            font_size: FONT_SIZE.default,
            mono_font: String::new(),
            tooltips: true,
            playful: true,
            font_rendering: FontRendering::default(),
            show_unfinished: false,
        }
    }
}

/// Text rasterization mode (GTK's `gtk-font-rendering`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontRendering {
    /// The desktop's hinting settings (GTK `manual`). The default: GTK's
    /// unhinted automatic mode draws a small `=` as a single bar on 1×
    /// displays, indistinguishable from `-`.
    #[default]
    DesktopHinting,
    /// GTK's own choice (unhinted, subpixel-positioned glyphs).
    Automatic,
}

impl FontRendering {
    pub const ALL: [FontRendering; 2] = [FontRendering::DesktopHinting, FontRendering::Automatic];

    pub fn title(self) -> &'static str {
        match self {
            FontRendering::DesktopHinting => "Desktop Hinting",
            FontRendering::Automatic => "Automatic",
        }
    }

    /// Nick of the matching `GtkFontRendering` value.
    pub fn gtk_nick(self) -> &'static str {
        match self {
            FontRendering::DesktopHinting => "manual",
            FontRendering::Automatic => "automatic",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Themes {
    /// Name of the preset the active theme started from.
    pub preset: String,
    pub accent: Accent,
}

impl Default for Themes {
    fn default() -> Self {
        Self {
            preset: model::DARK_NAME.to_owned(),
            accent: Accent::Theme,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GridStyle {
    #[default]
    Lines,
    Dots,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Viewport {
    pub grid: bool,
    pub minor_grid: bool,
    pub axis_labels: bool,
    /// Width of grid lines and axes, in logical px.
    #[serde(deserialize_with = "lenient::f64")]
    pub line_width: f64,
    pub antialias: bool,
    pub grid_style: GridStyle,
    /// Equal scales on both axes (box zoom keeps the aspect).
    pub lock_aspect: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            grid: true,
            minor_grid: true,
            axis_labels: true,
            line_width: LINE_WIDTH.default,
            antialias: true,
            grid_style: GridStyle::Lines,
            lock_aspect: true,
        }
    }
}

/// Operator that defines a named node (`a := 3` or `a = 3`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionOp {
    #[default]
    #[serde(rename = ":=")]
    ColonEquals,
    #[serde(rename = "=")]
    Equals,
}

impl DefinitionOp {
    pub const ALL: [DefinitionOp; 2] = [DefinitionOp::ColonEquals, DefinitionOp::Equals];

    pub fn symbol(self) -> &'static str {
        match self {
            DefinitionOp::ColonEquals => ":=",
            DefinitionOp::Equals => "=",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editing {
    /// Stored now; used by the scene's undo stack.
    #[serde(deserialize_with = "lenient::u32")]
    pub undo_steps: u32,
    /// Stored now; the evaluator will pass it to the parser
    /// (`viscas_core` `ParseOptions::definition_op`).
    pub definition_op: DefinitionOp,
    /// The console adds things that draw (`y = x^2`, `(1, 2)`) to the
    /// board as soon as they are entered. Off by default: typing never
    /// pins anything; "Add to board" (Ctrl+Shift+Enter, `:add`) does.
    pub console_add_plots: bool,
    /// The console adds new definitions (`a := 3`) to the board too.
    pub console_add_definitions: bool,
}

impl Default for Editing {
    fn default() -> Self {
        Self {
            undo_steps: UNDO_STEPS.default as u32,
            definition_op: DefinitionOp::default(),
            console_add_plots: false,
            console_add_definitions: false,
        }
    }
}

/// Stored now; used by the document milestone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveLoad {
    /// Minutes between document auto-saves; 0 disables.
    #[serde(deserialize_with = "lenient::u32")]
    pub autosave_minutes: u32,
    #[serde(deserialize_with = "lenient::u32")]
    pub recent_files: u32,
}

impl Default for SaveLoad {
    fn default() -> Self {
        Self {
            autosave_minutes: AUTOSAVE_MINUTES.default as u32,
            recent_files: RECENT_FILES.default as u32,
        }
    }
}

/// Mouse button that pans the viewport by dragging.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanButton {
    /// Left drag pans (Shift+left drag box-zooms); middle drag pans too.
    #[default]
    Left,
    /// Middle drag pans; left drag box-zooms (Blender-like).
    Middle,
}

/// What a left-button drag in the viewport does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    Pan,
    BoxZoom,
    /// Stretch the axes (Ctrl+middle drag).
    Stretch,
}

/// Mouse and trackpad behavior of the viewport camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    /// Scrolling up zooms out instead of in.
    pub invert_zoom: bool,
    /// Multiplies the zoom of one wheel step.
    #[serde(deserialize_with = "lenient::f64")]
    pub zoom_sensitivity: f64,
    pub pan_button: PanButton,
    /// Alt+left drag acts as the middle button (laptops, trackpads).
    pub emulate_three_button: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            invert_zoom: false,
            zoom_sensitivity: ZOOM_SENSITIVITY.default,
            pan_button: PanButton::Left,
            emulate_three_button: false,
        }
    }
}

impl Input {
    /// Zoom exponent of a scroll delta `dy` (positive = down): the view
    /// zooms by `step.powf(exponent)`.
    pub fn zoom_exponent(&self, dy: f64) -> f64 {
        let e = -dy * self.zoom_sensitivity;
        if self.invert_zoom { -e } else { e }
    }

    /// What a left drag started with these modifiers does.
    pub fn left_drag(&self, shift: bool, ctrl: bool, alt: bool) -> DragMode {
        if shift {
            // Shift+drag always box-zooms, emulation or not.
            DragMode::BoxZoom
        } else if self.emulate_three_button && alt {
            // Behaves like the middle button.
            if ctrl {
                DragMode::Stretch
            } else {
                DragMode::Pan
            }
        } else {
            match self.pan_button {
                PanButton::Left => DragMode::Pan,
                PanButton::Middle => DragMode::BoxZoom,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct System {
    /// Switch between `light_theme` and `dark_theme` when the desktop
    /// color scheme changes. Turned off by any manual theme choice.
    pub follow_system: bool,
    /// Theme used for a light desktop while following the system.
    pub light_theme: String,
    /// Theme used for a dark desktop while following the system.
    pub dark_theme: String,
}

impl Default for System {
    fn default() -> Self {
        Self {
            follow_system: false,
            light_theme: model::LIGHT_NAME.to_owned(),
            dark_theme: model::DARK_NAME.to_owned(),
        }
    }
}

/// Longest theme name kept.
pub const MAX_NAME_LEN: usize = 96;
/// Longest monospace family name kept.
pub const MAX_FONT_LEN: usize = 64;
/// Largest preferences or theme file read, in bytes.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Trims and truncates `s` to `max` chars, dropping control characters.
pub fn clean_name(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A font family name restricted to letters, digits, spaces and `-_.`,
/// so it is safe to embed in CSS and Pango descriptions.
pub fn clean_font(s: &str) -> String {
    let kept: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    clean_name(&collapsed, MAX_FONT_LEN)
}

/// Lenient number deserializers: accept integers or floats of any size
/// and saturate, so an out-of-range value is clamped by validation
/// instead of discarding the whole section.
mod lenient {
    use serde::Deserializer;
    use serde::de::{self, Visitor};

    struct Num;

    impl Visitor<'_> for Num {
        type Value = f64;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a number")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
            Ok(v as f64)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
            Ok(v as f64)
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
            Ok(v)
        }
    }

    pub fn f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        d.deserialize_any(Num)
    }

    pub fn u32<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        let v = d.deserialize_any(Num)?;
        Ok(if v.is_nan() {
            0
        } else {
            v.round().clamp(0.0, f64::from(u32::MAX)) as u32
        })
    }
}

/// Every user preference. `theme` holds the active (possibly edited) theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    #[serde(deserialize_with = "lenient::u32")]
    pub version: u32,
    /// Save automatically after every change.
    pub auto_save: bool,
    pub interface: Interface,
    pub themes: Themes,
    pub viewport: Viewport,
    pub editing: Editing,
    pub save_load: SaveLoad,
    pub system: System,
    pub input: Input,
    /// Key bindings: preset and per-action overrides.
    pub keymap: KeymapPrefs,
    /// Last export dialog settings.
    pub export: crate::export::settings::ExportSettings,
    /// Loaded and saved separately so missing colors fall back to the
    /// preset instead of failing the whole file.
    #[serde(skip)]
    pub theme: Theme,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: VERSION,
            auto_save: true,
            interface: Interface::default(),
            themes: Themes::default(),
            viewport: Viewport::default(),
            editing: Editing::default(),
            save_load: SaveLoad::default(),
            system: System::default(),
            input: Input::default(),
            keymap: KeymapPrefs::default(),
            export: Default::default(),
            theme: model::viscas_dark(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        model::viscas_dark()
    }
}

impl Preferences {
    /// Clamps numbers into range and repairs invalid values.
    pub fn validate(&mut self) {
        self.export.validate();
        let i = &mut self.interface;
        i.ui_scale = UI_SCALE.clamp(i.ui_scale);
        i.font_size = FONT_SIZE.clamp(i.font_size);
        i.mono_font = clean_font(&i.mono_font);
        let v = &mut self.viewport;
        v.line_width = LINE_WIDTH.clamp(v.line_width);
        self.editing.undo_steps = UNDO_STEPS.clamp_u32(self.editing.undo_steps);
        let s = &mut self.save_load;
        s.autosave_minutes = AUTOSAVE_MINUTES.clamp_u32(s.autosave_minutes);
        s.recent_files = RECENT_FILES.clamp_u32(s.recent_files);
        self.input.zoom_sensitivity = ZOOM_SENSITIVITY.clamp(self.input.zoom_sensitivity);
        self.keymap.validate();
        self.theme.name = clean_name(&self.theme.name, MAX_NAME_LEN);
        if self.theme.name.is_empty() {
            self.theme.name = "Untitled".to_owned();
        }
        self.themes.preset = clean_name(&self.themes.preset, MAX_NAME_LEN);
        if self.themes.preset.is_empty() {
            self.themes.preset = self.theme.name.clone();
        }
        let sys = &mut self.system;
        sys.light_theme = clean_name(&sys.light_theme, MAX_NAME_LEN);
        sys.dark_theme = clean_name(&sys.dark_theme, MAX_NAME_LEN);
        if sys.light_theme.is_empty() {
            sys.light_theme = model::LIGHT_NAME.to_owned();
        }
        if sys.dark_theme.is_empty() {
            sys.dark_theme = model::DARK_NAME.to_owned();
        }
    }

    /// The theme to draw with: the active theme with the accent applied.
    pub fn effective_theme(&self) -> Theme {
        self.theme.with_accent(self.themes.accent)
    }

    /// Serializes to TOML, the theme as a trailing `[theme]` table.
    pub fn to_toml(&self) -> Result<String, String> {
        let mut table = toml::Table::try_from(self).map_err(|e| e.to_string())?;
        let theme = toml::Table::try_from(&self.theme).map_err(|e| e.to_string())?;
        table.insert("theme".to_owned(), toml::Value::Table(theme));
        toml::to_string_pretty(&table).map_err(|e| e.to_string())
    }

    /// Parses a preferences file. Never fails: problems become warnings
    /// and the affected part falls back to its default.
    pub fn from_toml(text: &str) -> (Preferences, Vec<String>) {
        let mut warnings = Vec::new();
        let mut table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("invalid TOML, using defaults: {e}"));
                return (Preferences::default(), warnings);
            }
        };
        let theme_table = table.remove("theme");
        let mut prefs = match Preferences::deserialize(toml::Value::Table(table.clone())) {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!("{e}"));
                Preferences::salvage(table, &mut warnings)
            }
        };
        if prefs.version > VERSION {
            warnings.push(format!(
                "file version {} is newer than {VERSION}; unknown settings are ignored",
                prefs.version
            ));
        }
        prefs.version = VERSION;
        let base = model::builtin(&prefs.themes.preset).unwrap_or_default();
        prefs.theme = match theme_table {
            None => base,
            Some(toml::Value::Table(t)) => Theme::merge_over(&base, t).unwrap_or_else(|e| {
                warnings.push(format!("invalid [theme], using {}: {e}", base.name));
                base.clone()
            }),
            Some(_) => {
                warnings.push("`theme` is not a table".to_owned());
                base
            }
        };
        warnings.extend(prefs.keymap.validate());
        prefs.validate();
        (prefs, warnings)
    }

    /// Deserializes section by section, dropping the sections that fail.
    fn salvage(table: toml::Table, warnings: &mut Vec<String>) -> Preferences {
        let mut good = toml::Table::new();
        for (key, value) in table {
            let mut probe = toml::Table::new();
            probe.insert(key.clone(), value.clone());
            match Preferences::deserialize(toml::Value::Table(probe)) {
                Ok(_) => {
                    good.insert(key, value);
                }
                Err(e) => warnings.push(format!("ignoring `{key}`: {e}")),
            }
        }
        Preferences::deserialize(toml::Value::Table(good)).unwrap_or_default()
    }
}

/// `$XDG_CONFIG_HOME/viscas`, or `$HOME/.config/viscas`.
pub fn config_dir_from(xdg: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match (xdg.filter(|x| Path::new(x).is_absolute()), home) {
        (Some(xdg), _) => Some(PathBuf::from(xdg).join("viscas")),
        (None, Some(home)) if !home.is_empty() => {
            Some(PathBuf::from(home).join(".config").join("viscas"))
        }
        _ => None,
    }
}

/// The Viscas config directory for the current environment.
/// `$VISCAS_CONFIG_DIR` overrides it (tests, screenshots, portable use).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VISCAS_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let home = std::env::var("HOME").ok();
    config_dir_from(xdg.as_deref(), home.as_deref())
}

pub fn preferences_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("preferences.toml"))
}

/// Directory of user themes (`*.toml`).
pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

/// Reads a text file of at most [`MAX_FILE_BYTES`].
pub fn read_capped(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("larger than {MAX_FILE_BYTES} bytes"),
        ));
    }
    Ok(text)
}

/// Result of loading the preferences file.
pub struct Loaded {
    pub prefs: Preferences,
    pub warnings: Vec<String>,
    /// Part of an existing file was discarded (the file should be backed
    /// up before it is overwritten).
    pub lossy: bool,
}

/// Loads preferences from `path`; a missing file silently gives defaults.
pub fn load(path: &Path) -> Loaded {
    match read_capped(path) {
        Ok(text) => {
            let (prefs, warnings) = Preferences::from_toml(&text);
            let lossy = !warnings.is_empty();
            let warnings = warnings
                .into_iter()
                .map(|w| format!("{}: {w}", path.display()))
                .collect();
            Loaded {
                prefs,
                warnings,
                lossy,
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded {
            prefs: Preferences::default(),
            warnings: vec![],
            lossy: false,
        },
        Err(e) => Loaded {
            prefs: Preferences::default(),
            warnings: vec![format!("cannot read {}: {e}", path.display())],
            lossy: path.exists(),
        },
    }
}

/// Copies `path` to `<path>.bak`, or `.bak.N` with the first free N, so
/// it is never overwritten. Returns the backup path.
pub fn backup(path: &Path) -> std::io::Result<PathBuf> {
    let base = path.as_os_str().to_owned();
    let candidate = |n: u32| {
        let mut name = base.clone();
        if n == 0 {
            name.push(".bak");
        } else {
            name.push(format!(".bak.{n}"));
        }
        PathBuf::from(name)
    };
    // Launching again with the same damaged file reuses its backup.
    let original = std::fs::read(path)?;
    if let Some(same) = (0..1000)
        .map(candidate)
        .take_while(|p| p.exists())
        .find(|p| std::fs::read(p).is_ok_and(|b| b == original))
    {
        return Ok(same);
    }
    let target = (0..1000)
        .map(candidate)
        .find(|p| !p.exists())
        .ok_or_else(|| std::io::Error::other("too many backups"))?;
    std::fs::copy(path, &target)?;
    Ok(target)
}

/// Writes `text` to `path` atomically: a uniquely named temporary file in
/// the same directory is written and synced, then renamed over the
/// target and the directory synced. A symlinked `path` keeps the link
/// and replaces its target; an existing file keeps its permissions.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let target = resolve_symlink(path);
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&dir)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &target)?;
        // Make the rename durable; not every platform can open a directory.
        if let Ok(d) = std::fs::File::open(&dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The final target of a chain of symlinks (at most 16), or `path`.
fn resolve_symlink(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..16 {
        match std::fs::read_link(&current) {
            Ok(link) => {
                current = match current.parent() {
                    Some(parent) if link.is_relative() => parent.join(link),
                    _ => link,
                };
            }
            Err(_) => break,
        }
    }
    current
}

/// Saves preferences to `path`. Nothing is written if serializing fails.
pub fn save(path: &Path, prefs: &Preferences) -> std::io::Result<()> {
    let text = prefs.to_toml().map_err(std::io::Error::other)?;
    write_atomic(path, &text)
}

/// `name`, or `name (2)`, `name (3)`… — the first not in `taken`.
pub fn unique_name(name: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| name.to_owned())
}

/// User themes found in `dir`, sorted by name; bad files become warnings.
pub fn load_user_themes(dir: &Path) -> (Vec<Theme>, Vec<String>) {
    let mut themes = Vec::new();
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (themes, warnings);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match read_capped(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| Theme::from_toml(&t))
        {
            Ok(theme) => themes.push(theme),
            Err(e) => warnings.push(format!("{}: {e}", path.display())),
        }
    }
    themes.sort_by(|a, b| a.name.cmp(&b.name));
    (themes, warnings)
}

/// File name for a theme called `name` (unsafe characters replaced).
pub fn theme_file_name(name: &str) -> String {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.is_empty() {
        "theme".into()
    } else {
        stem
    };
    format!("{stem}.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::model::Color;

    #[test]
    fn keymap_and_input_round_trip() {
        let mut p = Preferences::default();
        p.input.invert_zoom = true;
        p.input.zoom_sensitivity = 2.5;
        p.input.pan_button = PanButton::Middle;
        p.input.emulate_three_button = true;
        p.keymap.preset = "blender".into();
        p.keymap
            .bindings
            .insert("win.save".into(), vec!["Ctrl+eacute".into(), "F5".into()]);
        p.keymap.bindings.insert("win.undo".into(), Vec::new());
        p.keymap
            .bindings
            .insert("someday.command".into(), vec!["Alt+F9".into()]);
        let text = p.to_toml().unwrap();
        let (back, warnings) = Preferences::from_toml(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, p);
    }

    #[test]
    fn keymap_and_input_are_validated() {
        let text = "[input]\nzoom_sensitivity = 99\npan_button = \"middle\"\n\
                    [keymap]\nversion = 7\npreset = \"nope\"\n\
                    [keymap.bindings]\n\"win.save\" = [\"Ctrl+dead_grave\", \"ctrl+s\"]\n";
        let (p, warnings) = Preferences::from_toml(text);
        assert_eq!(p.input.zoom_sensitivity, ZOOM_SENSITIVITY.max);
        assert_eq!(p.input.pan_button, PanButton::Middle);
        assert_eq!(p.keymap.version, crate::keymap::model::KEYMAP_VERSION);
        assert_eq!(p.keymap.preset, "viscas");
        assert_eq!(p.keymap.bindings["win.save"], vec!["Ctrl+S"]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        // A broken keymap section falls back alone.
        let (p, w) = Preferences::from_toml("auto_save = false\nkeymap = 3\n");
        assert!(!p.auto_save);
        assert_eq!(p.keymap, KeymapPrefs::default());
        assert!(!w.is_empty());
    }

    #[test]
    fn input_zoom_and_drag_modes() {
        let mut i = Input::default();
        assert!(i.zoom_exponent(1.0) < 0.0, "scrolling down zooms out");
        i.invert_zoom = true;
        i.zoom_sensitivity = 2.0;
        assert_eq!(i.zoom_exponent(1.0), 2.0);
        assert_eq!(i.left_drag(false, false, false), DragMode::Pan);
        assert_eq!(i.left_drag(true, false, false), DragMode::BoxZoom);
        assert_eq!(i.left_drag(false, false, true), DragMode::Pan);
        i.pan_button = PanButton::Middle;
        assert_eq!(i.left_drag(false, false, false), DragMode::BoxZoom);
        assert_eq!(i.left_drag(false, true, true), DragMode::BoxZoom);
        i.emulate_three_button = true;
        assert_eq!(i.left_drag(false, false, true), DragMode::Pan);
        assert_eq!(i.left_drag(false, true, true), DragMode::Stretch);
        assert_eq!(i.left_drag(true, false, true), DragMode::BoxZoom);
    }

    #[test]
    fn round_trip_keeps_everything() {
        let mut p = Preferences::default();
        p.interface.ui_scale = 1.25;
        p.interface.mono_font = "Hack".into();
        p.interface.font_rendering = FontRendering::Automatic;
        p.viewport.grid_style = GridStyle::Dots;
        p.editing.definition_op = DefinitionOp::Equals;
        p.themes.accent = Accent::Blue;
        p.theme.ui.bg1 = Color::hex(0x123456);
        p.system.dark_theme = model::HIGH_CONTRAST_NAME.into();
        let text = p.to_toml().unwrap();
        assert!(text.contains("definition_op = \"=\""));
        assert!(text.contains("font_rendering = \"automatic\""));
        let (back, warnings) = Preferences::from_toml(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, p);
    }

    #[test]
    fn font_rendering_defaults_to_desktop_hinting() {
        let (p, w) = Preferences::from_toml("[interface]\nfont_size = 12.0\n");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(p.interface.font_rendering, FontRendering::DesktopHinting);
        assert_eq!(FontRendering::DesktopHinting.gtk_nick(), "manual");
        assert_eq!(FontRendering::Automatic.gtk_nick(), "automatic");
        let (p, _) = Preferences::from_toml("[interface]\nfont_rendering = \"desktop-hinting\"\n");
        assert_eq!(p.interface.font_rendering, FontRendering::DesktopHinting);
    }

    #[test]
    fn empty_file_gives_defaults() {
        let (p, w) = Preferences::from_toml("");
        assert!(w.is_empty());
        assert_eq!(p, Preferences::default());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let (p, w) = Preferences::from_toml("[interface]\nfont_size = 14.0\n");
        assert!(w.is_empty());
        assert_eq!(p.interface.font_size, 14.0);
        assert!(p.interface.tooltips);
        assert_eq!(p.viewport, Viewport::default());
    }

    #[test]
    fn console_auto_add_defaults() {
        // Files without the keys: nothing is added automatically.
        let (p, w) = Preferences::from_toml("[editing]\nundo_steps = 50\n");
        assert!(w.is_empty());
        assert!(!p.editing.console_add_plots);
        assert!(!p.editing.console_add_definitions);
        let (p, _) = Preferences::from_toml(
            "[editing]\nconsole_add_plots = true\nconsole_add_definitions = true\n",
        );
        assert!(p.editing.console_add_plots);
        assert!(p.editing.console_add_definitions);
    }

    #[test]
    fn values_are_clamped() {
        let text = "[interface]\nui_scale = 9.0\nfont_size = -3.0\n[editing]\nundo_steps = 0\n\
                    [viewport]\nline_width = 100.0\n[save_load]\nrecent_files = 900\n";
        let (p, _) = Preferences::from_toml(text);
        assert_eq!(p.interface.ui_scale, 2.0);
        assert_eq!(p.interface.font_size, 8.0);
        assert_eq!(p.editing.undo_steps, 1);
        assert_eq!(p.viewport.line_width, 4.0);
        assert_eq!(p.save_load.recent_files, 50);
        let mut q = Preferences::default();
        q.interface.ui_scale = f64::NAN;
        q.validate();
        assert_eq!(q.interface.ui_scale, 1.0);
    }

    #[test]
    fn invalid_input_never_panics() {
        for text in [
            "[[[",
            "interface = 3",
            "[interface]\nui_scale = \"big\"\n",
            "[viewport]\ngrid_style = \"hexagons\"\n",
            "[editing]\ndefinition_op = \"<-\"\n",
            "[theme]\nui = 5\n",
            "theme = 1",
            "version = \"one\"",
        ] {
            let (p, w) = Preferences::from_toml(text);
            assert!(!w.is_empty(), "{text:?} should warn");
            assert!(p.interface.ui_scale > 0.0);
        }
    }

    #[test]
    fn a_bad_section_does_not_reset_the_others() {
        let (p, w) = Preferences::from_toml(
            "[interface]\nfont_size = 15.0\n[viewport]\ngrid_style = \"hexagons\"\n",
        );
        assert_eq!(w.len(), 2);
        assert_eq!(p.interface.font_size, 15.0);
        assert_eq!(p.viewport, Viewport::default());
    }

    #[test]
    fn theme_falls_back_to_preset() {
        let (p, w) = Preferences::from_toml(
            "[themes]\npreset = \"Viscas Light\"\n[theme]\nname = \"Mine\"\n[theme.ui]\nfg = \"#010101\"\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(p.theme.name, "Mine");
        assert_eq!(p.theme.ui.fg, Color::hex(0x010101));
        assert_eq!(p.theme.ui.bg1, model::viscas_light().ui.bg1);
        let (q, _) = Preferences::from_toml("[themes]\npreset = \"Viscas Light\"\n");
        assert_eq!(q.theme, model::viscas_light());
    }

    #[test]
    fn newer_version_warns_but_loads() {
        let (p, w) = Preferences::from_toml("version = 99\nauto_save = false\n");
        assert_eq!(w.len(), 1);
        assert!(!p.auto_save);
        assert_eq!(p.version, VERSION);
    }

    #[test]
    fn config_dir_resolution() {
        assert_eq!(
            config_dir_from(Some("/x"), Some("/h")),
            Some(PathBuf::from("/x/viscas"))
        );
        assert_eq!(
            config_dir_from(Some("relative"), Some("/h")),
            Some(PathBuf::from("/h/.config/viscas"))
        );
        assert_eq!(
            config_dir_from(None, Some("/h")),
            Some(PathBuf::from("/h/.config/viscas"))
        );
        assert_eq!(config_dir_from(None, None), None);
    }

    #[test]
    fn files_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!("viscas-prefs-test-{}", std::process::id()));
        let path = dir.join("preferences.toml");
        let mut p = Preferences::default();
        p.system.follow_system = true;
        save(&path, &p).unwrap();
        let loaded = load(&path);
        assert!(loaded.warnings.is_empty() && !loaded.lossy);
        assert_eq!(loaded.prefs, p);
        let missing = load(&dir.join("nope.toml"));
        assert!(missing.warnings.is_empty() && !missing.lossy);
        assert_eq!(missing.prefs, Preferences::default());

        // A damaged file is reported as lossy. Backing up the same content
        // twice reuses the backup; different content never overwrites it.
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[[[").unwrap();
        assert!(load(&bad).lossy);
        let b1 = backup(&bad).unwrap();
        assert_eq!(backup(&bad).unwrap(), b1);
        std::fs::write(&bad, "[[[ v2").unwrap();
        let b2 = backup(&bad).unwrap();
        assert_ne!(b1, b2);
        assert_eq!(std::fs::read_to_string(&b1).unwrap(), "[[[");
        assert!(b1.to_string_lossy().ends_with("bad.toml.bak"));
        assert!(b2.to_string_lossy().ends_with("bad.toml.bak.1"));

        // Oversized files are refused.
        let big = dir.join("big.toml");
        std::fs::write(&big, vec![b'#'; MAX_FILE_BYTES as usize + 10]).unwrap();
        assert!(read_capped(&big).is_err());
        assert!(load(&big).lossy);

        let themes = dir.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(
            themes.join("good.toml"),
            model::viscas_light().to_toml().unwrap(),
        )
        .unwrap();
        std::fs::write(themes.join("bad.toml"), "[ui]\nbg1 = 4").unwrap();
        std::fs::write(themes.join("skip.txt"), "x").unwrap();
        let (list, w) = load_user_themes(&themes);
        assert_eq!(list.len(), 1);
        assert_eq!(w.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_symlink_and_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = std::env::temp_dir().join(format!("viscas-atomic-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.toml");
        std::fs::write(&real, "old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("link.toml");
        symlink("real.toml", &link).unwrap();
        write_atomic(&link, "new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn out_of_range_numbers_are_clamped_not_dropped() {
        let (p, w) = Preferences::from_toml(
            "[editing]\nundo_steps = -5\n[save_load]\nautosave_minutes = 99999999999999\nrecent_files = 3\n\
             [interface]\nui_scale = 2\nfont_size = 1e9\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(p.editing.undo_steps, 1);
        assert_eq!(p.save_load.autosave_minutes, 120);
        assert_eq!(p.save_load.recent_files, 3);
        assert_eq!(p.interface.ui_scale, 2.0);
        assert_eq!(p.interface.font_size, 20.0);
    }

    #[test]
    fn strings_are_capped_and_sanitized() {
        let mut p = Preferences::default();
        p.interface.mono_font = "Fira\"; } * { color: red; /*".into();
        p.theme.name = "x".repeat(500);
        p.themes.preset = "\u{7}".into();
        p.system.light_theme = "  ".into();
        p.validate();
        assert_eq!(p.interface.mono_font, "Fira color red");
        assert_eq!(p.theme.name.chars().count(), MAX_NAME_LEN);
        assert_eq!(p.themes.preset, p.theme.name);
        assert_eq!(p.system.light_theme, model::LIGHT_NAME);
        assert_eq!(clean_font(&"a".repeat(200)).len(), MAX_FONT_LEN);
    }

    #[test]
    fn unique_names() {
        let taken = vec!["Mine".to_owned(), "Mine (2)".to_owned()];
        assert_eq!(unique_name("Other", &taken), "Other");
        assert_eq!(unique_name("Mine", &taken), "Mine (3)");
    }

    #[test]
    fn theme_file_names_are_safe() {
        assert_eq!(theme_file_name("My Theme/../x"), "my_theme____x.toml");
        assert_eq!(theme_file_name("  "), "theme.toml");
    }
}
