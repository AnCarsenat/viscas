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
//! [`FORMAT_VERSION`] are refused. The app's area/workspace layout travels
//! as an opaque JSON value in `layout`.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::node::{Node, NodeContent, NodeId, NodeKind, PropValue, Style};
use crate::scene::Scene;

/// Current document format version.
pub const FORMAT_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One node with its children, as stored on disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocNode {
    pub id: NodeId,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DocNode>,
}

/// A whole document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    /// Next id to allocate (ids are never reused).
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub nodes: Vec<DocNode>,
    /// App-owned area/workspace layout, opaque to the scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<serde_json::Value>,
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
            DocumentError::Serialize(m) => write!(f, "cannot write document: {m}"),
        }
    }
}

impl std::error::Error for DocumentError {}

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
    }
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
        }
    }

    /// Build a scene from a document. Every node starts dirty; history and
    /// selection are empty.
    pub fn from_document(doc: Document) -> Result<Scene, DocumentError> {
        let mut scene = Scene::new();
        let mut seen = HashSet::new();
        let mut max_id = 0;
        fn walk(
            scene: &mut Scene,
            seen: &mut HashSet<NodeId>,
            max_id: &mut u64,
            parent: Option<NodeId>,
            d: DocNode,
        ) -> Result<NodeId, DocumentError> {
            if !seen.insert(d.id) {
                return Err(DocumentError::DuplicateId(d.id));
            }
            *max_id = (*max_id).max(d.id.0);
            let mut node = Node::new(
                d.id,
                NodeContent {
                    name: d.name,
                    kind: d.kind,
                    visible: d.visible,
                    locked: d.locked,
                    style: d.style,
                    props: d.props,
                },
            );
            node.parent = parent;
            let id = node.id;
            scene.nodes.insert(id, node);
            let mut children = Vec::with_capacity(d.children.len());
            for c in d.children {
                children.push(walk(scene, seen, max_id, Some(id), c)?);
            }
            if let Some(n) = scene.nodes.get_mut(&id) {
                n.children = children;
            }
            Ok(id)
        }
        for d in doc.nodes {
            let id = walk(&mut scene, &mut seen, &mut max_id, None, d)?;
            scene.roots.push(id);
        }
        scene.next_id = doc.next_id.max(max_id + 1);
        scene.layout = doc.layout;
        scene.reanalyze_all();
        scene.drain_events();
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
