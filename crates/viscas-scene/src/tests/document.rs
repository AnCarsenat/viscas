//! `.viscas` serialization.

use serde_json::json;

use super::*;

fn rich_scene() -> Scene {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("Params")).unwrap();
    s.add(Some(f), NodeContent::slider("a", "3", "0", "10"))
        .unwrap();
    s.add(Some(f), NodeContent::variable("b", "a^2 + 1"))
        .unwrap();
    let mut fun = NodeContent::function("f", &["x"], "a x^2 + b");
    fun.style.color = Some(Color::rgba(0x12, 0x34, 0x56, 0x80));
    fun.style.line_style = LineStyle::Dashed;
    fun.style.label = Some("parabola".into());
    s.add(None, fun).unwrap();
    let mut p = NodeContent::plot("y = f(x) {0 < x < 2}");
    p.visible = false;
    p.locked = true;
    p.props.insert("samples".into(), PropValue::Int(512));
    p.props.insert("smooth".into(), PropValue::Bool(true));
    p.props.insert("tension".into(), PropValue::Float(0.5));
    p.props
        .insert("hint".into(), PropValue::Text("quoted \"text\"".into()));
    s.add(None, p).unwrap();
    s.add(None, NodeContent::point("P", "(1, f(1))")).unwrap();
    s.add(None, NodeContent::list("L", "[k^2 for k = [1..5]]"))
        .unwrap();
    let mut col = Column::new("x_1", &["1", "2", ""]);
    col.formula = None;
    let mut col2 = Column::new("y_1", &[]);
    col2.formula = Some("x_1^2".into());
    s.add(None, NodeContent::table("T", vec![col, col2]))
        .unwrap();
    s.add(
        None,
        NodeContent::script(ScriptLanguage::Viscas, "fn h(u) {\n    return u + 1\n}"),
    )
    .unwrap();
    s.add(
        None,
        NodeContent::script(ScriptLanguage::Rhai, "let q = 1;\nq"),
    )
    .unwrap();
    s.add(None, NodeContent::note("# Title\n\nSome *markdown*."))
        .unwrap();
    s.add(None, NodeContent::viewport("Main")).unwrap();
    s.add(None, NodeContent::ticker("a -> a + 1", 100)).unwrap();
    s.add(
        None,
        NodeContent::regression("T.y_1 ~ m T.x_1 + c", &["m", "c"]),
    )
    .unwrap();
    s.set_layout(Some(json!({
        "workspace": "Graph",
        "areas": [{"editor": "viewport", "split": 0.7}, {"editor": "scene"}],
        "maximized": null,
    })));
    s
}

#[test]
fn roundtrip_preserves_everything() {
    let s = rich_scene();
    let text = s.save_to_string().unwrap();
    let t = Scene::load_from_str(&text).unwrap();
    assert_eq!(s.to_document(), t.to_document());
    assert_eq!(t.save_to_string().unwrap(), text, "stable output");
    assert_eq!(s.dfs_ids(), t.dfs_ids());
    for n in s.iter() {
        assert_eq!(Some(n), t.get(n.id));
    }
}

#[test]
fn roundtrip_rebuilds_graph_and_marks_dirty() {
    let s = rich_scene();
    let mut t = Scene::load_from_str(&s.save_to_string().unwrap()).unwrap();
    let b = t.lookup("b").unwrap();
    let a = t.lookup("a").unwrap();
    assert_eq!(t.graph().direct_upstream(b), vec![a]);
    assert_eq!(t.take_dirty().len(), t.len());
    assert!(!t.can_undo());
    assert!(t.selection().is_empty());
    assert!(!t.has_events());
}

#[test]
fn layout_blob_roundtrips() {
    let s = rich_scene();
    let t = Scene::load_from_str(&s.save_to_string().unwrap()).unwrap();
    assert_eq!(t.layout(), s.layout());
    assert_eq!(t.layout().unwrap()["areas"][0]["split"], json!(0.7));
}

#[test]
fn next_id_survives_so_ids_are_not_reused() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    s.remove(b).unwrap();
    let mut t = Scene::load_from_str(&s.save_to_string().unwrap()).unwrap();
    let c = var(&mut t, "c", "3");
    assert!(c > b);
    assert!(t.contains(a));
}

#[test]
fn output_is_readable_ron() {
    let mut s = Scene::new();
    var(&mut s, "a", "3");
    let text = s.save_to_string().unwrap();
    assert!(text.contains("version: 1"), "{text}");
    assert!(text.contains("name: \"a\""), "implicit Some: {text}");
    assert!(text.contains("Variable("), "{text}");
    assert!(!text.contains("visible"), "defaults omitted: {text}");
    assert!(!text.contains("style"), "defaults omitted: {text}");
    assert!(!text.contains("layout"), "{text}");
}

#[test]
fn hand_written_document_loads() {
    let text = r##"
// A hand-written document: comments, trailing commas, omitted defaults.
(
    version: 1,
    nodes: [
        (id: 10, name: "a", kind: Slider(value: "3", min: "0", max: "10")),
        (
            id: 11,
            name: "Plots",
            kind: Folder,
            children: [
                (id: 12, kind: Plot(relation: "y = a x"), style: (color: "#ff8800")),
            ],
        ),
        (id: 13, kind: Viewport()),
    ],
)
"##;
    let s = Scene::load_from_str(text).unwrap();
    let a = NodeId(10);
    let p = NodeId(12);
    assert_eq!(s.parent(p), Some(NodeId(11)));
    assert_eq!(s.graph().direct_upstream(p), vec![a]);
    assert_eq!(
        s.get(p).unwrap().style.color,
        Some(Color::rgb(0xff, 0x88, 0x00))
    );
    assert_eq!(s.get(p).unwrap().style.line_width, 2.0);
    match &s.get(NodeId(13)).unwrap().kind {
        NodeKind::Viewport { axes, bounds, .. } => {
            assert!(*axes);
            assert_eq!(*bounds, Bounds::default());
        }
        k => panic!("{k:?}"),
    }
    let mut s = s;
    let n = var(&mut s, "z", "1");
    assert_eq!(n, NodeId(14), "next_id defaults past the max id");
}

#[test]
fn explicit_some_is_accepted() {
    let text = r#"(version: 1, nodes: [(id: 1, name: Some("a"), kind: Variable(expr: "1"))])"#;
    let s = Scene::load_from_str(text).unwrap();
    assert_eq!(s.lookup("a"), Some(NodeId(1)));
}

#[test]
fn missing_version_is_an_error() {
    let text = r#"(nodes: [])"#;
    assert_eq!(
        Scene::load_from_str(text).unwrap_err(),
        DocumentError::MissingVersion
    );
}

#[test]
fn newer_version_is_refused() {
    let text = r#"(version: 99, nodes: [(id: 1, kind: FutureKind(x: 1))])"#;
    assert_eq!(
        Scene::load_from_str(text).unwrap_err(),
        DocumentError::TooNew { version: 99 }
    );
}

#[test]
fn unsupported_old_version() {
    assert_eq!(
        migrate(0, "(version: 0)").unwrap_err(),
        DocumentError::Unsupported { version: 0 }
    );
    assert!(Scene::load_from_str("(version: 0)").is_err());
}

#[test]
fn syntax_errors_are_reported() {
    let err = Scene::load_from_str("(version: 1, nodes: [").unwrap_err();
    assert!(matches!(err, DocumentError::Syntax(_)), "{err:?}");
    let err = Scene::load_from_str("not ron at all").unwrap_err();
    assert!(matches!(err, DocumentError::Syntax(_)), "{err:?}");
}

#[test]
fn unknown_kind_is_a_syntax_error() {
    let err = Scene::load_from_str(r#"(version: 1, nodes: [(id: 1, kind: Blob)])"#).unwrap_err();
    assert!(matches!(err, DocumentError::Syntax(_)), "{err:?}");
}

#[test]
fn duplicate_ids_are_rejected() {
    let text = r#"(version: 1, nodes: [
        (id: 1, kind: Folder, children: [(id: 1, kind: Folder)]),
    ])"#;
    assert_eq!(
        Scene::load_from_str(text).unwrap_err(),
        DocumentError::DuplicateId(NodeId(1))
    );
}

#[test]
fn duplicate_names_in_documents_load_with_diagnostics() {
    let text = r#"(version: 1, nodes: [
        (id: 1, name: "a", kind: Variable(expr: "1")),
        (id: 2, name: "a", kind: Variable(expr: "2")),
    ])"#;
    let s = Scene::load_from_str(text).unwrap();
    assert!(
        s.diagnostics(NodeId(1))
            .iter()
            .any(|d| matches!(d.kind, DiagnosticKind::DuplicateDefinition { .. }))
    );
}

#[test]
fn bad_color_is_rejected() {
    let text = r##"(version: 1, nodes: [(id: 1, kind: Folder, style: (color: "#12"))])"##;
    assert!(Scene::load_from_str(text).is_err());
}

#[test]
fn color_parse_and_display() {
    assert_eq!("#ff0000".parse::<Color>().unwrap(), Color::rgb(255, 0, 0));
    assert_eq!(
        "#00ff0080".parse::<Color>().unwrap(),
        Color::rgba(0, 255, 0, 128)
    );
    assert_eq!(Color::rgb(1, 2, 3).to_string(), "#010203");
    assert_eq!(Color::rgba(1, 2, 3, 4).to_string(), "#01020304");
    for bad in ["ff0000", "#ff00", "#gg0000", "#ff00000"] {
        assert!(bad.parse::<Color>().is_err(), "{bad}");
    }
}

#[test]
fn empty_scene_roundtrip() {
    let s = Scene::new();
    let text = s.save_to_string().unwrap();
    let t = Scene::load_from_str(&text).unwrap();
    assert!(t.is_empty());
    assert_eq!(t.layout(), None);
}

#[test]
fn document_struct_roundtrip() {
    let doc = rich_scene().to_document();
    let text = save_document(&doc).unwrap();
    assert_eq!(load_document(&text).unwrap(), doc);
    assert_eq!(doc.version, FORMAT_VERSION);
}

#[test]
fn unicode_and_escapes_survive() {
    let mut s = Scene::new();
    s.add(None, NodeContent::variable("θ_1", "√2 · π\t\"q\"\n"))
        .unwrap();
    s.add(None, NodeContent::note("multi\nline \\ back"))
        .unwrap();
    let t = Scene::load_from_str(&s.save_to_string().unwrap()).unwrap();
    assert_eq!(s.to_document(), t.to_document());
}
