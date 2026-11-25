//! Table-driven parser tests. Structure is checked through a compact
//! S-expression rendering; every successful case is also checked for the
//! round-trip property `parse(print(ast)) == ast` (ignoring spans).

use super::*;

// ---------------------------------------------------------------------------
// S-expression rendering
// ---------------------------------------------------------------------------

fn sx(e: &Expr) -> String {
    let list = |head: &str, items: Vec<String>| {
        let mut s = format!("({head}");
        for i in items {
            s.push(' ');
            s.push_str(&i);
        }
        s.push(')');
        s
    };
    match &*e.kind {
        ExprKind::Number { text, imaginary } => {
            format!("{text}{}", if *imaginary { "i" } else { "" })
        }
        ExprKind::Str(s) => format!("{s:?}"),
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Prime { expr, order } => list("prime", vec![sx(expr), order.to_string()]),
        ExprKind::Unary { op, expr } => {
            let h = match op {
                UnaryOp::Neg => "neg",
                UnaryOp::Pos => "pos",
                UnaryOp::Not => "not",
            };
            list(h, vec![sx(expr)])
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let h = match op {
                BinOp::Mod => "mod",
                BinOp::Pow => "^",
                BinOp::To => "to",
            };
            list(h, vec![sx(lhs), sx(rhs)])
        }
        ExprKind::Add(terms) => {
            if terms.iter().all(|t| t.op == AddOp::Add) {
                list("+", terms.iter().map(|t| sx(&t.expr)).collect())
            } else if terms.len() == 2 {
                list("-", vec![sx(&terms[0].expr), sx(&terms[1].expr)])
            } else {
                let items = terms
                    .iter()
                    .enumerate()
                    .map(|(i, t)| match (i, t.op) {
                        (0, _) => sx(&t.expr),
                        (_, AddOp::Add) => format!("+{}", sx(&t.expr)),
                        (_, AddOp::Sub) => format!("-{}", sx(&t.expr)),
                    })
                    .collect();
                list("±", items)
            }
        }
        ExprKind::Mul(factors) => {
            if factors.iter().all(|f| f.op == MulOp::Mul) {
                list("*", factors.iter().map(|f| sx(&f.expr)).collect())
            } else if factors.len() == 2 {
                list("/", vec![sx(&factors[0].expr), sx(&factors[1].expr)])
            } else {
                let items = factors
                    .iter()
                    .enumerate()
                    .map(|(i, f)| match (i, f.op) {
                        (0, _) => sx(&f.expr),
                        (_, MulOp::Mul) => format!("*{}", sx(&f.expr)),
                        (_, MulOp::Div) => format!("/{}", sx(&f.expr)),
                    })
                    .collect();
                list("*/", items)
            }
        }
        ExprKind::And(items) => list("and", items.iter().map(sx).collect()),
        ExprKind::Or(items) => list("or", items.iter().map(sx).collect()),
        ExprKind::Postfix { op, expr } => {
            let h = match op {
                PostfixOp::Factorial => "!",
                PostfixOp::Percent => "%",
            };
            list(h, vec![sx(expr)])
        }
        ExprKind::Compare { first, rest } if rest.len() == 1 => {
            list(rest[0].0.as_str(), vec![sx(first), sx(&rest[0].1)])
        }
        ExprKind::Compare { first, rest } => {
            let mut items = vec![sx(first)];
            for (op, e) in rest {
                items.push(op.as_str().to_string());
                items.push(sx(e));
            }
            list("cmp", items)
        }
        ExprKind::Call { callee, args } => {
            let mut items = vec![sx(callee)];
            items.extend(args.iter().map(sx));
            list("call", items)
        }
        ExprKind::Member { expr, name } => list(".", vec![sx(expr), name.clone()]),
        ExprKind::Index { expr, indices } => {
            let mut items = vec![sx(expr)];
            items.extend(indices.iter().map(sx));
            list("index", items)
        }
        ExprKind::List(items) => list("list", items.iter().map(sx).collect()),
        ExprKind::Tuple(items) => list("tuple", items.iter().map(sx).collect()),
        ExprKind::Range { start, second, end } => {
            let mut items = vec![sx(start)];
            if let Some(s) = second {
                items.push(sx(s));
            }
            items.push(sx(end));
            list("range", items)
        }
        ExprKind::Comprehension { body, bindings } => {
            let mut items = vec![sx(body)];
            for b in bindings {
                items.push(format!("({} {})", b.name, sx(&b.iter)));
            }
            list("for", items)
        }
        ExprKind::Piecewise {
            branches,
            otherwise,
        } => {
            let mut items: Vec<String> = branches
                .iter()
                .map(|b| match &b.value {
                    Some(v) => format!("({} {})", sx(&b.cond), sx(v)),
                    None => format!("({})", sx(&b.cond)),
                })
                .collect();
            if let Some(o) = otherwise {
                items.push(format!("(else {})", sx(o)));
            }
            list("piecewise", items)
        }
        ExprKind::Restrict { expr, conds } => {
            let mut items = vec![sx(expr)];
            items.extend(conds.iter().map(sx));
            list("restrict", items)
        }
        ExprKind::Derivative { var, order, expr } => {
            let h = if *order == 1 {
                format!("d/d{var}")
            } else {
                format!("d{order}/d{var}")
            };
            list(&h, vec![sx(expr)])
        }
        ExprKind::Integral { bounds, body, var } => {
            let mut items = vec![var.clone()];
            if let Some((lo, hi)) = bounds {
                items.push(sx(lo));
                items.push(sx(hi));
            }
            items.push(sx(body));
            list("int", items)
        }
        ExprKind::BigOp {
            op,
            var,
            from,
            to,
            body,
        } => {
            let h = match op {
                BigOpKind::Sum => "sum",
                BigOpKind::Product => "prod",
            };
            list(h, vec![var.clone(), sx(from), sx(to), sx(body)])
        }
    }
}

fn sx_block(b: &Block) -> String {
    let mut s = String::from("(block");
    for st in &b.stmts {
        s.push(' ');
        s.push_str(&sx_stmt(st));
    }
    s.push(')');
    s
}

/// Statement restrictions, rendered as ` {c1 c2}` (empty when none).
fn sx_conds(conds: &[Expr]) -> String {
    if conds.is_empty() {
        return String::new();
    }
    let items: Vec<String> = conds.iter().map(sx).collect();
    format!(" {{{}}}", items.join(" "))
}

fn sx_stmt(s: &Stmt) -> String {
    match &s.kind {
        StmtKind::Expr { expr, restrictions } if restrictions.is_empty() => sx(expr),
        StmtKind::Expr { expr, restrictions } => {
            format!("(where {}{})", sx(expr), sx_conds(restrictions))
        }
        StmtKind::Equation {
            lhs,
            rhs,
            restrictions,
        } => format!("(= {} {}{})", sx(lhs), sx(rhs), sx_conds(restrictions)),
        StmtKind::Definition {
            lhs,
            rhs,
            restrictions,
            ..
        } => format!("(:= {} {}{})", sx(lhs), sx(rhs), sx_conds(restrictions)),
        StmtKind::Regression {
            lhs,
            rhs,
            restrictions,
        } => format!("(~ {} {}{})", sx(lhs), sx(rhs), sx_conds(restrictions)),
        StmtKind::Action { actions } => {
            let parts: Vec<String> = actions
                .iter()
                .map(|(t, v)| format!("(-> {} {})", sx(t), sx(v)))
                .collect();
            if parts.len() == 1 {
                parts[0].clone()
            } else {
                format!("(actions {})", parts.join(" "))
            }
        }
        StmtKind::Let { name, value } => format!("(let {name} {})", sx(value)),
        StmtKind::If {
            cond,
            then,
            otherwise,
        } => {
            let mut out = format!("(if {} {}", sx(cond), sx_block(then));
            match otherwise {
                Some(ElseBranch::Block(b)) => {
                    out.push(' ');
                    out.push_str(&sx_block(b));
                }
                Some(ElseBranch::If(s)) => {
                    out.push(' ');
                    out.push_str(&sx_stmt(s));
                }
                None => {}
            }
            out.push(')');
            out
        }
        StmtKind::For { var, iter, body } => {
            format!("(for {var} {} {})", sx(iter), sx_block(body))
        }
        StmtKind::While { cond, body } => format!("(while {} {})", sx(cond), sx_block(body)),
        StmtKind::Fn { name, params, body } => {
            format!("(fn {name} ({}) {})", params.join(" "), sx_block(body))
        }
        StmtKind::Return(None) => "(return)".into(),
        StmtKind::Return(Some(e)) => format!("(return {})", sx(e)),
    }
}

fn sx_program(p: &Program) -> String {
    p.stmts.iter().map(sx_stmt).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fmt_errors(src: &str, errs: &[ParseError]) -> String {
    errs.iter()
        .map(|e| e.render(src))
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn expr_ok(src: &str) -> Expr {
    match parse_expr(src) {
        Ok(e) => e,
        Err(errs) => panic!("parse_expr({src:?}) failed:\n{}", fmt_errors(src, &errs)),
    }
}

#[track_caller]
fn stmt_ok(src: &str) -> Stmt {
    match parse_statement(src) {
        Ok(s) => s,
        Err(errs) => panic!(
            "parse_statement({src:?}) failed:\n{}",
            fmt_errors(src, &errs)
        ),
    }
}

#[track_caller]
fn program_ok(src: &str) -> Program {
    match parse_program(src) {
        Ok(p) => p,
        Err(errs) => panic!("parse_program({src:?}) failed:\n{}", fmt_errors(src, &errs)),
    }
}

#[track_caller]
fn roundtrip_expr(src: &str, e: &Expr) {
    let printed = e.to_string();
    let again = parse_expr(&printed).unwrap_or_else(|errs| {
        panic!(
            "re-parse of {printed:?} (from {src:?}) failed:\n{}",
            fmt_errors(&printed, &errs)
        )
    });
    assert_eq!(
        again.without_spans(),
        e.without_spans(),
        "round trip changed the tree: {src:?} -> {printed:?}"
    );
    assert_eq!(again.to_string(), printed, "printing is not idempotent");
}

#[track_caller]
fn roundtrip_stmt(src: &str, s: &Stmt) {
    let printed = s.to_string();
    let again = parse_statement(&printed).unwrap_or_else(|errs| {
        panic!(
            "re-parse of {printed:?} (from {src:?}) failed:\n{}",
            fmt_errors(&printed, &errs)
        )
    });
    assert_eq!(
        again.without_spans(),
        s.without_spans(),
        "round trip changed the tree: {src:?} -> {printed:?}"
    );
}

#[track_caller]
fn roundtrip_program(src: &str, p: &Program) {
    let printed = p.to_string();
    let again = parse_program(&printed).unwrap_or_else(|errs| {
        panic!(
            "re-parse of {printed:?} (from {src:?}) failed:\n{}",
            fmt_errors(&printed, &errs)
        )
    });
    assert_eq!(
        again.without_spans(),
        p.without_spans(),
        "round trip changed the program: {src:?} -> {printed:?}"
    );
}

/// Each row: (source, expected S-expression).
#[track_caller]
fn check_exprs(cases: &[(&str, &str)]) {
    for (src, want) in cases {
        let e = expr_ok(src);
        assert_eq!(sx(&e), *want, "parse_expr({src:?})");
        roundtrip_expr(src, &e);
    }
}

#[track_caller]
fn check_stmts(cases: &[(&str, &str)]) {
    for (src, want) in cases {
        let s = stmt_ok(src);
        assert_eq!(sx_stmt(&s), *want, "parse_statement({src:?})");
        roundtrip_stmt(src, &s);
    }
}

#[track_caller]
fn check_programs(cases: &[(&str, &str)]) {
    for (src, want) in cases {
        let p = program_ok(src);
        assert_eq!(sx_program(&p), *want, "parse_program({src:?})");
        roundtrip_program(src, &p);
    }
}

/// Each row: (source, number of errors, substring of the first message).
#[track_caller]
fn check_expr_errors(cases: &[(&str, usize, &str)]) {
    for (src, count, needle) in cases {
        let errs = match parse_expr(src) {
            Ok(e) => panic!("parse_expr({src:?}) unexpectedly succeeded: {}", sx(&e)),
            Err(errs) => errs,
        };
        assert_eq!(errs.len(), *count, "error count for {src:?}: {errs:?}");
        assert!(
            errs[0].message.contains(needle),
            "first error for {src:?} was {:?}, expected it to contain {needle:?}",
            errs[0].message
        );
        for e in &errs {
            assert!(e.span.end <= src.len(), "span out of range for {src:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

fn kinds(src: &str) -> Vec<TokenKind> {
    let (toks, errs) = lexer::lex(src);
    assert!(errs.is_empty(), "lex errors for {src:?}: {errs:?}");
    toks.into_iter().map(|t| t.kind).collect()
}

fn num(text: &str, imaginary: bool) -> TokenKind {
    TokenKind::Number {
        text: text.into(),
        imaginary,
    }
}

fn id(name: &str) -> TokenKind {
    TokenKind::Ident(name.into())
}

#[test]
fn lex_numbers() {
    use TokenKind::*;
    let cases: Vec<(&str, Vec<TokenKind>)> = vec![
        ("42", vec![num("42", false), Eof]),
        ("3.14", vec![num("3.14", false), Eof]),
        (".5", vec![num(".5", false), Eof]),
        ("1e5", vec![num("1e5", false), Eof]),
        ("1.2E-3", vec![num("1.2E-3", false), Eof]),
        ("6.02e+23", vec![num("6.02e+23", false), Eof]),
        ("3i", vec![num("3", true), Eof]),
        ("2.5i", vec![num("2.5", true), Eof]),
        ("1e3i", vec![num("1e3", true), Eof]),
        ("2e", vec![num("2", false), id("e"), Eof]),
        ("2ex", vec![num("2", false), id("ex"), Eof]),
        ("3in", vec![num("3", false), id("in"), Eof]),
        (
            "1..10",
            vec![num("1", false), DotDot, num("10", false), Eof],
        ),
        (
            "1...10",
            vec![num("1", false), DotDot, num("10", false), Eof],
        ),
        ("2x", vec![num("2", false), id("x"), Eof]),
    ];
    for (src, want) in cases {
        assert_eq!(kinds(src), want, "lex({src:?})");
    }
}

#[test]
fn lex_identifiers_and_symbols() {
    use TokenKind::*;
    let cases: Vec<(&str, Vec<TokenKind>)> = vec![
        ("x_1", vec![id("x_1"), Eof]),
        ("y_{12}", vec![id("y_12"), Eof]),
        ("x1", vec![id("x1"), Eof]),
        ("θ_1", vec![id("θ_1"), Eof]),
        ("dθ", vec![id("dθ"), Eof]),
        ("2πr", vec![num("2", false), id("π"), id("r"), Eof]),
        ("πx", vec![id("π"), id("x"), Eof]),
        ("f''", vec![id("f"), Prime, Prime, Eof]),
        (
            "a≤b≥c≠d",
            vec![id("a"), Le, id("b"), Ge, id("c"), NotEq, id("d"), Eof],
        ),
        (
            "a·b×c÷d−e",
            vec![
                id("a"),
                Star,
                id("b"),
                Star,
                id("c"),
                Slash,
                id("d"),
                Minus,
                id("e"),
                Eof,
            ],
        ),
        ("a->b→c", vec![id("a"), Arrow, id("b"), Arrow, id("c"), Eof]),
        ("x**2", vec![id("x"), Caret, num("2", false), Eof]),
        (
            "x mod y",
            vec![id("x"), Keyword(token::Keyword::Mod), id("y"), Eof],
        ),
        ("a # comment\nb", vec![id("a"), Newline, id("b"), Eof]),
        ("\"hi\\n\"", vec![Str("hi\n".into()), Eof]),
        (
            "√∛∫∑∏∞",
            vec![Sqrt, Cbrt, Integral, Sum, Product, id("∞"), Eof],
        ),
        ("a==b!=c", vec![id("a"), EqEq, id("b"), NotEq, id("c"), Eof]),
    ];
    for (src, want) in cases {
        assert_eq!(kinds(src), want, "lex({src:?})");
    }
}

#[test]
fn lex_spans_are_byte_ranges() {
    let (toks, _) = lexer::lex("π + x_{12}");
    let spans: Vec<(usize, usize)> = toks.iter().map(|t| (t.span.start, t.span.end)).collect();
    assert_eq!(spans, vec![(0, 2), (3, 4), (5, 11), (11, 11)]);
}

#[test]
fn lex_errors() {
    let cases = [
        ("a $ b", "unexpected character `$`"),
        ("x_", "expected subscript"),
        ("x_{1", "unterminated subscript"),
        ("\"abc", "unterminated string"),
    ];
    for (src, needle) in cases {
        let (_, errs) = lexer::lex(src);
        assert_eq!(errs.len(), 1, "{src:?}: {errs:?}");
        assert!(errs[0].message.contains(needle), "{src:?}: {errs:?}");
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[test]
fn atoms() {
    check_exprs(&[
        ("42", "42"),
        ("1.5e-3", "1.5e-3"),
        ("3i", "3i"),
        ("x", "x"),
        ("x_{12}", "x_12"),
        ("θ", "θ"),
        ("\"text\"", "\"text\""),
        ("(x)", "x"),
        ("((x))", "x"),
    ]);
}

#[test]
fn precedence_and_associativity() {
    check_exprs(&[
        ("1 + 2 * 3", "(+ 1 (* 2 3))"),
        ("(1 + 2) * 3", "(* (+ 1 2) 3)"),
        ("1 - 2 - 3", "(± 1 -2 -3)"),
        ("(1 - 2) - 3", "(- (- 1 2) 3)"),
        ("a + b - c + d", "(± a +b -c +d)"),
        ("1 - (2 - 3)", "(- 1 (- 2 3))"),
        ("8 / 4 / 2", "(*/ 8 /4 /2)"),
        ("a / b * c", "(*/ a /b *c)"),
        ("a * b mod c * d", "(* (mod (* a b) c) d)"),
        ("a mod b mod c", "(mod (mod a b) c)"),
        ("2^3^2", "(^ 2 (^ 3 2))"),
        ("(2^3)^2", "(^ (^ 2 3) 2)"),
        ("-x^2", "(neg (^ x 2))"),
        ("(-x)^2", "(^ (neg x) 2)"),
        ("2^-3", "(^ 2 (neg 3))"),
        ("-2^-x^2", "(neg (^ 2 (neg (^ x 2))))"),
        ("--x", "(neg (neg x))"),
        ("+x", "(pos x)"),
        ("a mod b * c", "(* (mod a b) c)"),
        ("a + b mod c", "(+ a (mod b c))"),
        ("3!", "(! 3)"),
        ("-3!", "(neg (! 3))"),
        ("x^2!", "(^ x (! 2))"),
        ("50%", "(% 50)"),
        ("x * -y", "(* x (neg y))"),
        ("a − b", "(- a b)"),
        ("a · b × c ÷ d", "(*/ a *b *c /d)"),
        ("1 +\n 2", "(+ 1 2)"),
        ("x**2", "(^ x 2)"),
    ]);
}

#[test]
fn logic_and_comparisons() {
    check_exprs(&[
        ("x < 1", "(< x 1)"),
        ("x <= 1", "(<= x 1)"),
        ("x ≤ 1", "(<= x 1)"),
        ("x ≥ 1", "(>= x 1)"),
        ("x ≠ 1", "(!= x 1)"),
        ("x == 1", "(== x 1)"),
        ("0 < x < 2", "(cmp 0 < x < 2)"),
        ("0 <= x < y + 1 <= 5", "(cmp 0 <= x < (+ y 1) <= 5)"),
        ("a < b and c > d", "(and (< a b) (> c d))"),
        ("a or b and c", "(or a (and b c))"),
        ("not a and b", "(and (not a) b)"),
        ("not x < 3", "(not (< x 3))"),
        ("not not a", "(not (not a))"),
        ("(a or b) and c", "(and (or a b) c)"),
        ("(x < 1) < 2", "(< (< x 1) 2)"),
    ]);
}

#[test]
fn implicit_multiplication() {
    check_exprs(&[
        ("2x", "(* 2 x)"),
        ("2 x", "(* 2 x)"),
        ("x y", "(* x y)"),
        ("xy", "xy"),
        ("2(x+1)", "(* 2 (+ x 1))"),
        ("(a)(b)", "(* a b)"),
        ("(a + 1)(a - 1)", "(* (+ a 1) (- a 1))"),
        ("2πr", "(* 2 π r)"),
        ("x 2", "(* x 2)"),
        ("1/2x", "(/ 1 (* 2 x))"),
        ("1/2 x", "(/ 1 (* 2 x))"),
        ("1/2 sin x", "(/ 1 (* 2 (call sin x)))"),
        ("2x/3y", "(*/ 2 *x /(* 3 y))"),
        ("1/2 * x", "(*/ 1 /2 *x)"),
        ("(1/2)x", "(* (/ 1 2) x)"),
        ("-2x", "(* (neg 2) x)"),
        ("2x^2", "(* 2 (^ x 2))"),
        ("2x!", "(* 2 (! x))"),
        ("3i x", "(* 3i x)"),
        ("2[1, 2]", "(* 2 (list 1 2))"),
        ("2√x", "(* 2 (call sqrt x))"),
        ("|x|", "(call abs x)"),
        ("2|x|", "(* 2 (call abs x))"),
        ("||x| - 1|", "(call abs (- (call abs x) 1))"),
        ("|x + |y||", "(call abs (+ x (call abs y)))"),
        ("|a|b|c|", "(* (call abs a) b (call abs c))"),
        ("|(a, b)|", "(call abs (tuple a b))"),
        ("sin |x|", "(call sin (call abs x))"),
        ("a b c", "(* a b c)"),
        ("2 f(x)", "(* 2 (call f x))"),
        ("f(x)(y)", "(* (call f x) y)"),
    ]);
}

#[test]
fn prefix_function_application() {
    check_exprs(&[
        ("sin x", "(call sin x)"),
        ("sin(x)", "(call sin x)"),
        ("sin 2x", "(call sin (* 2 x))"),
        ("sin x^2", "(call sin (^ x 2))"),
        ("sin x cos x", "(* (call sin x) (call cos x))"),
        ("2 sin x", "(* 2 (call sin x))"),
        ("2 sin x cos x", "(* 2 (call sin x) (call cos x))"),
        ("sin x + 1", "(+ (call sin x) 1)"),
        ("sin x / 2", "(/ (call sin x) 2)"),
        ("sin x y", "(call sin (* x y))"),
        ("sin cos x", "(call sin (call cos x))"),
        ("sin 2πx", "(call sin (* 2 π x))"),
        ("sin -x", "(call sin (neg x))"),
        ("sin -2x", "(call sin (* (neg 2) x))"),
        ("sin - x", "(call sin (neg x))"),
        ("sin(x)^2", "(^ (call sin x) 2)"),
        ("sin^2 x", "(^ (call sin x) 2)"),
        ("sin^2(x)", "(^ (call sin x) 2)"),
        ("sin^-1 x", "(call arcsin x)"),
        ("sin^2", "(^ sin 2)"),
        ("ln 2x", "(call ln (* 2 x))"),
        ("log_2 8", "(call log_2 8)"),
        ("exp x", "(call exp x)"),
        ("sin x!", "(call sin (! x))"),
        ("sin", "sin"),
        ("sin * x", "(* sin x)"),
        ("cos(x) sin(x)", "(* (call cos x) (call sin x))"),
        ("√2x", "(call sqrt (* 2 x))"),
        ("√x y", "(call sqrt (* x y))"),
        ("√x sin x", "(* (call sqrt x) (call sin x))"),
        ("√x + 1", "(+ (call sqrt x) 1)"),
        ("√-x", "(call sqrt (neg x))"),
        ("√(x+1)", "(call sqrt (+ x 1))"),
        ("√x^2", "(call sqrt (^ x 2))"),
        ("∛8", "(call cbrt 8)"),
        ("√-1", "(call sqrt (neg 1))"),
    ]);
}

#[test]
fn postfix_calls_members_indexing() {
    check_exprs(&[
        ("f(x, y)", "(call f x y)"),
        ("f()", "(call f)"),
        ("f'(x)", "(call (prime f 1) x)"),
        ("f''(x)", "(call (prime f 2) x)"),
        ("f'", "(prime f 1)"),
        ("P.x", "(. P x)"),
        ("T.x1", "(. T x1)"),
        ("A.b.c", "(. (. A b) c)"),
        ("P.f(2)", "(call (. P f) 2)"),
        ("L[2]", "(index L 2)"),
        ("L[L > 3]", "(index L (> L 3))"),
        ("L[1, 3]", "(index L 1 3)"),
        ("L[[1, 3]]", "(index L (list 1 3))"),
        ("M[1, 2]", "(index M 1 2)"),
        ("L[2..4]", "(index L (range 2 4))"),
        ("L[1][2]", "(index (index L 1) 2)"),
        ("M[1][2]^2", "(^ (index (index M 1) 2) 2)"),
        ("[1, 2, 3][2]", "(index (list 1 2 3) 2)"),
        ("(2)[1]", "(index 2 1)"),
    ]);
}

#[test]
fn lists_ranges_tuples() {
    check_exprs(&[
        ("[]", "(list)"),
        ("[1, 2, 3]", "(list 1 2 3)"),
        ("[1..10]", "(range 1 10)"),
        ("[1 ... 10]", "(range 1 10)"),
        ("[0, 0.1 .. 1]", "(range 0 0.1 1)"),
        ("[a..b+1]", "(range a (+ b 1))"),
        ("[k^2 for k = [1..5]]", "(for (^ k 2) (k (range 1 5)))"),
        ("[k for k in L]", "(for k (k L))"),
        ("[x + y for x = L, y = M]", "(for (+ x y) (x L) (y M))"),
        ("[[1, 2], [3, 4]]", "(list (list 1 2) (list 3 4))"),
        ("[\n 1,\n 2\n]", "(list 1 2)"),
        ("(1, 2)", "(tuple 1 2)"),
        ("(x, y, z)", "(tuple x y z)"),
        ("(cos t, sin t)", "(tuple (call cos t) (call sin t))"),
        ("[x = 1, 2]", "(list (== x 1) 2)"),
    ]);
}

#[test]
fn piecewise_and_restrictions() {
    check_exprs(&[
        (
            "{x < 0: -x, x > 5: 5, x}",
            "(piecewise ((< x 0) (neg x)) ((> x 5) 5) (else x))",
        ),
        ("{x < 0: -x}", "(piecewise ((< x 0) (neg x)))"),
        ("{x > 0}", "(piecewise ((> x 0)))"),
        ("{x = 0: 1, 2}", "(piecewise ((== x 0) 1) (else 2))"),
        (
            "2 + {x > 0: 1, 0}",
            "(+ 2 (piecewise ((> x 0) 1) (else 0)))",
        ),
        ("x^2 {0 < x < 2}", "(restrict (^ x 2) (cmp 0 < x < 2))"),
        ("x {x > 0}{x < 1}", "(restrict x (> x 0) (< x 1))"),
        ("x {x > 0, x < 1}", "(restrict x (> x 0) (< x 1))"),
        ("{x < 0: 1,\n x}", "(piecewise ((< x 0) 1) (else x))"),
    ]);
}

#[test]
fn calculus() {
    check_exprs(&[
        ("d/dx x^2", "(d/dx (^ x 2))"),
        ("d/dx f(x)", "(d/dx (call f x))"),
        ("d/dx sin x", "(d/dx (call sin x))"),
        ("d/dx x^2 + 1", "(+ (d/dx (^ x 2)) 1)"),
        ("d/dx 2x y", "(d/dx (* 2 x y))"),
        ("d/dx (-x)", "(d/dx (neg x))"),
        ("d / dx + 1", "(+ (/ d dx) 1)"),
        ("d/dx - 1", "(- (/ d dx) 1)"),
        ("d/dx {x}", "(restrict (/ d dx) x)"),
        ("d/dx (x + 1)", "(d/dx (+ x 1))"),
        ("d^2/dx^2 x^3", "(d2/dx (^ x 3))"),
        ("d/dθ sin θ", "(d/dθ (call sin θ))"),
        ("2 d/dx x", "(* 2 (d/dx x))"),
        ("d / dx", "(/ d dx)"),
        ("∫(0, 1) x^2 dx", "(int x 0 1 (^ x 2))"),
        ("int(0, 1) x^2 dx", "(int x 0 1 (^ x 2))"),
        ("∫(a, b) x^2 + 1 dx", "(int x a b (+ (^ x 2) 1))"),
        ("∫ x dx", "(int x x)"),
        ("∫ (x + 1)^2 dx", "(int x (^ (+ x 1) 2))"),
        ("∫(0, π) sin θ dθ", "(int θ 0 π (call sin θ))"),
        (
            "∫(0, 1) ∫(0, x) x y dy dx",
            "(int x 0 1 (int y 0 x (* x y)))",
        ),
        ("∫(0, 1) t dt + 1", "(+ (int t 0 1 t) 1)"),
        ("2∫(0, 1) x dx", "(* 2 (int x 0 1 x))"),
        ("∫(0, 1) delta x dx", "(int x 0 1 (* delta x))"),
        ("int(x)", "(call int x)"),
        ("int(a, b)", "(call int a b)"),
        ("∑(k=1, n) k", "(sum k 1 n k)"),
        ("sum(k = 1, n) k^2", "(sum k 1 n (^ k 2))"),
        ("∑(k=1, n) 2k + 1", "(+ (sum k 1 n (* 2 k)) 1)"),
        ("∏(k=1, 5) k", "(prod k 1 5 k)"),
        ("prod(j = 1, n) (1 + j)", "(prod j 1 n (+ 1 j))"),
        ("∑(i=1, n) ∑(j=1, i) i j", "(sum i 1 n (sum j 1 i (* i j)))"),
        ("sum(L)", "(call sum L)"),
    ]);
}

#[test]
fn units_and_conversion() {
    check_exprs(&[
        ("5 km/h to m/s", "(to (*/ 5 *km /h) (/ m s))"),
        ("3 m to in", "(to (* 3 m) in)"),
        ("1 to a to b", "(to (to 1 a) b)"),
        ("x + 1 to y or z", "(to (+ x 1) (or y z))"),
        ("2 kg m/s^2", "(*/ 2 *kg *m /(^ s 2))"),
        ("1 kg m / s s", "(*/ 1 *kg *m /(* s s))"),
    ]);
}

// ---------------------------------------------------------------------------
// Statements and programs
// ---------------------------------------------------------------------------

#[test]
fn statements() {
    check_stmts(&[
        ("a = 3", "(= a 3)"),
        ("f(x) = x^2", "(= (call f x) (^ x 2))"),
        ("f(x, y) = x y", "(= (call f x y) (* x y))"),
        ("y = sin x", "(= y (call sin x))"),
        ("x^2 + y^2 = 4", "(= (+ (^ x 2) (^ y 2)) 4)"),
        ("x^2 + y^2 < 4", "(< (+ (^ x 2) (^ y 2)) 4)"),
        ("y = x^2 {0 < x < 2}", "(= y (^ x 2) {(cmp 0 < x < 2)})"),
        ("y = x {x > 0}{x < 1}", "(= y x {(> x 0) (< x 1)})"),
        ("y = x {x > 0, x < 1}", "(= y x {(> x 0) (< x 1)})"),
        (
            "x^2 + y^2 = 4 {x > 0}",
            "(= (+ (^ x 2) (^ y 2)) 4 {(> x 0)})",
        ),
        (
            "x^2 + y^2 < 4 {x > 0}",
            "(where (< (+ (^ x 2) (^ y 2)) 4) {(> x 0)})",
        ),
        ("y1 ~ m x1 + b {x1 > 0}", "(~ y1 (+ (* m x1) b) {(> x1 0)})"),
        ("(x {x > 0}) = 1", "(= (restrict x (> x 0)) 1)"),
        ("a -> 1, b -> b + 1", "(actions (-> a 1) (-> b (+ b 1)))"),
        ("a -> 1,\n b -> 2", "(actions (-> a 1) (-> b 2))"),
        ("y1 ~ m x1 + b", "(~ y1 (+ (* m x1) b))"),
        ("a -> a + 1", "(-> a (+ a 1))"),
        ("a → a + 1", "(-> a (+ a 1))"),
        ("P = (1, 2)", "(= P (tuple 1 2))"),
        ("L = [1..10]", "(= L (range 1 10))"),
        ("r = θ", "(= r θ)"),
        ("f'(x) = 2x", "(= (call (prime f 1) x) (* 2 x))"),
        ("let x = 3", "(let x 3)"),
        ("return x + 1", "(return (+ x 1))"),
        ("return", "(return)"),
        ("x == 3", "(== x 3)"),
        ("a =\n 3", "(= a 3)"),
        ("2x;", "(* 2 x)"),
        ("\n\nx\n", "x"),
        ("{x < 0: -x, x}", "(piecewise ((< x 0) (neg x)) (else x))"),
        (
            "y = {x < 0: -x, x}",
            "(= y (piecewise ((< x 0) (neg x)) (else x)))",
        ),
        ("g(x) = d/dx f(x)", "(= (call g x) (d/dx (call f x)))"),
    ]);
}

#[test]
fn script_statements() {
    check_stmts(&[
        ("if x > 0 { y = 1 }", "(if (> x 0) (block (= y 1)))"),
        (
            "if x = 0 { a } else { b }",
            "(if (== x 0) (block a) (block b))",
        ),
        (
            "if a { 1 } else if b { 2 } else { 3 }",
            "(if a (block 1) (if b (block 2) (block 3)))",
        ),
        (
            "for k in [1..10] { s = s + k }",
            "(for k (range 1 10) (block (= s (+ s k))))",
        ),
        (
            "while i < 10 { i = i + 1 }",
            "(while (< i 10) (block (= i (+ i 1))))",
        ),
        (
            "fn add(a, b) { return a + b }",
            "(fn add (a b) (block (return (+ a b))))",
        ),
        ("fn zero() {}", "(fn zero () (block))"),
        ("if x {}", "(if x (block))"),
        (
            "if (x {x > 0}) { 1 }",
            "(if (restrict x (> x 0)) (block 1))",
        ),
        (
            "while x < {y > 0: 1, 2} { x }",
            "(while (< x (piecewise ((> y 0) 1) (else 2))) (block x))",
        ),
    ]);
}

#[test]
fn programs() {
    check_programs(&[
        ("a = 1\nb = 2", "(= a 1) (= b 2)"),
        ("a = 1; b = 2;", "(= a 1) (= b 2)"),
        ("\n\n a = 1 \n\n", "(= a 1)"),
        ("", ""),
        ("# only a comment", ""),
        (
            "let s = 0\nfor k in [1..10] {\n    s = s + k^2\n}\nreturn s",
            "(let s 0) (for k (range 1 10) (block (= s (+ s (^ k 2))))) (return s)",
        ),
        (
            "fn fact(n) {\n  if n <= 1 {\n    return 1\n  }\n  return n fact(n - 1)\n}",
            "(fn fact (n) (block (if (<= n 1) (block (return 1))) (return (* n (call fact (- n 1))))))",
        ),
        (
            "if x > 0 {\n  y = 1\n}\nelse {\n  y = 2\n}",
            "(if (> x 0) (block (= y 1)) (block (= y 2)))",
        ),
        (
            "while i < 3 { i = i + 1; print(i) }",
            "(while (< i 3) (block (= i (+ i 1)) (call print i)))",
        ),
        (
            "f(x) = x^2 # square\ny = f(x)",
            "(= (call f x) (^ x 2)) (= y (call f x))",
        ),
        ("L = [\n  1,\n  2\n]\nM = L", "(= L (list 1 2)) (= M L)"),
    ]);
}

// ---------------------------------------------------------------------------
// Errors and recovery
// ---------------------------------------------------------------------------

#[test]
fn expression_errors() {
    check_expr_errors(&[
        ("", 1, "unexpected end of input"),
        ("1 +", 1, "unexpected end of input"),
        ("(1 + 2", 1, "expected `)`"),
        ("[1, 2", 1, "expected `]`"),
        ("f(1,", 1, "unexpected end of input"),
        ("()", 1, "empty parentheses"),
        ("{}", 1, "empty braces"),
        ("x {}", 1, "empty restriction"),
        ("1 2", 1, "expected end of input"),
        ("* 3", 1, "expected an expression"),
        ("a = 3", 1, "expected end of input"),
        ("{a, b}", 1, "expected `:`"),
        ("[1, 2, 3 .. 10]", 1, "at most two elements"),
        ("L[]", 1, "empty index"),
        ("P.", 1, "expected identifier"),
        ("∫(0, 1) x^2", 1, "differential"),
        ("∫ dx", 1, "integrand"),
        ("∑ k", 1, "expected `(`"),
        ("∑(k, n) k", 1, "expected `=`"),
        ("[k for 1 = L]", 1, "expected identifier"),
        ("a $ b", 1, "unexpected character"),
        ("x_ + 1", 1, "expected subscript"),
        ("1 + $ + ", 2, "unexpected character"),
        ("(1, 2", 1, "expected `)`"),
        ("2 3", 1, "expected end of input"),
        ("a <", 1, "unexpected end of input"),
        ("x^{2}", 1, "braces do not group"),
        ("√{x}", 1, "braces do not group"),
        ("sin^{2} x", 1, "braces do not group"),
        ("d^3/dx^2 f", 1, "derivative order mismatch"),
        ("d^0/dx^0 f", 1, "derivative order mismatch"),
        ("|x", 1, "expected `|`"),
        ("|x + 1", 1, "expected `|`"),
        ("|a|b|", 1, "unexpected end of input"),
    ]);
}

#[test]
fn statement_errors() {
    let cases: &[(&str, &str)] = &[
        ("let = 3", "expected identifier"),
        ("let x 3", "expected `=`"),
        ("a = b = c", "expected end of statement"),
        ("for k [1..3] { }", "expected `in`"),
        ("if x y = 1", "expected `{`"),
        ("fn (a) { }", "expected identifier"),
        ("fn f(a b) { }", "expected `)`"),
        ("if x { 1 } y", "expected end of statement"),
        ("a = 1\nb = 2", "expected end of input"),
        ("if x {", "unclosed block"),
        ("let x = 3 {x > 0}", "expected end of statement"),
        ("return x {x > 0}", "expected end of statement"),
        ("a -> 1 {x > 0}", "expected end of statement"),
        ("a -> 1, b", "expected `->`"),
        ("y = x {x > 0} = 2", "expected end of statement"),
    ];
    for (src, needle) in cases {
        let errs = parse_statement(src).expect_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains(needle)),
            "{src:?}: {errs:?} should mention {needle:?}"
        );
    }
}

#[test]
fn program_error_recovery() {
    let src = "a = 1 +\nb = (2\nc = 3\nd = * 4\ne = 5";
    let (prog, errs) = parse_program_partial(src);
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert_eq!(sx_program(&prog), "(= c 3) (= e 5)");
    // `a = 1 +` continues onto the next line, so the first error is at `=`.
    assert_eq!(&src[errs[0].span.start..errs[0].span.end], "=");
    assert!(parse_program(src).is_err());

    // Errors inside blocks are recovered locally; the block still parses.
    let src = "if x {\n  y = )\n  z = 2\n}\nw = 1";
    let (prog, errs) = parse_program_partial(src);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(sx_program(&prog), "(if x (block (= z 2))) (= w 1)");

    // Stray closing brace.
    let (prog, errs) = parse_program_partial("a\n}\nb");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].message.contains("unexpected `}`"));
    assert_eq!(sx_program(&prog), "a b");

    // Semicolon-separated statements recover independently.
    let (prog, errs) = parse_program_partial("a = ; b = 2; c = (;");
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert_eq!(sx_program(&prog), "(= b 2)");

    // An unterminated string stops at the end of its line.
    let (prog, errs) = parse_program_partial("s = \"ab\nx = 1");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("unterminated string"));
    assert_eq!(sx_program(&prog), "(= s \"ab\") (= x 1)");

    // Lexical and syntax errors are reported together, sorted by position.
    let (_, errs) = parse_program_partial("x = 1 $\ny = 1 +");
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs.windows(2).all(|w| w[0].span.start <= w[1].span.start));
}

#[test]
fn error_rendering() {
    let src = "a = 1\nb = (2 + ";
    let errs = parse_program(src).unwrap_err();
    assert_eq!(errs.len(), 1);
    let r = errs[0].render(src);
    assert!(r.starts_with("2:"), "{r}");
    let e = ParseError::new(Span::new(3, 4), "boom");
    assert_eq!(e.line_col("ab\ncd"), (2, 1));
    assert_eq!(e.render("ab\ncd"), "2:1: boom\ncd\n^");
    assert_eq!(e.to_string(), "boom (at 3..4)");
}

// ---------------------------------------------------------------------------
// Spans
// ---------------------------------------------------------------------------

#[test]
fn spans() {
    let src = "f(x) = 2 sin(x) + y_{1}";
    let s = stmt_ok(src);
    assert_eq!(s.span, Span::new(0, src.len()));
    let StmtKind::Equation { lhs, rhs, .. } = &s.kind else {
        panic!("expected equation");
    };
    assert_eq!(&src[lhs.span.start..lhs.span.end], "f(x)");
    assert_eq!(&src[rhs.span.start..rhs.span.end], "2 sin(x) + y_{1}");
    let ExprKind::Add(terms) = &*rhs.kind else {
        panic!("expected sum");
    };
    let (prod, y) = (&terms[0].expr, &terms[1].expr);
    assert_eq!(&src[prod.span.start..prod.span.end], "2 sin(x)");
    assert_eq!(&src[y.span.start..y.span.end], "y_{1}");

    let src = "(a + b)";
    let e = expr_ok(src);
    assert_eq!(e.span, Span::new(0, 7));

    let src = "  [1..10]  ";
    let e = expr_ok(src);
    assert_eq!(&src[e.span.start..e.span.end], "[1..10]");

    let src = "∫(0, 1) x dx";
    let e = expr_ok(src);
    assert_eq!(e.span, Span::new(0, src.len()));

    let errs = parse_expr("1 + (2 * )").unwrap_err();
    assert_eq!(errs[0].span, Span::new(9, 10));
}

// ---------------------------------------------------------------------------
// Limits: deep nesting and long chains must not overflow the stack
// ---------------------------------------------------------------------------

/// Run `f` on a thread with a 2 MiB stack (the default for test threads,
/// stated explicitly so the limit is part of the test).
fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .expect("thread panicked (stack overflow?)");
}

fn head(src: &str) -> String {
    src.chars().take(20).collect()
}

fn nest(open: &str, inner: &str, close: &str, n: usize) -> String {
    format!("{}{inner}{}", open.repeat(n), close.repeat(n))
}

/// (open, inner, close, depth units per level, extra units at the innermost
/// level) for every nesting construct.
const NESTINGS: &[(&str, &str, &str, usize, usize)] = &[
    ("(", "x", ")", 1, 0),
    ("[", "1", "]", 1, 0),
    ("-", "x", "", 1, 0),
    ("not ", "x", "", 1, 0),
    ("√", "x", "", 1, 0),
    ("sin ", "x", "", 1, 0),
    ("x^", "x", "", 1, 0),
    ("|", "x", "|", 1, 0),
    ("f(", "x", ")", 1, 0),
    ("{x: ", "1", "}", 1, 0),
    ("(1 + ", "x", ")", 1, 0),
    ("[", "k", " for k = L]", 1, 0),
    ("d/dx ", "x", "", 1, 0),
    ("∑(k=1, 2) ", "k", "", 1, 1),
    ("∫ ", "x", " dx", 1, 0),
    ("L[", "1", "]", 2, 0),
    ("-(", "x", ")", 2, 0),
];

#[test]
fn nesting_limit_errors() {
    on_small_stack(|| {
        for &(open, inner, close, units, extra) in NESTINGS {
            // The top-level expression itself uses one unit.
            for n in [(parser::MAX_DEPTH - 1 - extra) / units + 1, 300] {
                let src = nest(open, inner, close, n);
                let errs = parse_expr(&src).expect_err(&head(&src));
                assert!(
                    errs.iter().any(|e| e.message.contains("nested too deeply")),
                    "{}… (n = {n}): {errs:?}",
                    head(&src)
                );
            }
        }
        for src in [
            nest("-", "x", "", 200_000),
            "1 mod ".repeat(300) + "1",
            "1 to ".repeat(300) + "1",
            // Long postfix chains nest one level per operator.
            "1".to_string() + &"!".repeat(5000),
            "x".to_string() + &"%".repeat(5000),
            "f".to_string() + &"'".repeat(5000),
            "x".to_string() + &".m".repeat(5000),
            "x".to_string() + &"[1]".repeat(5000),
            "1".to_string() + &"!".repeat(parser::MAX_DEPTH + 1),
        ] {
            let errs = parse_expr(&src).expect_err(&head(&src));
            assert!(errs.iter().any(|e| e.message.contains("too")), "{errs:?}");
        }
        let program = nest("if x {\n", "y", "\n}", parser::MAX_DEPTH);
        let (_, errs) = parse_program_partial(&program);
        assert!(errs.iter().any(|e| e.message.contains("nested too deeply")));
    });
}

/// The deepest accepted input of every kind parses, prints, re-parses and
/// drops on a 2 MiB stack — also in unoptimised builds (about 1 MiB is
/// used in the worst case).
#[test]
fn deepest_accepted_nesting_fits_small_stack() {
    on_small_stack(|| {
        for &(open, inner, close, units, extra) in NESTINGS {
            let src = nest(open, inner, close, (parser::MAX_DEPTH - 1 - extra) / units);
            let e = parse_expr(&src).unwrap_or_else(|e| panic!("{}…: {e:?}", head(&src)));
            roundtrip_expr(&head(&src), &e);
        }
        // The longest accepted postfix chain.
        let src = "x".to_string() + &"!".repeat(parser::MAX_DEPTH);
        let e = parse_expr(&src).unwrap_or_else(|e| panic!("postfix chain: {e:?}"));
        roundtrip_expr("postfix chain", &e);
        // Each `if` uses one unit; the innermost statement and its
        // expression use two more.
        let program = nest("if x {\n", "y", "\n}", parser::MAX_DEPTH - 2);
        let p = program_ok(&program);
        roundtrip_program("nested ifs", &p);
    });
}

#[test]
fn long_chains_are_flat() {
    on_small_stack(|| {
        let n = 100_000;
        for sep in ["+", "-", "*", "/", " ", " and ", " or "] {
            let src = vec!["x"; n].join(sep);
            let e = parse_expr(&src).unwrap_or_else(|e| panic!("{sep:?}: {e:?}"));
            let len = match &*e.kind {
                ExprKind::Add(v) => v.len(),
                ExprKind::Mul(v) => v.len(),
                ExprKind::And(v) | ExprKind::Or(v) => v.len(),
                other => panic!("{sep:?}: unexpected {other:?}"),
            };
            assert_eq!(len, n, "{sep:?}");
            let printed = e.to_string();
            let again = parse_expr(&printed).unwrap();
            assert_eq!(again.without_spans(), e.without_spans());
            drop(again);
            drop(e);
        }
        // Long comparison chains and lists are flat as well.
        let src = vec!["x"; n].join(" < ");
        assert!(parse_expr(&src).is_ok());
        let src = format!("[{}]", vec!["1"; n].join(", "));
        assert!(parse_expr(&src).is_ok());
        // Many statements.
        let src = "a = 1\n".repeat(n);
        assert_eq!(program_ok(&src).stmts.len(), n);
    });
}

#[test]
fn integral_header_scan_is_linear() {
    // `∫(...)` scans its parentheses once; a long bound must stay fast.
    let src = format!("∫({}x, 1) x dx", "x+".repeat(50_000));
    let start = std::time::Instant::now();
    assert!(parse_expr(&src).is_ok());
    assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
    let src = format!("int({}x, 1) x dx", "x+".repeat(50_000));
    assert!(parse_expr(&src).is_ok());
}

// ---------------------------------------------------------------------------
// Pretty-printer
// ---------------------------------------------------------------------------

#[test]
fn canonical_printing() {
    let cases = [
        ("2x", "2 * x"),
        ("-x^2", "-x^2"),
        ("(-x)^2", "(-x)^2"),
        ("2^-3", "2^(-3)"),
        ("a - (b - c)", "a - (b - c)"),
        ("(a - b) - c", "(a - b) - c"),
        ("a - b - c", "a - b - c"),
        ("1/2x", "1 / (2 * x)"),
        ("a / b * c", "a / b * c"),
        ("(a * b) * c", "(a * b) * c"),
        ("(a and b) and c", "(a and b) and c"),
        ("a and b and c", "a and b and c"),
        ("|x|", "abs(x)"),
        ("sin", "(sin)"),
        ("sin^2", "(sin)^2"),
        ("(sin) - x", "(sin) - x"),
        ("d / dx", "d / (dx)"),
        ("d / dx(y)", "d/dx y"),
        ("x^({a: 1})", "x^({a: 1})"),
        ("d/dx (-x)", "d/dx (-x)"),
        ("d/dx ({x > 0: 1})", "d/dx ({x > 0: 1})"),
        ("L[[1, 2]]", "L[[1, 2]]"),
        ("2^(3^2)", "2^3^2"),
        ("x ≤ 1", "x <= 1"),
        ("√x", "sqrt(x)"),
        ("sin x cos x", "sin(x) * cos(x)"),
        ("[1 ... 10]", "[1..10]"),
        ("[0,0.1..1]", "[0, 0.1..1]"),
        ("L[2..4]", "L[2..4]"),
        ("L[1,3]", "L[1, 3]"),
        ("L[[5]]", "L[[5]]"),
        ("{x<0:-x,x}", "{x < 0: -x, x}"),
        ("{x = 0: 1}", "{x == 0: 1}"),
        ("int(0,1) x^2 dx", "∫(0, 1) x^2 dx"),
        ("∫ (x+1) dx", "∫ x + 1 dx"),
        ("∫ ((1, 2)) dx", "∫ ((1, 2)) dx"),
        ("sum(k=1,n) k", "∑(k = 1, n) k"),
        ("(∑(k=1,n) k) x", "(∑(k = 1, n) k) * x"),
        ("x * (d/dx x)", "x * (d/dx x)"),
        ("(d/dx x) y", "(d/dx x) * y"),
        ("d^2/dx^2 y", "d^2/dx^2 y"),
        ("5 km/h to m/s", "5 * km / h to m / s"),
        ("not (a and b)", "not (a and b)"),
        ("(x {x>0})!", "(x {x > 0})!"),
        ("\"a\\\"b\"", "\"a\\\"b\""),
        ("y_{12}", "y_12"),
        ("x mod 3", "x mod 3"),
        ("(2)[1]", "(2)[1]"),
    ];
    for (src, want) in cases {
        let e = expr_ok(src);
        assert_eq!(e.to_string(), want, "print(parse({src:?}))");
        roundtrip_expr(src, &e);
    }
}

#[test]
fn program_printing() {
    let src =
        "fn f(n){if n<=1{return 1}else{return n f(n-1)}}\nfor k in L { print(k) ; }\nwhile 1 {}";
    let p = program_ok(src);
    let want = "\
fn f(n) {
    if n <= 1 {
        return 1
    } else {
        return n * f(n - 1)
    }
}
for k in L {
    print(k)
}
while 1 {}";
    assert_eq!(p.to_string(), want);
    roundtrip_program(src, &p);
}

// ---------------------------------------------------------------------------
// Property: parse(print(ast)) == ast on generated trees
// ---------------------------------------------------------------------------

/// Small deterministic PRNG (xorshift64*), to avoid a dev-dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

fn mk(kind: ExprKind) -> Expr {
    Expr::new(kind, Span::default())
}

fn bx(kind: ExprKind) -> Expr {
    mk(kind)
}

fn gen_leaf(r: &mut Rng) -> ExprKind {
    match r.below(4) {
        0 => ExprKind::Number {
            text: r.pick(&["0", "1", "2.5", "1e3", ".5", "6.02e-23"]).into(),
            imaginary: r.below(5) == 0,
        },
        1 => ExprKind::Str(r.pick(&["", "a b", "q\"uote", "x\\y\n"]).into()),
        _ => ExprKind::Ident(
            r.pick(&["x", "y", "a1", "θ", "π", "x_1", "km", "sin", "f", "d", "dt"])
                .into(),
        ),
    }
}

fn gen_expr(r: &mut Rng, depth: u32) -> ExprKind {
    if depth == 0 {
        return gen_leaf(r);
    }
    let d = depth - 1;
    let sub = |r: &mut Rng| mk(gen_expr(r, d));
    match r.below(22) {
        0 | 1 => gen_leaf(r),
        2 => ExprKind::Unary {
            op: [UnaryOp::Neg, UnaryOp::Pos, UnaryOp::Not][r.below(3) as usize],
            expr: sub(r),
        },
        3 => ExprKind::Binary {
            op: [BinOp::Mod, BinOp::Pow, BinOp::To][r.below(3) as usize],
            lhs: sub(r),
            rhs: sub(r),
        },
        4 => ExprKind::Add(
            (0..2 + r.below(2))
                .map(|i| Term {
                    op: if i > 0 && r.below(2) == 0 {
                        AddOp::Sub
                    } else {
                        AddOp::Add
                    },
                    expr: sub(r),
                })
                .collect(),
        ),
        5 => ExprKind::Mul(
            (0..2 + r.below(2))
                .map(|i| Factor {
                    op: if i > 0 && r.below(2) == 0 {
                        MulOp::Div
                    } else {
                        MulOp::Mul
                    },
                    expr: sub(r),
                })
                .collect(),
        ),
        6 => {
            let items = (0..2 + r.below(2)).map(|_| sub(r)).collect();
            if r.below(2) == 0 {
                ExprKind::And(items)
            } else {
                ExprKind::Or(items)
            }
        }
        7 => ExprKind::Postfix {
            op: if r.below(2) == 0 {
                PostfixOp::Factorial
            } else {
                PostfixOp::Percent
            },
            expr: sub(r),
        },
        8 => {
            let n = 1 + r.below(2) as usize;
            let ops = [
                CmpOp::Lt,
                CmpOp::Le,
                CmpOp::Gt,
                CmpOp::Ge,
                CmpOp::Eq,
                CmpOp::Ne,
            ];
            ExprKind::Compare {
                first: sub(r),
                rest: (0..n).map(|_| (ops[r.below(6) as usize], sub(r))).collect(),
            }
        }
        9 => ExprKind::Call {
            callee: bx(ExprKind::Ident(r.pick(&["f", "g", "sin", "max"]).into())),
            args: (0..r.below(3)).map(|_| sub(r)).collect(),
        },
        10 => ExprKind::Member {
            expr: sub(r),
            name: r.pick(&["x", "y1"]).into(),
        },
        11 => ExprKind::Index {
            expr: sub(r),
            indices: (0..1 + r.below(2)).map(|_| sub(r)).collect(),
        },
        12 => ExprKind::List((0..r.below(3)).map(|_| sub(r)).collect()),
        13 => ExprKind::Range {
            start: sub(r),
            second: if r.below(2) == 0 { Some(sub(r)) } else { None },
            end: sub(r),
        },
        14 => ExprKind::Comprehension {
            body: sub(r),
            bindings: vec![Binding {
                name: "k".into(),
                iter: sub(r),
                span: Span::default(),
            }],
        },
        15 => ExprKind::Tuple(vec![sub(r), sub(r)]),
        16 => {
            if r.below(3) == 0 {
                ExprKind::Piecewise {
                    branches: vec![Branch {
                        cond: sub(r),
                        value: None,
                    }],
                    otherwise: None,
                }
            } else {
                ExprKind::Piecewise {
                    branches: vec![Branch {
                        cond: sub(r),
                        value: Some(sub(r)),
                    }],
                    otherwise: if r.below(2) == 0 { Some(sub(r)) } else { None },
                }
            }
        }
        17 => ExprKind::Restrict {
            expr: mk(gen_no_restrict(r, d)),
            conds: vec![sub(r)],
        },
        18 => ExprKind::Derivative {
            var: r.pick(&["x", "θ", "t_1"]).into(),
            order: 1 + r.below(2) as u32,
            expr: sub(r),
        },
        19 => ExprKind::Integral {
            bounds: if r.below(2) == 0 {
                Some((sub(r), sub(r)))
            } else {
                None
            },
            body: sub(r),
            var: r.pick(&["x", "t"]).into(),
        },
        20 => ExprKind::BigOp {
            op: if r.below(2) == 0 {
                BigOpKind::Sum
            } else {
                BigOpKind::Product
            },
            var: "k".into(),
            from: sub(r),
            to: sub(r),
            body: sub(r),
        },
        _ => ExprKind::Prime {
            expr: bx(ExprKind::Ident("f".into())),
            order: 1 + r.below(2) as u32,
        },
    }
}

fn gen_no_restrict(r: &mut Rng, depth: u32) -> ExprKind {
    loop {
        let k = gen_expr(r, depth);
        if !matches!(k, ExprKind::Restrict { .. }) {
            return k;
        }
    }
}

/// The generator can produce a few shapes the parser never does; skip them.
fn parser_producible(e: &Expr) -> bool {
    let mut ok = true;
    visit(e, &mut |e| {
        // A differential-looking name inside an integrand is ambiguous
        // (`∫ x dt dt`); the parser treats it as the end of the integrand.
        if let ExprKind::Integral { body, .. } = &*e.kind {
            visit(body, &mut |b| {
                if matches!(&*b.kind, ExprKind::Ident(n) if parser::differential_var(n).is_some()) {
                    ok = false;
                }
            });
        }
    });
    ok
}

fn visit(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    let mut go = |c: &Expr| visit(c, f);
    match &*e.kind {
        ExprKind::Number { .. } | ExprKind::Str(_) | ExprKind::Ident(_) => {}
        ExprKind::Prime { expr, .. }
        | ExprKind::Unary { expr, .. }
        | ExprKind::Postfix { expr, .. }
        | ExprKind::Member { expr, .. }
        | ExprKind::Derivative { expr, .. } => go(expr),
        ExprKind::Binary { lhs, rhs, .. } => {
            go(lhs);
            go(rhs);
        }
        ExprKind::Compare { first, rest } => {
            go(first);
            rest.iter().for_each(|(_, e)| go(e));
        }
        ExprKind::Call { callee, args } => {
            go(callee);
            args.iter().for_each(go);
        }
        ExprKind::Index { expr, indices } => {
            go(expr);
            indices.iter().for_each(go);
        }
        ExprKind::Add(terms) => terms.iter().for_each(|t| go(&t.expr)),
        ExprKind::Mul(factors) => factors.iter().for_each(|f| go(&f.expr)),
        ExprKind::List(v) | ExprKind::Tuple(v) | ExprKind::And(v) | ExprKind::Or(v) => {
            v.iter().for_each(go)
        }
        ExprKind::Range { start, second, end } => {
            go(start);
            if let Some(s) = second {
                go(s);
            }
            go(end);
        }
        ExprKind::Comprehension { body, bindings } => {
            go(body);
            bindings.iter().for_each(|b| go(&b.iter));
        }
        ExprKind::Piecewise {
            branches,
            otherwise,
        } => {
            for b in branches {
                go(&b.cond);
                if let Some(v) = &b.value {
                    go(v);
                }
            }
            if let Some(o) = otherwise {
                go(o);
            }
        }
        ExprKind::Restrict { expr, conds } => {
            go(expr);
            conds.iter().for_each(go);
        }
        ExprKind::Integral { bounds, body, .. } => {
            if let Some((lo, hi)) = bounds {
                go(lo);
                go(hi);
            }
            go(body);
        }
        ExprKind::BigOp { from, to, body, .. } => {
            go(from);
            go(to);
            go(body);
        }
    }
}

#[test]
fn property_print_parse_roundtrip() {
    // `VISCAS_PROPTEST_ITERS` / `VISCAS_PROPTEST_SEED` allow longer local runs.
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let iters = env("VISCAS_PROPTEST_ITERS").unwrap_or(3000) as u32;
    let mut r = Rng(env("VISCAS_PROPTEST_SEED").unwrap_or(0x9E37_79B9_7F4A_7C15) | 1);
    let mut checked = 0;
    for i in 0..iters {
        let e = mk(gen_expr(&mut r, 1 + (i % 5)));
        if !parser_producible(&e) {
            continue;
        }
        let printed = e.to_string();
        let parsed = parse_expr(&printed).unwrap_or_else(|errs| {
            panic!(
                "generated {e:?}\nprinted {printed:?}\nfailed to parse:\n{}",
                fmt_errors(&printed, &errs)
            )
        });
        assert_eq!(
            parsed.without_spans(),
            e,
            "round trip mismatch for {printed:?}\nreprinted {:?}",
            parsed.to_string()
        );
        checked += 1;
    }
    assert!(checked * 3 > iters * 2, "only {checked} trees checked");
}

// ---------------------------------------------------------------------------
// Definition operator
// ---------------------------------------------------------------------------

#[test]
fn colon_eq_definitions() {
    for (src, want) in [
        ("a := 3", "(:= a 3)"),
        ("a ≔ 3", "(:= a 3)"),
        ("f(x) := x^2", "(:= (call f x) (^ x 2))"),
        (
            "g(x, y) := x y {x > 0}",
            "(:= (call g x y) (* x y) {(> x 0)})",
        ),
        ("a = 3", "(= a 3)"),
        ("x = 3", "(= x 3)"),
    ] {
        let s = stmt_ok(src);
        assert_eq!(sx_stmt(&s), want, "{src}");
        roundtrip_stmt(src, &s);
    }
    assert_eq!(stmt_ok("f(x) := x^2").to_string(), "f(x) := x^2");
}

#[test]
fn colon_eq_rejects_bad_heads() {
    for src in ["a + b := 1", "f(1) := 2", "f(x, x) := 1", "(a, b) := 1"] {
        assert!(parse_statement(src).is_err(), "{src}");
    }
}

#[test]
fn colon_in_piecewise_unaffected() {
    let e = parse_expr("{x < 0: -x, x}").unwrap();
    assert!(matches!(&*e.kind, ExprKind::Piecewise { .. }));
}

#[test]
fn eq_mode_definitions() {
    let opts = ParseOptions {
        definition_op: DefinitionOp::Eq,
    };
    let p = |src: &str| sx_stmt(&parse_statement_with(src, &opts).unwrap());
    assert_eq!(p("a = 3"), "(:= a 3)");
    assert_eq!(p("f(x) = x^2"), "(:= (call f x) (^ x 2))");
    assert_eq!(p("y = x^2"), "(= y (^ x 2))");
    assert_eq!(p("x = 3"), "(= x 3)");
    assert_eq!(p("r = θ"), "(= r θ)");
    assert_eq!(p("sin(x) = y"), "(= (call sin x) y)");
    assert_eq!(p("x^2 + y^2 = 4"), "(= (+ (^ x 2) (^ y 2)) 4)");
    assert_eq!(p("b := 1"), "(:= b 1)");
    let prog = parse_program_with("for k in L { s = s + k }", &opts).unwrap();
    assert!(sx_stmt(&prog.stmts[0]).contains("(:= s"));
}
