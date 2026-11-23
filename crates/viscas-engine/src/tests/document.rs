//! Document I/O through the engine.

use viscas_scene::{NodeContent, SceneEvent};

use super::*;

fn sample(e: &mut Engine) {
    var(e, "a", "2");
    func(e, "f", &["x"], "a x");
    plot(e, "y = f(x)");
}

#[test]
fn roundtrip_preserves_nodes_and_layout_blob() {
    let mut e = engine();
    sample(&mut e);
    let blob = serde_json::json!({"areas": [{"editor": "viewport", "split": 0.5}], "v": 3});
    e.set_layout(Some(blob.clone()));
    let text = e.save_string().unwrap();

    let mut f = engine();
    f.open_str(&text).unwrap();
    assert_eq!(f.layout(), Some(&blob));
    assert_eq!(f.scene().len(), 3);
    let a = f.scene().lookup("a").unwrap();
    assert_eq!(exact(&f, a), "2");
    assert_eq!(first_value(&mut f, "f(4)"), "8");
    let p = f.scene().find_kind(viscas_scene::KindTag::Plot)[0];
    assert!(f.plot_of(p).is_some(), "plots are compiled after loading");
}

fn first_value(e: &mut Engine, src: &str) -> String {
    e.console_eval(src).results[0].exact.clone()
}

#[test]
fn opened_document_has_clean_history() {
    let mut e = engine();
    sample(&mut e);
    let text = e.save_string().unwrap();
    let mut f = engine();
    f.open_str(&text).unwrap();
    assert!(!f.scene().can_undo());
}

#[test]
fn open_emits_refresh_events() {
    let mut e = engine();
    sample(&mut e);
    let text = e.save_string().unwrap();
    e.drain_events();
    e.open_str(&text).unwrap();
    let ev = e.drain_events();
    assert!(ev.contains(&SceneEvent::GraphChanged {
        touched: Vec::new()
    }));
}

#[test]
fn invalid_document_leaves_scene_untouched() {
    let mut e = engine();
    sample(&mut e);
    assert!(e.open_str("this is not a document").is_err());
    assert_eq!(e.scene().len(), 3);
}

#[test]
fn new_document_clears_everything() {
    let mut e = engine();
    sample(&mut e);
    e.set_layout(Some(serde_json::json!(1)));
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    e.new_document();
    assert!(e.scene().is_empty());
    assert!(e.layout().is_none());
    assert!(e.layers(vp).is_empty());
    assert!(e.evaluator().lookup("a").is_none());
    assert!(e.console_eval("a").error.is_some());
}

#[test]
fn save_and_open_files() {
    let dir = std::env::temp_dir().join(format!("viscas-engine-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.viscas");
    let mut e = engine();
    sample(&mut e);
    e.set_layout(Some(serde_json::json!({"k": "v"})));
    e.save_path(&path).unwrap();
    assert_eq!(e.document_path(), Some(path.as_path()));

    let mut f = engine();
    f.open_path(&path).unwrap();
    assert_eq!(f.document_path(), Some(path.as_path()));
    assert_eq!(f.layout(), Some(&serde_json::json!({"k": "v"})));
    assert_eq!(f.scene().len(), 3);
    assert!(f.open_path(dir.join("missing.viscas")).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn colors_survive_roundtrip() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    let c = e.scene().get(p).unwrap().style.color;
    let text = e.save_string().unwrap();
    let mut f = engine();
    f.open_str(&text).unwrap();
    assert_eq!(f.scene().get(p).unwrap().style.color, c);
}

#[test]
fn errors_survive_roundtrip() {
    let mut e = engine();
    let b = e.add_node(None, NodeContent::variable("b", "1/0")).unwrap();
    let text = e.save_string().unwrap();
    let mut f = engine();
    f.open_str(&text).unwrap();
    assert!(f.scene().status(b).is_error());
}
