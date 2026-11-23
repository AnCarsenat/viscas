//! The `.viscas` document format.
//!
//! Documents are [RON](https://github.com/ron-rs/ron) text. RON was chosen
//! over JSON and TOML because it maps serde enums naturally — a node kind
//! reads `Function(params: ["x"], body: "x^2 + a")` instead of JSON's
//! `{"Function": {...}}` nesting or TOML's poor support for deep arrays of
//! tables — and it allows comments, trailing commas and unquoted field
//! names, which keeps hand-edited and diffed documents readable. The node
//! tree is stored nested (children inline), defaults are omitted, and
//! `Option`s are written bare (`implicit_some`).
//!
//! ```ron
//! #![enable(implicit_some)]
//! (
//!     version: 1,
//!     next_id: 4,
//!     nodes: [
//!         (id: 1, name: "a", kind: Slider(value: "3", min: "0", max: "10")),
//!         (id: 2, name: "f", kind: Function(params: ["x"], body: "a x^2")),
//!         (id: 3, kind: Plot(relation: "y = f(x)")),
//!     ],
//! )
//! ```
//!
//! Every document carries a `version`. [`load_document`] reads the version
//! first and routes older versions through [`migrate`]; documents newer than
//! [`FORMAT_VERSION`] are refused. Fields this version does not know (on the
//! document or on a node) are kept as raw RON text and written back
//! unchanged, so additive changes by newer versions round-trip. The app's
//! area/workspace layout travels as an opaque JSON value in `layout`.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::names::is_valid_identifier;
use crate::node::{Node, NodeContent, NodeId, NodeKind, PropValue, Style};
use crate::scene::Scene;

/// Current document format version.
pub const FORMAT_VERSION: u32 = 1;

/// One node with its children, as stored on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct DocNode {
    pub id: NodeId,
    pub name: Option<String>,
    pub kind: NodeKind,
    pub visible: bool,
    pub locked: bool,
    pub style: Style,
    pub props: BTreeMap<String, PropValue>,
    pub children: Vec<DocNode>,
    /// Unknown fields as raw RON text.
    pub extra: BTreeMap<String, String>,
}

/// A whole document.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub version: u32,
    /// Next id to allocate (ids are never reused).
    pub next_id: u64,
    pub nodes: Vec<DocNode>,
    /// App-owned area/workspace layout, opaque to the scene.
    pub layout: Option<serde_json::Value>,
    /// Unknown fields as raw RON text.
    pub extra: BTreeMap<String, String>,
}

/// Errors reading or writing documents.
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentError {
    /// Malformed RON (message includes the position).
    Syntax(String),
    /// The `version` field is missing.
    MissingVersion,
    /// Written by a newer Viscas.
    TooNew {
        version: u32,
    },
    /// No migration path from this version.
    Unsupported {
        version: u32,
    },
    /// Two nodes share an id.
    DuplicateId(NodeId),
    /// A node id is too large to allocate a successor.
    IdOverflow(NodeId),
    /// A node's content is invalid (bad name or parameter).
    InvalidContent {
        id: NodeId,
        message: String,
    },
    Serialize(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::Syntax(m) => write!(f, "malformed document: {m}"),
            DocumentError::MissingVersion => f.write_str("document has no version"),
            DocumentError::TooNew { version } => write!(
                f,
                "document version {version} is newer than supported ({FORMAT_VERSION})"
            ),
            DocumentError::Unsupported { version } => {
                write!(f, "document version {version} is not supported")
            }
            DocumentError::DuplicateId(id) => write!(f, "duplicate node id {id}"),
            DocumentError::IdOverflow(id) => write!(f, "node id {id} is too large"),
            DocumentError::InvalidContent { id, message } => write!(f, "node {id}: {message}"),
            DocumentError::Serialize(m) => write!(f, "cannot write document: {m}"),
        }
    }
}

impl std::error::Error for DocumentError {}

// ---------------------------------------------------------------------------
// Serde with preserved unknown fields
// ---------------------------------------------------------------------------

/// Field names of unknown fields must be `'static` for the serializer;
/// each distinct name is leaked once.
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = NAMES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(&n) = set.get(name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    set.insert(leaked);
    leaked
}

fn serialize_extra<S: SerializeStruct>(
    st: &mut S,
    extra: &BTreeMap<String, String>,
) -> Result<(), S::Error> {
    for (k, raw) in extra {
        let value = ron::value::RawValue::from_ron(raw).map_err(serde::ser::Error::custom)?;
        st.serialize_field(intern(k), value)?;
    }
    Ok(())
}

fn read_extra<'de, A: MapAccess<'de>>(
    map: &mut A,
    key: String,
    extra: &mut BTreeMap<String, String>,
) -> Result<(), A::Error> {
    let raw: Box<ron::value::RawValue> = map.next_value()?;
    extra.insert(key, raw.get_ron().trim().to_string());
    Ok(())
}

const NODE_FIELDS: &[&str] = &[
    "id", "name", "kind", "visible", "locked", "style", "props", "children",
];

impl Serialize for DocNode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("DocNode", NODE_FIELDS.len() + self.extra.len())?;
        st.serialize_field("id", &self.id)?;
        if let Some(name) = &self.name {
            st.serialize_field("name", name)?;
        }
        st.serialize_field("kind", &self.kind)?;
        if !self.visible {
            st.serialize_field("visible", &false)?;
        }
        if self.locked {
            st.serialize_field("locked", &true)?;
        }
        if !self.style.is_default() {
            st.serialize_field("style", &self.style)?;
        }
        if !self.props.is_empty() {
            st.serialize_field("props", &self.props)?;
        }
        if !self.children.is_empty() {
            st.serialize_field("children", &self.children)?;
        }
        serialize_extra(&mut st, &self.extra)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for DocNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<DocNode, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = DocNode;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a node")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DocNode, A::Error> {
                let mut id = None;
                let mut kind = None;
                let mut node = DocNode {
                    id: NodeId(0),
                    name: None,
                    kind: NodeKind::Folder,
                    visible: true,
                    locked: false,
                    style: Style::default(),
                    props: BTreeMap::new(),
                    children: Vec::new(),
                    extra: BTreeMap::new(),
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => id = Some(map.next_value()?),
                        "name" => node.name = map.next_value()?,
                        "kind" => kind = Some(map.next_value()?),
                        "visible" => node.visible = map.next_value()?,
                        "locked" => node.locked = map.next_value()?,
                        "style" => node.style = map.next_value()?,
                        "props" => node.props = map.next_value()?,
                        "children" => node.children = map.next_value()?,
                        _ => read_extra(&mut map, key, &mut node.extra)?,
                    }
                }
                node.id = id.ok_or_else(|| de::Error::missing_field("id"))?;
                node.kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
                Ok(node)
            }
        }
        d.deserialize_struct("DocNode", NODE_FIELDS, V)
    }
}

const DOC_FIELDS: &[&str] = &["version", "next_id", "nodes", "layout"];

impl Serialize for Document {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Document", DOC_FIELDS.len() + self.extra.len())?;
        st.serialize_field("version", &self.version)?;
        st.serialize_field("next_id", &self.next_id)?;
        st.serialize_field("nodes", &self.nodes)?;
        if let Some(layout) = &self.layout {
            st.serialize_field("layout", layout)?;
        }
        serialize_extra(&mut st, &self.extra)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Document, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Document;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a document")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Document, A::Error> {
                let mut version = None;
                let mut doc = Document {
                    version: 0,
                    next_id: 0,
                    nodes: Vec::new(),
                    layout: None,
                    extra: BTreeMap::new(),
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "version" => version = Some(map.next_value()?),
                        "next_id" => doc.next_id = map.next_value()?,
                        "nodes" => doc.nodes = map.next_value()?,
                        "layout" => doc.layout = map.next_value()?,
                        _ => read_extra(&mut map, key, &mut doc.extra)?,
                    }
                }
                doc.version = version.ok_or_else(|| de::Error::missing_field("version"))?;
                Ok(doc)
            }
        }
        d.deserialize_struct("Document", DOC_FIELDS, V)
    }
}

// ---------------------------------------------------------------------------
// Reading and writing
// ---------------------------------------------------------------------------

fn ron_options() -> ron::Options {
    ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
}

/// Serialize a document to pretty RON.
pub fn save_document(doc: &Document) -> Result<String, DocumentError> {
    let config = ron::ser::PrettyConfig::new()
        .struct_names(false)
        .indentor("    ")
        .extensions(ron::extensions::Extensions::IMPLICIT_SOME);
    let mut text = ron_options()
        .to_string_pretty(doc, config)
        .map_err(|e| DocumentError::Serialize(e.to_string()))?;
    text.push('\n');
    Ok(text)
}

#[derive(Deserialize)]
struct VersionProbe {
    version: Option<u32>,
}

/// Parse a document of any supported version.
pub fn load_document(text: &str) -> Result<Document, DocumentError> {
    let probe: VersionProbe = ron_options()
        .from_str(text)
        .map_err(|e| DocumentError::Syntax(e.to_string()))?;
    let version = probe.version.ok_or(DocumentError::MissingVersion)?;
    if version > FORMAT_VERSION {
        return Err(DocumentError::TooNew { version });
    }
    migrate(version, text)
}

/// Migration hook: turn a document of an older `version` into the current
/// [`Document`]. When the format changes, bump [`FORMAT_VERSION`], keep the
/// old schema as a private struct and add an arm converting it here.
pub fn migrate(version: u32, text: &str) -> Result<Document, DocumentError> {
    match version {
        FORMAT_VERSION => ron_options()
            .from_str(text)
            .map_err(|e| DocumentError::Syntax(e.to_string())),
        _ => Err(DocumentError::Unsupported { version }),
    }
}

fn to_doc_node(scene: &Scene, node: &Node) -> DocNode {
    DocNode {
        id: node.id,
        name: node.name.clone(),
        kind: node.kind.clone(),
        visible: node.visible,
        locked: node.locked,
        style: node.style.clone(),
        props: node.props.clone(),
        children: node
            .children
            .iter()
            .filter_map(|c| scene.get(*c))
            .map(|c| to_doc_node(scene, c))
            .collect(),
        extra: node.extra.clone(),
    }
}

/// Check what the scene API would refuse: invalid names of defining
/// nodes and invalid or repeated parameters. (Duplicate names are allowed
/// and reported as diagnostics.)
fn validate_content(id: NodeId, c: &NodeContent) -> Result<(), DocumentError> {
    let invalid = |message: String| DocumentError::InvalidContent { id, message };
    if c.kind.defines_by_name()
        && let Some(name) = &c.name
        && !is_valid_identifier(name)
    {
        return Err(invalid(format!("`{name}` is not a valid name")));
    }
    if let NodeKind::Function { params, .. } | NodeKind::Regression { params, .. } = &c.kind {
        let mut seen = HashSet::new();
        for p in params {
            if !is_valid_identifier(p) || !seen.insert(p) {
                return Err(invalid(format!("invalid parameter `{p}`")));
            }
        }
    }
    Ok(())
}

impl Scene {
    /// Snapshot as a [`Document`].
    pub fn to_document(&self) -> Document {
        Document {
            version: FORMAT_VERSION,
            next_id: self.next_id,
            nodes: self
                .roots
                .iter()
                .filter_map(|r| self.get(*r))
                .map(|n| to_doc_node(self, n))
                .collect(),
            layout: self.layout.clone(),
            extra: self.doc_extra.clone(),
        }
    }

    /// Build a scene from a document. Every node starts dirty; history and
    /// selection are empty.
    pub fn from_document(doc: Document) -> Result<Scene, DocumentError> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        fn walk(
            nodes: &mut Vec<Node>,
            seen: &mut HashSet<NodeId>,
            parent: Option<NodeId>,
            d: DocNode,
        ) -> Result<NodeId, DocumentError> {
            if !seen.insert(d.id) {
                return Err(DocumentError::DuplicateId(d.id));
            }
            let content = Scene::normalize(NodeContent {
                name: d.name,
                kind: d.kind,
                visible: d.visible,
                locked: d.locked,
                style: d.style,
                props: d.props,
                extra: d.extra,
            });
            validate_content(d.id, &content)?;
            let mut node = Node::new(d.id, content);
            node.parent = parent;
            let id = node.id;
            let slot = nodes.len();
            nodes.push(node);
            let mut children = Vec::with_capacity(d.children.len());
            for c in d.children {
                children.push(walk(nodes, seen, Some(id), c)?);
            }
            nodes[slot].children = children;
            Ok(id)
        }
        for d in doc.nodes {
            roots.push(walk(&mut nodes, &mut seen, None, d)?);
        }
        let mut next_id = doc.next_id.max(1);
        for n in &nodes {
            let after =
                n.id.0
                    .checked_add(1)
                    .ok_or(DocumentError::IdOverflow(n.id))?;
            next_id = next_id.max(after);
        }
        let mut scene = Scene::new();
        scene.load_nodes(nodes, roots, next_id);
        scene.layout = doc.layout;
        scene.doc_extra = doc.extra;
        Ok(scene)
    }

    /// Serialize to `.viscas` text.
    pub fn save_to_string(&self) -> Result<String, DocumentError> {
        save_document(&self.to_document())
    }

    /// Parse `.viscas` text.
    pub fn load_from_str(text: &str) -> Result<Scene, DocumentError> {
        Scene::from_document(load_document(text)?)
    }
}
