//! Arithmetic in the sketch language: the smallest thing that closes
//! the gap code generation would close.
//!
//! WHY NOT JUST HAVE THE MODEL WRITE PYTHON
//!
//! It is the obvious move -- a model is far better at Python than at
//! any DSL invented for it, and loops, variables and arithmetic come
//! free. Weigh it honestly against what actually failed:
//!
//!   water sphere -> house      a menu problem
//!   carve -> statue            a polarity problem
//!   `pass hollow the ribcage`  a parser bug
//!   cross-pass references      a scoping bug
//!
//! Not one of those is an expressiveness failure, and Python fixes
//! none of them. What Python WOULD fix is parametric repetition --
//! "forty towers, each a tenth taller than the last" -- and that is
//! arithmetic over an index, not a programming language.
//!
//! Against it: an interpreter is a dependency in a game engine that
//! has none, plus a sandbox, plus a new failure surface. And the
//! failure MODES are the wrong way round. A DSL line that does not
//! parse loses that line; a Python exception loses the whole build.
//! When the author is a 1B, partial output is the difference between
//! a usable answer and nothing.
//!
//! So: expressions, bound over a repeat's index. Fifty lines, no
//! dependency, and a parse error is still local.
//!
//! WHAT THIS DOES NOT DO
//!
//! No control flow, no user functions, no state. If a request really
//! needs those it needs a real language, and the right answer then is
//! to generate a PROGRAM in this crate's own program language from a
//! model that can reason -- not to run the model's code.

use std::collections::BTreeMap;

/// Evaluate an arithmetic expression with named variables.
///
/// Supports + - * / %, parentheses, unary minus, and the functions a
/// placement actually needs: sin, cos, min, max, abs, floor, round,
/// sqrt, pow. Angles in DEGREES, because every other number a player
/// or a model writes here is in degrees.
pub fn eval(src: &str, vars: &BTreeMap<String, f32>) -> Result<f32, String> {
    let toks = lex(src)?;
    let mut p = P { t: &toks, i: 0, vars };
    let v = p.expr(0)?;
    if p.i < p.t.len() { return Err(format!("unexpected '{}' in {src:?}", p.t[p.i])); }
    if !v.is_finite() { return Err(format!("{src:?} is not a finite number")); }
    Ok(v)
}

/// Is this worth evaluating, or is it a plain number? Cheap enough to
/// always call, and it keeps the common path free.
pub fn is_expr(s: &str) -> bool {
    s.parse::<f32>().is_err() && !s.is_empty()
}

fn lex(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() { i += 1; continue; }
        if ch.is_ascii_digit() || ch == '.' {
            let st = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') { i += 1; }
            out.push(c[st..i].iter().collect());
        } else if ch.is_alphabetic() || ch == '_' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; }
            out.push(c[st..i].iter().collect());
        } else if "+-*/%(),".contains(ch) {
            out.push(ch.to_string()); i += 1;
        } else {
            return Err(format!("'{ch}' has no meaning in an expression"));
        }
    }
    Ok(out)
}

struct P<'a> { t: &'a [String], i: usize, vars: &'a BTreeMap<String, f32> }

impl<'a> P<'a> {
    fn peek(&self) -> Option<&str> { self.t.get(self.i).map(|s| s.as_str()) }
    fn eat(&mut self) -> Option<String> { let v = self.t.get(self.i).cloned(); if v.is_some() { self.i += 1; } v }

    fn expr(&mut self, min_bp: u8) -> Result<f32, String> {
        let mut lhs = self.atom()?;
        while let Some(op) = self.peek() {
            let bp = match op { "+" | "-" => 1, "*" | "/" | "%" => 2, _ => break };
            if bp < min_bp { break; }
            let op = self.eat().unwrap();
            let rhs = self.expr(bp + 1)?;
            lhs = match op.as_str() {
                "+" => lhs + rhs, "-" => lhs - rhs, "*" => lhs * rhs,
                "/" => { if rhs == 0.0 { return Err("divide by zero".into()); } lhs / rhs }
                _ => { if rhs == 0.0 { return Err("modulo by zero".into()); } lhs % rhs }
            };
        }
        Ok(lhs)
    }

    fn atom(&mut self) -> Result<f32, String> {
        let t = self.eat().ok_or("expression ended early")?;
        match t.as_str() {
            "(" => { let v = self.expr(0)?;
                     if self.eat().as_deref() != Some(")") { return Err("missing ')'".into()); }
                     Ok(v) }
            "-" => Ok(-self.atom()?),
            "+" => self.atom(),
            _ if t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.') =>
                t.parse().map_err(|_| format!("'{t}' is not a number")),
            name => {
                if self.peek() == Some("(") {
                    self.eat();
                    let mut args = Vec::new();
                    if self.peek() != Some(")") {
                        loop {
                            args.push(self.expr(0)?);
                            match self.peek() { Some(",") => { self.eat(); } _ => break }
                        }
                    }
                    if self.eat().as_deref() != Some(")") { return Err(format!("missing ')' after {name}")); }
                    return call(name, &args);
                }
                self.vars.get(name).copied()
                    .ok_or_else(|| format!("'{name}' is not a known value; available: {}",
                        if self.vars.is_empty() { "(none here)".to_string() }
                        else { self.vars.keys().cloned().collect::<Vec<_>>().join(", ") }))
            }
        }
    }
}

fn call(name: &str, a: &[f32]) -> Result<f32, String> {
    let need = |n: usize| if a.len() == n { Ok(()) } else { Err(format!("{name} takes {n} argument(s)")) };
    match name {
        "sin" => { need(1)?; Ok(a[0].to_radians().sin()) }
        "cos" => { need(1)?; Ok(a[0].to_radians().cos()) }
        "abs" => { need(1)?; Ok(a[0].abs()) }
        "floor" => { need(1)?; Ok(a[0].floor()) }
        "round" => { need(1)?; Ok(a[0].round()) }
        "sqrt" => { need(1)?; if a[0] < 0.0 { Err("sqrt of a negative".into()) } else { Ok(a[0].sqrt()) } }
        "min" => { need(2)?; Ok(a[0].min(a[1])) }
        "max" => { need(2)?; Ok(a[0].max(a[1])) }
        "pow" => { need(2)?; Ok(a[0].powf(a[1])) }
        "lerp" => { if a.len() != 3 { return Err("lerp takes 3 arguments".into()); }
                    Ok(a[0] + (a[1] - a[0]) * a[2]) }
        _ => Err(format!("no function '{name}'; have: sin, cos, abs, floor, round, sqrt, min, max, pow, lerp")),
    }
}

/// The variables bound inside a repeat: which copy this is, how many
/// there are, and how far through (0..1). `t` is what makes "each one
/// taller than the last" a single expression.
pub fn repeat_vars(i: usize, n: usize) -> BTreeMap<String, f32> {
    let mut v = BTreeMap::new();
    v.insert("i".into(), i as f32);
    v.insert("n".into(), n as f32);
    v.insert("t".into(), if n <= 1 { 0.0 } else { i as f32 / (n - 1) as f32 });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&str, f32)]) -> BTreeMap<String, f32> {
        pairs.iter().map(|(k, x)| (k.to_string(), *x)).collect()
    }

    #[test]
    fn arithmetic_works_with_the_precedence_a_person_expects() {
        let e = BTreeMap::new();
        assert_eq!(eval("2 + 3 * 4", &e).unwrap(), 14.0);
        assert_eq!(eval("(2 + 3) * 4", &e).unwrap(), 20.0);
        assert_eq!(eval("-5 + 2", &e).unwrap(), -3.0);
        assert_eq!(eval("10 / 4", &e).unwrap(), 2.5);
        assert_eq!(eval("10 % 3", &e).unwrap(), 1.0);
        assert_eq!(eval("2 - 3 - 4", &e).unwrap(), -5.0, "left associative");
    }

    #[test]
    fn the_functions_a_placement_needs_are_there_and_in_degrees() {
        let e = BTreeMap::new();
        assert!((eval("cos(0)", &e).unwrap() - 1.0).abs() < 1e-6);
        assert!((eval("sin(90)", &e).unwrap() - 1.0).abs() < 1e-6, "DEGREES, not radians");
        assert_eq!(eval("max(3, 7)", &e).unwrap(), 7.0);
        assert_eq!(eval("pow(2, 10)", &e).unwrap(), 1024.0);
        assert_eq!(eval("lerp(10, 20, 0.5)", &e).unwrap(), 15.0);
        assert_eq!(eval("floor(3.9)", &e).unwrap(), 3.0);
    }

    #[test]
    fn the_repeat_variables_say_each_one_taller_than_the_last() {
        // the exact thing the DSL could not express, in one expression
        let heights: Vec<f32> = (0..8)
            .map(|i| eval("10 + t * 20", &repeat_vars(i, 8)).unwrap()).collect();
        assert_eq!(heights[0], 10.0);
        assert_eq!(heights[7], 30.0);
        assert!(heights.windows(2).all(|w| w[1] > w[0]), "{heights:?}");
        // and a ring laid out by hand
        let xs: Vec<f32> = (0..4).map(|i| eval("round(cos(i * 90) * 20)", &repeat_vars(i, 4)).unwrap()).collect();
        assert_eq!(xs, vec![20.0, 0.0, -20.0, 0.0]);
    }

    #[test]
    fn an_unknown_name_says_what_is_available_rather_than_failing_silently() {
        let vars = v(&[("i", 2.0), ("n", 8.0)]);
        let e = eval("grow * 2", &vars).unwrap_err();
        assert!(e.contains("'grow' is not a known value"), "{e}");
        assert!(e.contains("i, n"), "it lists what IS available: {e}");
        // the silent-ignore failure this whole module exists to prevent
        assert!(eval("i * 2", &vars).is_ok());
    }

    #[test]
    fn bad_expressions_fail_loudly_and_locally() {
        let e = BTreeMap::new();
        for bad in ["2 +", "(2 + 3", "2 / 0", "sqrt(-1)", "nosuchfn(2)", "2 $ 3", "min(1)"] {
            assert!(eval(bad, &e).is_err(), "{bad:?} should fail");
        }
        // and the error says what to do
        assert!(eval("nosuchfn(2)", &e).unwrap_err().contains("have: sin, cos"));
        assert!(eval("min(1)", &e).unwrap_err().contains("takes 2"));
    }

    #[test]
    fn a_plain_number_is_not_an_expression_so_the_common_path_stays_free() {
        assert!(!is_expr("12"));
        assert!(!is_expr("-3.5"));
        assert!(is_expr("i * 2"));
        assert!(is_expr("cos(t)"));
    }

    #[test]
    fn there_is_no_control_flow_and_that_is_deliberate() {
        // if a request needs these it needs a real language, and the
        // answer then is to generate a program, not to run the model's
        let e = BTreeMap::new();
        for not_a_language in ["if 1 then 2", "for i in 0..3", "x = 5", "print(1)"] {
            assert!(eval(not_a_language, &e).is_err(), "{not_a_language:?}");
        }
    }
}
