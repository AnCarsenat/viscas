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

// ----- rich completions -----------------------------------------------------

#[test]
fn rich_items_carry_docs_and_snippets() {
    let e = engine();
    let set = e.completions("2 + lo", 6, Scope::Expression);
    assert_eq!(set.start, 4);
    assert_eq!(set.fragment, "lo");
    let log = set.items.iter().find(|c| c.text == "log").unwrap();
    assert_eq!(log.signature.as_deref(), Some("log(x, b)"));
    assert_eq!(log.signatures.len(), 2);
    assert!(log.doc.to_lowercase().contains("logarithm"));
    assert!(!log.examples.is_empty());
    assert_eq!(log.category, "elementary");
    // Two parameters: placeholders, the caret on the first.
    assert_eq!(log.snippet.text, "log(x, b)");
    assert_eq!(log.snippet.placeholders, vec![(4, 5), (7, 8)]);
    assert_eq!(log.snippet.cursor, 4);
    // One parameter: the caret goes inside the parentheses.
    let sin = e.completions("si", 2, Scope::Expression);
    let sin = sin.items.iter().find(|c| c.text == "sin").unwrap();
    assert_eq!(sin.snippet.text, "sin()");
    assert_eq!(sin.snippet.cursor, 4);
    assert!(sin.snippet.placeholders.is_empty());
    assert!(sin.is_function());
}

#[test]
fn completions_match_loosely() {
    let e = engine();
    let set = e.completions("sqt", 3, Scope::Expression);
    assert!(set.items.iter().any(|c| c.text == "sqrt"));
    let up = e.completions("SI", 2, Scope::Expression);
    assert!(up.items.iter().any(|c| c.text == "sin"));
    // The first letter must match.
    let set = e.completions("qrt", 3, Scope::Expression);
    assert!(!set.items.iter().any(|c| c.text == "sqrt"));
}

#[test]
fn commands_only_in_console_scope() {
    let e = engine();
    assert!(!e.completions(":he", 3, Scope::Console).items.is_empty());
    assert!(e.completions(":he", 3, Scope::Expression).items.is_empty());
    let help = e.completions(":he", 3, Scope::Console);
    assert_eq!(help.items[0].snippet.text, ":help ");
    // `:help` arguments complete names and commands.
    let topic = e.completions(":help mo", 8, Scope::Console);
    assert!(topic.items.iter().any(|c| c.text == "mode"));
}

#[test]
fn completions_skip_numbers_and_strings() {
    let e = engine();
    assert!(e.completions("12", 2, Scope::Console).items.is_empty());
    let set = e.completions("2si", 3, Scope::Console);
    assert_eq!(set.start, 1);
    assert!(set.items.iter().any(|c| c.text == "sin"));
    assert!(e.completions("\"si", 3, Scope::Console).items.is_empty());
}

#[test]
fn node_items_show_values_and_function_params() {
    let mut e = engine();
    var(&mut e, "alpha", "1/2");
    func(&mut e, "area", &["w", "h"], "w h");
    e.update();
    let set = e.completions("a", 1, Scope::Expression);
    let alpha = set.items.iter().find(|c| c.text == "alpha").unwrap();
    assert_eq!(alpha.kind, CompletionKind::Node);
    assert_eq!(alpha.detail, "1/2");
    assert!(alpha.doc.contains("1/2"));
    let area = set.items.iter().find(|c| c.text == "area").unwrap();
    assert_eq!(area.signature.as_deref(), Some("area(w, h)"));
    assert_eq!(area.snippet.text, "area(w, h)");
    // Console functions too.
    e.console_eval("sq(t) := t^2");
    let sq = e.completions("sq", 2, Scope::Console);
    let sq = sq.items.iter().find(|c| c.text == "sq").unwrap();
    assert_eq!(sq.kind, CompletionKind::Console);
    assert_eq!(sq.snippet.text, "sq()");
}

#[test]
fn describe_names() {
    let mut e = engine();
    var(&mut e, "k", "3");
    e.update();
    assert!(e.describe("sin").unwrap().doc.contains("ine"));
    assert_eq!(e.describe("k").unwrap().detail, "3");
    assert!(e.describe(":help").is_some());
    assert!(e.describe("x").is_some());
    assert!(e.describe("nope").is_none());
}

// ----- signature help ------------------------------------------------------

#[test]
fn call_at_finds_the_innermost_call() {
    let c = call_at("log(8, 2", 8).unwrap();
    assert_eq!((c.name.as_str(), c.arg, c.open), ("log", 1, 3));
    let c = call_at("max(1, sin(2", 12).unwrap();
    assert_eq!((c.name.as_str(), c.arg), ("sin", 0));
    let c = call_at("max(1, sin(2), ", 15).unwrap();
    assert_eq!((c.name.as_str(), c.arg), ("max", 2));
    // Brackets nest; commas inside them do not count.
    let c = call_at("f([1, 2], ", 10).unwrap();
    assert_eq!((c.name.as_str(), c.arg), ("f", 1));
    let c = call_at("f(1, [2, ", 9).unwrap();
    assert_eq!((c.name.as_str(), c.arg), ("f", 1));
    assert!(call_at("(1, 2", 5).is_none());
    assert!(call_at("2(", 2).is_none());
    assert!(call_at("sin(1)", 6).is_none());
    assert!(call_at("f(\"a,(\", ", 9).is_some_and(|c| c.arg == 1));
}

#[test]
fn signature_help_for_builtins() {
    let e = engine();
    let s = e.signature_at("2 + log(8, ", 11).unwrap();
    assert_eq!(s.name, "log");
    assert_eq!(s.active().label, "log(x, b)");
    assert_eq!(s.active_param, Some(1));
    let s = e.signature_at("log(", 4).unwrap();
    assert_eq!(s.active().label, "log(x)");
    assert_eq!(s.active_param, Some(0));
    let s = e.signature_at("gcd(1, 2, 3, ", 13).unwrap();
    assert_eq!(s.active_param, Some(1), "variadic repeats the last param");
    let s = e.signature_at("arcsin(", 7).unwrap();
    assert_eq!(s.active().label, "arcsin(x)");
    assert!(e.signature_at("nope(", 5).is_none());
    // Too many arguments: no parameter is active.
    let s = e.signature_at("sin(1, ", 7).unwrap();
    assert_eq!(s.active_param, None);
}

#[test]
fn signature_help_for_user_functions() {
    let mut e = engine();
    func(&mut e, "area", &["w", "h"], "w h");
    e.update();
    let s = e.signature_at("area(2, ", 8).unwrap();
    assert_eq!(s.active().params, ["w", "h"]);
    assert_eq!(s.active_param, Some(1));
    assert!(s.doc.contains("w h"));
    e.console_eval("g(a, b, c) := a + b + c");
    let s = e.signature_at("g(1, 2, ", 8).unwrap();
    assert_eq!(s.active().label, "g(a, b, c)");
    assert_eq!(s.active_param, Some(2));
}

// ----- diagnostics ----------------------------------------------------------

#[test]
fn diagnostics_report_parse_errors() {
    let e = engine();
    let d = e.diagnostics("2 + * 3", Scope::Console);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, Severity::Error);
    let d = e.diagnostics("sin(", Scope::Console);
    assert!(!d.is_empty());
    assert!(d[0].at_end("sin("));
    assert!(e.diagnostics("sin(x) + 1", Scope::Expression).is_empty());
    assert!(e.diagnostics("", Scope::Console).is_empty());
}

#[test]
fn diagnostics_flag_unknown_functions_with_fixes() {
    let e = engine();
    let line = "1 + sni(2)";
    let d = e.diagnostics(line, Scope::Console);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].span, viscas_core::syntax::Span { start: 4, end: 7 });
    assert!(d[0].message.contains("sni"));
    assert_eq!(d[0].suggestions[0], "sin");
    assert_eq!(d[0].apply(line, 0), Some(("1 + sin(2)".to_string(), 7)));
    assert_eq!(d[0].apply(line, 9), None);
}

#[test]
fn diagnostics_flag_likely_typos_as_warnings() {
    let e = engine();
    let d = e.diagnostics("2 sqr x", Scope::Expression);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
    assert!(d[0].suggestions.contains(&"sqrt".to_string()));
    // Short products of letters are fine.
    assert!(e.diagnostics("y = a b x", Scope::Expression).is_empty());
}

#[test]
fn diagnostics_know_bound_and_scene_names() {
    let mut e = engine();
    var(&mut e, "speed", "3");
    e.update();
    assert!(e.diagnostics("speed^2", Scope::Expression).is_empty());
    assert!(
        e.diagnostics("area(w, h) := w h", Scope::Expression)
            .is_empty()
    );
    assert!(
        e.diagnostics("[kk^2 for kk = [1..3]]", Scope::Console)
            .is_empty()
    );
    assert!(e.diagnostics("∫(0, 1) x^2 dx", Scope::Console).is_empty());
    assert!(e.diagnostics("ans + 1", Scope::Console).is_empty());
    e.console_eval("vel := 2");
    assert!(e.diagnostics("vel(1)", Scope::Console).is_empty());
    let d = e.diagnostics("f(x) = x^2", Scope::Expression);
    assert!(d.iter().any(|d| d.message.contains(":=")), "{d:?}");
}

#[test]
fn diagnostics_for_console_commands() {
    let e = engine();
    let d = e.diagnostics(":hlep sin", Scope::Console);
    assert_eq!(d[0].suggestions[0], ":help");
    assert_eq!(d[0].apply(":hlep sin", 0).unwrap().0, ":help sin");
    let d = e.diagnostics(":mode dge", Scope::Console);
    assert_eq!(d[0].span, viscas_core::syntax::Span { start: 6, end: 9 });
    assert_eq!(d[0].suggestions[0], "deg");
    assert!(e.diagnostics(":mode deg", Scope::Console).is_empty());
    assert!(e.diagnostics(":vars", Scope::Console).is_empty());
}

// ----- preview -------------------------------------------------------------

fn preview(e: &Engine, line: &str) -> Option<(String, String)> {
    let task = e.preview_task(line)?;
    std::thread::spawn(move || task.run(Default::default()))
        .join()
        .unwrap()
}

#[test]
fn preview_evaluates_pure_expressions() {
    let mut e = engine();
    assert_eq!(preview(&e, "1/4 + 1/4").unwrap().0, "1/2");
    var(&mut e, "a", "3");
    e.update();
    assert_eq!(preview(&e, "a^2").unwrap().0, "9");
    e.console_eval("b := 2");
    assert_eq!(preview(&e, "a b").unwrap().0, "6");
}

#[test]
fn preview_skips_non_pure_input() {
    let e = engine();
    for line in [
        "", "2", ":vars", "a := 3", "y = x^2", "x + 1", "zz + 1", "1; 2", "sin(",
    ] {
        assert!(e.preview_task(line).is_none(), "{line:?}");
    }
}

#[test]
fn preview_gives_up_on_expensive_input() {
    let e = engine();
    assert!(preview(&e, "sum([1..100000])").is_none());
    let task = e.preview_task("fact(3000)").unwrap();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    assert!(task.run(cancel).is_none(), "cancelled before it starts");
}
