//! Edge re-pointing (patch cables) and refactoring renames.

use super::*;

#[test]
fn repoint_rewrites_only_the_identifier() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    let c = var(&mut s, "c", "  a^2 +   3a  # keep");
    assert_eq!(s.repoint(c, "a", "b").unwrap(), 2);
    assert_eq!(main_src(&s, c), "  b^2 +   3b  # keep");
    assert_eq!(s.graph().direct_upstream(c), vec![b]);
}

#[test]
fn repoint_skips_bound_occurrences() {
    let mut s = Scene::new();
    var(&mut s, "k", "1");
    var(&mut s, "m", "1");
    let c = var(&mut s, "c", "k + ∑(k = 1, 5) k");
    assert_eq!(s.repoint(c, "k", "m").unwrap(), 1);
    assert_eq!(main_src(&s, c), "m + ∑(k = 1, 5) k");
}

#[test]
fn repoint_in_function_body_respects_params() {
    let mut s = Scene::new();
    var(&mut s, "x_0", "1");
    let f = func(&mut s, "f", &["x"], "x + a + a(x)");
    assert_eq!(s.repoint(f, "a", "x_0").unwrap(), 2);
    assert_eq!(main_src(&s, f), "x + x_0 + x_0(x)");
    assert_eq!(s.repoint(f, "x", "q").unwrap(), 0, "x is a parameter");
}

#[test]
fn repoint_with_longer_and_unicode_names() {
    let mut s = Scene::new();
    let c = var(&mut s, "c", "θ_1 + a·a");
    assert_eq!(s.repoint(c, "a", "alpha").unwrap(), 2);
    assert_eq!(main_src(&s, c), "θ_1 + alpha·alpha");
    assert_eq!(s.repoint(c, "θ_1", "β").unwrap(), 1);
    assert_eq!(main_src(&s, c), "β + alpha·alpha");
}

#[test]
fn repoint_split_name_spells_letters() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    var(&mut s, "b", "2");
    var(&mut s, "q", "3");
    let c = var(&mut s, "c", "2ab + 1");
    assert_eq!(s.repoint(c, "a", "q").unwrap(), 1);
    assert_eq!(main_src(&s, c), "2q b + 1");
}

#[test]
fn repoint_split_keeps_bound_letters() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    var(&mut s, "k", "1");
    let p = plot(&mut s, "y = ax");
    assert_eq!(s.repoint(p, "a", "k").unwrap(), 1);
    assert_eq!(main_src(&s, p), "y = k x");
}

#[test]
fn repoint_slider_and_table_fields() {
    let mut s = Scene::new();
    var(&mut s, "lo", "0");
    let sl = s
        .add(None, NodeContent::slider("s", "lo", "lo", "lo + 10"))
        .unwrap();
    assert_eq!(s.repoint(sl, "lo", "bottom").unwrap(), 3);
    match &s.get(sl).unwrap().kind {
        NodeKind::Slider {
            value, min, max, ..
        } => {
            assert_eq!(
                (value.as_str(), min.as_str(), max.as_str()),
                ("bottom", "bottom", "bottom + 10")
            );
        }
        k => panic!("{k:?}"),
    }
    let mut col = Column::new("y", &["a", "a + 1"]);
    col.formula = Some("2a".into());
    let t = s.add(None, NodeContent::table("T", vec![col])).unwrap();
    assert_eq!(s.repoint(t, "a", "z").unwrap(), 3);
    match &s.get(t).unwrap().kind {
        NodeKind::Table { columns } => {
            assert_eq!(columns[0].cells, vec!["z", "z + 1"]);
            assert_eq!(columns[0].formula.as_deref(), Some("2z"));
        }
        k => panic!("{k:?}"),
    }
}

#[test]
fn repoint_script_respects_scopes() {
    let mut s = Scene::new();
    let sc = s
        .add(
            None,
            NodeContent::script(
                ScriptLanguage::Viscas,
                "let y = a + 1\nfn g(a) {\n    return a * 2\n}\nprint(a)",
            ),
        )
        .unwrap();
    assert_eq!(s.repoint(sc, "a", "w").unwrap(), 2);
    assert_eq!(
        main_src(&s, sc),
        "let y = w + 1\nfn g(a) {\n    return a * 2\n}\nprint(w)"
    );
}

#[test]
fn repoint_rejects_invalid_target() {
    let mut s = Scene::new();
    let c = var(&mut s, "c", "a");
    assert_eq!(
        s.repoint(c, "a", "1x"),
        Err(SceneError::InvalidName("1x".into()))
    );
    assert_eq!(main_src(&s, c), "a");
}

#[test]
fn repoint_without_matches_records_nothing() {
    let mut s = Scene::new();
    let c = var(&mut s, "c", "a");
    let depth = s.undo_depth();
    assert_eq!(s.repoint(c, "zz", "q").unwrap(), 0);
    assert_eq!(s.undo_depth(), depth);
}

#[test]
fn repoint_edge_between_nodes() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    let p = plot(&mut s, "y = a x");
    assert_eq!(s.repoint_edge(p, a, b).unwrap(), 1);
    assert_eq!(main_src(&s, p), "y = b x");
    assert_eq!(s.graph().direct_upstream(p), vec![b]);
    let note = s.add(None, NodeContent::note("hi")).unwrap();
    assert!(s.repoint_edge(p, note, a).is_err());
}

#[test]
fn rename_refactor_updates_dependents() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + 1");
    let f = func(&mut s, "f", &["a"], "a + 1");
    let p = plot(&mut s, "y = a x");
    s.rename_refactor(a, "speed").unwrap();
    assert_eq!(s.get(a).unwrap().name.as_deref(), Some("speed"));
    assert_eq!(main_src(&s, b), "speed + 1");
    assert_eq!(main_src(&s, f), "a + 1", "parameter untouched");
    assert_eq!(main_src(&s, p), "y = speed x");
    assert_eq!(s.downstream(a), vec![b, p]);
}

#[test]
fn rename_refactor_validates_first() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    var(&mut s, "b", "a");
    assert_eq!(
        s.rename_refactor(a, "b"),
        Err(SceneError::DuplicateName("b".into()))
    );
    assert_eq!(s.lookup("a"), Some(a));
}

#[test]
fn rename_refactor_on_label_just_renames() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    s.rename_refactor(f, "Shapes").unwrap();
    assert_eq!(s.get(f).unwrap().name.as_deref(), Some("Shapes"));
}

#[test]
fn rename_refactor_recursive_function() {
    let mut s = Scene::new();
    let f = func(&mut s, "f", &["n"], "{n <= 1: 1, n f(n - 1)}");
    let u = var(&mut s, "u", "f(5)");
    s.rename_refactor(f, "fact").unwrap();
    assert_eq!(main_src(&s, u), "fact(5)");
    assert_eq!(main_src(&s, f), "{n <= 1: 1, n fact(n - 1)}");
    assert!(s.diagnostics(f).is_empty());
}
