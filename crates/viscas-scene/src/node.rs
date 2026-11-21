//! Nodes: identity, tree links, kind-specific body, style and free-form
//! properties.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable node identifier. Ids are allocated monotonically by the scene and
/// never reused, so they stay valid across undo/redo and save/load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Which text field of a node a span, parse error or reference belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceField {
    /// The kind's main text: expression, relation, function body, script.
    Main,
    /// Slider minimum.
    Min,
    /// Slider maximum.
    Max,
    /// Slider step.
    Step,
    /// One table cell.
    Cell { column: usize, row: usize },
    /// Whole-column formula of a table column.
    Formula { column: usize },
}

impl fmt::Display for SourceField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceField::Main => f.write_str("main"),
            SourceField::Min => f.write_str("min"),
            SourceField::Max => f.write_str("max"),
            SourceField::Step => f.write_str("step"),
            SourceField::Cell { column, row } => write!(f, "cell[{column}][{row}]"),
            SourceField::Formula { column } => write!(f, "formula[{column}]"),
        }
    }
}

/// How a source field is parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseMode {
    /// [`viscas_core::syntax::parse_expr`].
    Expr,
    /// [`viscas_core::syntax::parse_statement`].
    Statement,
    /// [`viscas_core::syntax::parse_program_partial`].
    Program,
}

/// A borrowed source field of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source<'a> {
    pub field: SourceField,
    pub text: &'a str,
    pub mode: ParseMode,
}

/// Script language of a [`NodeKind::Script`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptLanguage {
    #[default]
    Viscas,
    Rhai,
}

/// Slider animation mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnimationMode {
    #[default]
    Off,
    /// Wrap around from max to min.
    Loop,
    /// Ping-pong between min and max.
    Bounce,
    /// Run from min to max once.
    Once,
}

/// Slider animation settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    pub mode: AnimationMode,
    /// Speed multiplier (1.0 = traverse the range in about 10 s).
    pub speed: f64,
    pub playing: bool,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            mode: AnimationMode::Off,
            speed: 1.0,
            playing: false,
        }
    }
}

/// One column of a [`NodeKind::Table`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// Cell sources (values or expressions); empty strings are blank cells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<String>,
    /// Optional whole-column formula (`x1^2`), overriding the cells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

impl Column {
    pub fn new(name: impl Into<String>, cells: &[&str]) -> Self {
        Column {
            name: name.into(),
            cells: cells.iter().map(|s| s.to_string()).collect(),
            formula: None,
        }
    }
}

/// Visible region of a viewport.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            x_min: -10.0,
            x_max: 10.0,
            y_min: -10.0,
            y_max: 10.0,
        }
    }
}

/// Background grid of a viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GridKind {
    None,
    #[default]
    Cartesian,
    Polar,
}

/// Viewport projection. Only 2D for now; 3D lands in M9.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Projection {
    #[default]
    Orthographic2D,
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The type and body of a node (spec §3).
///
/// Defining kinds ([`NodeKind::defines_by_name`]) take their defined name
/// from [`Node::name`]: the body holds only the right-hand side, so a
/// Variable `a = 3` is `name: "a"`, `expr: "3"` and a Function
/// `f(x) = x^2` is `name: "f"`, `params: ["x"]`, `body: "x^2"`. This keeps
/// one source of truth for the name (renames never re-parse) and gives the
/// inspector separate name / expression fields. [`crate::parse_definition`]
/// splits an expression-list line into these parts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Folder,
    /// `a = 3`; unnamed variables are anonymous expressions (`2 + 3`).
    Variable {
        expr: String,
    },
    /// `f(x, y) = body`.
    Function {
        params: Vec<String>,
        body: String,
    },
    /// A relation: explicit `y = sin x`, polar `r = θ`, parametric
    /// `(cos t, sin t)`, implicit `x^2 + y^2 = 4`, inequalities and
    /// restrictions. Its name is only a label.
    Plot {
        relation: String,
    },
    /// `P = (1, 2)`.
    Point {
        expr: String,
    },
    /// A variable with slider metadata. `min`, `max` and `step` are
    /// expressions too (empty `step` = continuous).
    Slider {
        value: String,
        #[serde(default)]
        min: String,
        #[serde(default)]
        max: String,
        #[serde(default)]
        step: String,
        #[serde(default)]
        animation: Animation,
    },
    /// `L = [1..10]`.
    List {
        expr: String,
    },
    /// A sheet of named columns, usable as `T.x1`.
    Table {
        columns: Vec<Column>,
    },
    /// Script node; Viscas scripts define their top-level `fn`s.
    Script {
        #[serde(default)]
        language: ScriptLanguage,
        source: String,
    },
    /// Text / markdown.
    Note {
        text: String,
    },
    /// A 2D graph view.
    Viewport {
        #[serde(default)]
        bounds: Bounds,
        #[serde(default)]
        grid: GridKind,
        #[serde(default = "default_true")]
        minor_grid: bool,
        #[serde(default = "default_true")]
        axes: bool,
        #[serde(default = "default_true")]
        axis_numbers: bool,
        #[serde(default)]
        lock_aspect: bool,
        #[serde(default)]
        projection: Projection,
    },
    /// `a -> a + 1`; a ticker when `interval_ms` is set.
    Action {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval_ms: Option<u64>,
        #[serde(default)]
        running: bool,
    },
    /// `y1 ~ m x1 + b`; defines the fitted parameters `params`.
    Regression {
        source: String,
        params: Vec<String>,
    },
}

/// Field-less discriminant of [`NodeKind`], for icons, filters and menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KindTag {
    Folder,
    Variable,
    Function,
    Plot,
    Point,
    Slider,
    List,
    Table,
    Script,
    Note,
    Viewport,
    Action,
    Regression,
}

impl KindTag {
    pub const ALL: [KindTag; 13] = [
        KindTag::Folder,
        KindTag::Variable,
        KindTag::Function,
        KindTag::Plot,
        KindTag::Point,
        KindTag::Slider,
        KindTag::List,
        KindTag::Table,
        KindTag::Script,
        KindTag::Note,
        KindTag::Viewport,
        KindTag::Action,
        KindTag::Regression,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            KindTag::Folder => "folder",
            KindTag::Variable => "variable",
            KindTag::Function => "function",
            KindTag::Plot => "plot",
            KindTag::Point => "point",
            KindTag::Slider => "slider",
            KindTag::List => "list",
            KindTag::Table => "table",
            KindTag::Script => "script",
            KindTag::Note => "note",
            KindTag::Viewport => "viewport",
            KindTag::Action => "action",
            KindTag::Regression => "regression",
        }
    }
}

impl fmt::Display for KindTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl NodeKind {
    pub fn tag(&self) -> KindTag {
        match self {
            NodeKind::Folder => KindTag::Folder,
            NodeKind::Variable { .. } => KindTag::Variable,
            NodeKind::Function { .. } => KindTag::Function,
            NodeKind::Plot { .. } => KindTag::Plot,
            NodeKind::Point { .. } => KindTag::Point,
            NodeKind::Slider { .. } => KindTag::Slider,
            NodeKind::List { .. } => KindTag::List,
            NodeKind::Table { .. } => KindTag::Table,
            NodeKind::Script { .. } => KindTag::Script,
            NodeKind::Note { .. } => KindTag::Note,
            NodeKind::Viewport { .. } => KindTag::Viewport,
            NodeKind::Action { .. } => KindTag::Action,
            NodeKind::Regression { .. } => KindTag::Regression,
        }
    }

    /// Does [`Node::name`] define a name other nodes can reference? For the
    /// other kinds the name is a free-text label.
    pub fn defines_by_name(&self) -> bool {
        matches!(
            self,
            NodeKind::Variable { .. }
                | NodeKind::Function { .. }
                | NodeKind::Point { .. }
                | NodeKind::Slider { .. }
                | NodeKind::List { .. }
                | NodeKind::Table { .. }
        )
    }

    /// Kinds whose dependency cycles are legal recursion (function bodies
    /// are not evaluated when defined).
    pub fn allows_recursion(&self) -> bool {
        matches!(self, NodeKind::Function { .. } | NodeKind::Script { .. })
    }

    /// Default slider with range `[-10, 10]`.
    pub fn slider(value: impl Into<String>) -> NodeKind {
        NodeKind::Slider {
            value: value.into(),
            min: "-10".into(),
            max: "10".into(),
            step: String::new(),
            animation: Animation::default(),
        }
    }

    /// Default viewport.
    pub fn viewport() -> NodeKind {
        NodeKind::Viewport {
            bounds: Bounds::default(),
            grid: GridKind::default(),
            minor_grid: true,
            axes: true,
            axis_numbers: true,
            lock_aspect: false,
            projection: Projection::default(),
        }
    }

    /// Every parseable text field, in a stable order.
    pub fn sources(&self) -> Vec<Source<'_>> {
        fn main(text: &str, mode: ParseMode) -> Source<'_> {
            Source {
                field: SourceField::Main,
                text,
                mode,
            }
        }
        fn f(field: SourceField, text: &str) -> Source<'_> {
            Source {
                field,
                text,
                mode: ParseMode::Expr,
            }
        }
        match self {
            NodeKind::Folder | NodeKind::Note { .. } | NodeKind::Viewport { .. } => Vec::new(),
            NodeKind::Variable { expr } | NodeKind::Point { expr } | NodeKind::List { expr } => {
                vec![main(expr, ParseMode::Expr)]
            }
            NodeKind::Function { body, .. } => vec![main(body, ParseMode::Expr)],
            NodeKind::Plot { relation } => vec![main(relation, ParseMode::Statement)],
            NodeKind::Action { source, .. } | NodeKind::Regression { source, .. } => {
                vec![main(source, ParseMode::Statement)]
            }
            NodeKind::Script { language, source } => match language {
                ScriptLanguage::Viscas => vec![main(source, ParseMode::Program)],
                ScriptLanguage::Rhai => Vec::new(),
            },
            NodeKind::Slider {
                value,
                min,
                max,
                step,
                ..
            } => {
                vec![
                    main(value, ParseMode::Expr),
                    f(SourceField::Min, min),
                    f(SourceField::Max, max),
                    f(SourceField::Step, step),
                ]
            }
            NodeKind::Table { columns } => {
                let mut out = Vec::new();
                for (c, col) in columns.iter().enumerate() {
                    if let Some(formula) = &col.formula {
                        out.push(Source {
                            field: SourceField::Formula { column: c },
                            text: formula,
                            mode: ParseMode::Expr,
                        });
                    }
                    for (r, cell) in col.cells.iter().enumerate() {
                        out.push(Source {
                            field: SourceField::Cell { column: c, row: r },
                            text: cell,
                            mode: ParseMode::Expr,
                        });
                    }
                }
                out
            }
        }
    }

    /// Text of one source field.
    pub fn source(&self, field: SourceField) -> Option<&str> {
        self.sources()
            .into_iter()
            .find(|s| s.field == field)
            .map(|s| s.text)
    }

    /// Mutable text of one source field.
    pub fn source_mut(&mut self, field: SourceField) -> Option<&mut String> {
        match (self, field) {
            (
                NodeKind::Variable { expr } | NodeKind::Point { expr } | NodeKind::List { expr },
                SourceField::Main,
            ) => Some(expr),
            (NodeKind::Function { body, .. }, SourceField::Main) => Some(body),
            (NodeKind::Plot { relation }, SourceField::Main) => Some(relation),
            (
                NodeKind::Action { source, .. } | NodeKind::Regression { source, .. },
                SourceField::Main,
            ) => Some(source),
            (NodeKind::Script { source, .. }, SourceField::Main) => Some(source),
            (NodeKind::Slider { value, .. }, SourceField::Main) => Some(value),
            (NodeKind::Slider { min, .. }, SourceField::Min) => Some(min),
            (NodeKind::Slider { max, .. }, SourceField::Max) => Some(max),
            (NodeKind::Slider { step, .. }, SourceField::Step) => Some(step),
            (NodeKind::Table { columns }, SourceField::Cell { column, row }) => {
                columns.get_mut(column)?.cells.get_mut(row)
            }
            (NodeKind::Table { columns }, SourceField::Formula { column }) => {
                columns.get_mut(column)?.formula.as_mut()
            }
            (NodeKind::Note { text }, SourceField::Main) => Some(text),
            _ => None,
        }
    }
}

/// An RGBA color, serialized as `"#rrggbb"` or `"#rrggbbaa"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

/// Error parsing a [`Color`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorParseError(pub String);

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid color `{}` (expected #rrggbb or #rrggbbaa)",
            self.0
        )
    }
}

impl std::error::Error for ColorParseError {}

impl FromStr for Color {
    type Err = ColorParseError;

    fn from_str(s: &str) -> Result<Color, ColorParseError> {
        let err = || ColorParseError(s.to_string());
        let hex = s.strip_prefix('#').ok_or_else(err)?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        Ok(Color {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 255 },
        })
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Stroke pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// Visual style shared by all nodes (spec §3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    /// `None` = automatic palette color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    pub line_width: f64,
    pub line_style: LineStyle,
    pub opacity: f64,
    pub fill_opacity: f64,
    pub point_size: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub show_label: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: None,
            line_width: 2.0,
            line_style: LineStyle::Solid,
            opacity: 1.0,
            fill_opacity: 0.25,
            point_size: 6.0,
            label: None,
            show_label: false,
        }
    }
}

impl Style {
    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }
}

/// A free-form property value (metadata the app attaches to nodes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PropValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl From<bool> for PropValue {
    fn from(v: bool) -> Self {
        PropValue::Bool(v)
    }
}

impl From<i64> for PropValue {
    fn from(v: i64) -> Self {
        PropValue::Int(v)
    }
}

impl From<f64> for PropValue {
    fn from(v: f64) -> Self {
        PropValue::Float(v)
    }
}

impl From<&str> for PropValue {
    fn from(v: &str) -> Self {
        PropValue::Text(v.to_string())
    }
}

impl From<String> for PropValue {
    fn from(v: String) -> Self {
        PropValue::Text(v)
    }
}

/// Everything about a node except its identity and tree links: the unit
/// of content edits (and of their undo records).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: NodeKind,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Style::is_default")]
    pub style: Style,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, PropValue>,
}

impl NodeContent {
    /// Unnamed, visible, unlocked, default style.
    pub fn new(kind: NodeKind) -> Self {
        NodeContent {
            name: None,
            kind,
            visible: true,
            locked: false,
            style: Style::default(),
            props: BTreeMap::new(),
        }
    }

    /// Builder: set the name.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn folder(name: impl Into<String>) -> Self {
        Self::new(NodeKind::Folder).named(name)
    }

    pub fn variable(name: impl Into<String>, expr: impl Into<String>) -> Self {
        Self::new(NodeKind::Variable { expr: expr.into() }).named(name)
    }

    /// Unnamed expression node.
    pub fn expression(expr: impl Into<String>) -> Self {
        Self::new(NodeKind::Variable { expr: expr.into() })
    }

    pub fn function(name: impl Into<String>, params: &[&str], body: impl Into<String>) -> Self {
        Self::new(NodeKind::Function {
            params: params.iter().map(|p| p.to_string()).collect(),
            body: body.into(),
        })
        .named(name)
    }

    pub fn plot(relation: impl Into<String>) -> Self {
        Self::new(NodeKind::Plot {
            relation: relation.into(),
        })
    }

    pub fn point(name: impl Into<String>, expr: impl Into<String>) -> Self {
        Self::new(NodeKind::Point { expr: expr.into() }).named(name)
    }

    pub fn slider(
        name: impl Into<String>,
        value: impl Into<String>,
        min: impl Into<String>,
        max: impl Into<String>,
    ) -> Self {
        Self::new(NodeKind::Slider {
            value: value.into(),
            min: min.into(),
            max: max.into(),
            step: String::new(),
            animation: Animation::default(),
        })
        .named(name)
    }

    pub fn list(name: impl Into<String>, expr: impl Into<String>) -> Self {
        Self::new(NodeKind::List { expr: expr.into() }).named(name)
    }

    pub fn table(name: impl Into<String>, columns: Vec<Column>) -> Self {
        Self::new(NodeKind::Table { columns }).named(name)
    }

    pub fn script(language: ScriptLanguage, source: impl Into<String>) -> Self {
        Self::new(NodeKind::Script {
            language,
            source: source.into(),
        })
    }

    pub fn note(text: impl Into<String>) -> Self {
        Self::new(NodeKind::Note { text: text.into() })
    }

    pub fn viewport(name: impl Into<String>) -> Self {
        Self::new(NodeKind::viewport()).named(name)
    }

    pub fn action(source: impl Into<String>) -> Self {
        Self::new(NodeKind::Action {
            source: source.into(),
            interval_ms: None,
            running: false,
        })
    }

    pub fn ticker(source: impl Into<String>, interval_ms: u64) -> Self {
        Self::new(NodeKind::Action {
            source: source.into(),
            interval_ms: Some(interval_ms),
            running: false,
        })
    }

    pub fn regression(source: impl Into<String>, params: &[&str]) -> Self {
        Self::new(NodeKind::Regression {
            source: source.into(),
            params: params.iter().map(|p| p.to_string()).collect(),
        })
    }
}

/// A node of the scene tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    /// Ordered children.
    pub children: Vec<NodeId>,
    pub name: Option<String>,
    pub kind: NodeKind,
    pub visible: bool,
    pub locked: bool,
    pub style: Style,
    pub props: BTreeMap<String, PropValue>,
}

impl Node {
    pub(crate) fn new(id: NodeId, content: NodeContent) -> Node {
        Node {
            id,
            parent: None,
            children: Vec::new(),
            name: content.name,
            kind: content.kind,
            visible: content.visible,
            locked: content.locked,
            style: content.style,
            props: content.props,
        }
    }

    /// Copy of the editable content.
    pub fn content(&self) -> NodeContent {
        NodeContent {
            name: self.name.clone(),
            kind: self.kind.clone(),
            visible: self.visible,
            locked: self.locked,
            style: self.style.clone(),
            props: self.props.clone(),
        }
    }

    pub(crate) fn set_content(&mut self, c: NodeContent) {
        self.name = c.name;
        self.kind = c.kind;
        self.visible = c.visible;
        self.locked = c.locked;
        self.style = c.style;
        self.props = c.props;
    }

    /// The name this node defines, if its kind defines by name.
    pub fn defined_name(&self) -> Option<&str> {
        if self.kind.defines_by_name() {
            self.name.as_deref()
        } else {
            None
        }
    }

    /// Display title: the name, or the kind.
    pub fn title(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => self.kind.tag().to_string(),
        }
    }
}
