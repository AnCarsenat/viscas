//! Completion and suggestions.

use super::*;

fn texts(c: &[Completion]) -> Vec<&str> {
    c.iter().map(|c| c.text.as_str()).collect()
}

#[test]
fn completes_builtins() {
    let e = engine();
    let c = e.complete("si");
    let t = texts(&c);
    assert_eq!(t[0], "sin");
    assert!(t.contains(&"sinh") && t.contains(&"sign"));
    assert_eq!(c[0].kind, CompletionKind::Builtin);
    assert!(c[0].detail.contains("sin(x)"));
}

#[test]
fn nodes_rank_first() {
    let mut e = engine();
    var(&mut e, "sigma", "2");
    let c = e.complete("si");
    assert_eq!(c[0].text, "sigma");
    assert_eq!(c[0].kind, CompletionKind::Node);
    assert_eq!(c[0].detail, "2");
}

#[test]
fn completes_console_names() {
    let mut e = engine();
    e.console_eval("speed := 3");
    let c = e.complete("spe");
    assert_eq!(texts(&c), vec!["speed"]);
    assert_eq!(c[0].kind, CompletionKind::Console);
}

#[test]
fn uses_trailing_identifier_of_line() {
    let e = engine();
    let t: Vec<String> = e.complete("2 + co").into_iter().map(|c| c.text).collect();
    assert!(t.contains(&"cos".to_string()) && t.contains(&"coth".to_string()));
    assert!(t.iter().all(|s| s.starts_with("co")));
}

#[test]
fn completes_constants_and_aliases() {
    let e = engine();
    let c = e.complete("p");
    let t = texts(&c);
    for want in ["pi", "phi", "perm", "prod"] {
        assert!(t.contains(&want), "{want} missing from {t:?}");
    }
    let pi = c.iter().find(|c| c.text == "pi").unwrap();
    assert_eq!(pi.kind, CompletionKind::Constant);
    assert!(e.complete("arcs").iter().any(|c| c.text == "arcsin"));
}

#[test]
fn completes_commands() {
    let e = engine();
    let c = e.complete(":pi");
    assert_eq!(texts(&c), vec![":pin"]);
    assert_eq!(c[0].kind, CompletionKind::Command);
    assert_eq!(e.complete(":").len(), help::COMMAND_HELP.len());
}

#[test]
fn empty_fragment_has_no_completions() {
    let e = engine();
    assert!(e.complete("").is_empty());
    assert!(e.complete("2 + ").is_empty());
}

#[test]
fn completion_start_offsets() {
    assert_eq!(completion_start("2 + co"), 4);
    assert_eq!(completion_start("abc"), 0);
    assert_eq!(completion_start(":pi"), 0);
    assert_eq!(completion_start("x_1"), 0);
    assert_eq!(completion_start("f("), 2);
}

#[test]
fn suggest_close_names() {
    let cands: Vec<String> = ["sin", "sinh", "cos", "alpha"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(suggest("sni", &cands), vec!["sin".to_string()]);
    assert_eq!(suggest("alpah", &cands), vec!["alpha".to_string()]);
    assert!(suggest("zzzzzz", &cands).is_empty());
    assert!(suggest("", &cands).is_empty());
}

#[test]
fn help_tables_cover_every_builtin() {
    for b in viscas_core::eval::BUILTINS {
        assert!(
            help::FUNCTION_HELP.iter().any(|h| h.name == b.name),
            "missing help for {}",
            b.name
        );
    }
    assert!(help::function_help("arctan").is_some());
    assert!(help::constant_help("tau").is_some());
    assert!(help::command_help("vars").is_some());
    assert!(help::command_help(":vars").is_some());
    assert!(help::function_help("nope").is_none());
}
