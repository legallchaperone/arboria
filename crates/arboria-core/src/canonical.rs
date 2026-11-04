//! JSON exactly as Python's `json.dumps` writes it. Ports `arboria.slurm.canonical_json`
//! (the approval-card hash input) and the `json.dumps(..., ensure_ascii=False,
//! sort_keys=True)` the Python code uses for ledger columns (`events.payload_json`,
//! `runs.request_json`, `tasks.payload_json`, `decisions.payload_json`).
//!
//! Invariants (must match Python byte for byte; checked by `tests/vectors/canonical_json.json`):
//! - object keys sorted by code point (Python `sort_keys=True`), whatever the map type;
//! - `ensure_ascii=False`: non-ASCII is written as is; only `"`, `\`, and U+0000..U+001F are
//!   escaped, as `\"` `\\` `\n` `\r` `\t` `\b` `\f` or lowercase `\u00xx`; U+007F,
//!   U+2028 and U+2029 are not escaped;
//! - integers as written; floats as Python `repr(float)`: shortest round-trip digits,
//!   always with a `.0` or an exponent (`1.0`, `110.898`, `1e-05`, `1e+16`, `-0.0`);
//! - separators `,` / `:` ([`canonical_json`]) or `, ` / `: ` ([`ledger_json`]).
//!
//! Limits: integers outside i64/u64 cannot be represented by `serde_json::Value` (no
//! `arbitrary_precision`); Python's `NaN` / `Infinity` are not valid JSON and cannot occur
//! in a `Value` either ([`python_float_repr`] still formats them like Python).

use serde::Serialize;
use serde_json::{Number, Value};

/// Separators between items and between a key and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separators {
    /// `(",", ":")`: the card hash input.
    Compact,
    /// Python's default `(", ", ": ")`: ledger JSON columns.
    Spaced,
}

/// `json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.
pub fn canonical_json(v: &Value) -> String {
    dumps(v, Separators::Compact)
}

/// `json.dumps(v, ensure_ascii=False, sort_keys=True)`, the format of the ledger's JSON
/// columns.
pub fn ledger_json(v: &Value) -> String {
    dumps(v, Separators::Spaced)
}

/// [`canonical_json`] of any serializable value.
pub fn to_canonical_json<T: Serialize + ?Sized>(v: &T) -> crate::Result<String> {
    Ok(canonical_json(&serde_json::to_value(v)?))
}

/// [`ledger_json`] of any serializable value.
pub fn to_ledger_json<T: Serialize + ?Sized>(v: &T) -> crate::Result<String> {
    Ok(ledger_json(&serde_json::to_value(v)?))
}

/// `json.dumps(v, indent=2, ensure_ascii=False)`: keys in the map's order (insertion
/// order, serde_json's `preserve_order`), what arb writes into settings files and prints
/// for `--json`.
pub fn python_pretty(v: &Value) -> String {
    let mut out = String::new();
    write_pretty(&mut out, v, 0);
    out
}

fn write_pretty(out: &mut String, v: &Value, depth: usize) {
    let pad = |out: &mut String, d: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(d));
    };
    match v {
        Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                write_pretty(out, x, depth + 1);
            }
            pad(out, depth);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            out.push('{');
            for (i, (k, x)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                write_string(out, k);
                out.push_str(": ");
                write_pretty(out, x, depth + 1);
            }
            pad(out, depth);
            out.push('}');
        }
        Value::Array(_) => out.push_str("[]"),
        Value::Object(_) => out.push_str("{}"),
        v => write_value(out, v, Separators::Spaced, false),
    }
}

/// Serialize with sorted keys and the given separators.
pub fn dumps(v: &Value, sep: Separators) -> String {
    let mut out = String::new();
    write_value(&mut out, v, sep, true);
    out
}

/// `json.dumps(v, ensure_ascii=False)`: keys in the map's (insertion) order, spaced
/// separators (the hook's output line).
pub fn python_dumps(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v, Separators::Spaced, false);
    out
}

fn write_value(out: &mut String, v: &Value, sep: Separators, sort: bool) {
    let (item, key) = match sep {
        Separators::Compact => (",", ":"),
        Separators::Spaced => (", ", ": "),
    };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(item);
                }
                write_value(out, x, sep, sort);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // Sort explicitly: the Map type depends on serde_json features of the whole
            // dependency graph (`preserve_order`). String order is code-point order.
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if sort {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            out.push('{');
            for (i, (k, x)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(item);
                }
                write_string(out, k);
                out.push_str(key);
                write_value(out, x, sep, sort);
            }
            out.push('}');
        }
    }
}

fn write_number(out: &mut String, n: &Number) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else if let Some(f) = n.as_f64() {
        out.push_str(&python_float_repr(f));
    } else {
        // Unreachable without serde_json's arbitrary_precision feature.
        out.push_str(&n.to_string());
    }
}

/// A JSON string literal as Python writes it with `ensure_ascii=False`.
pub fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's `repr(float)` (and `json.dumps` of a float): the shortest digits that round
/// trip, in fixed notation when the decimal exponent is in -4..=15, else `d.ddde±XX`.
/// Non-finite values as `json.dumps` writes them: `NaN`, `Infinity`, `-Infinity`.
pub fn python_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    // Rust's `{:e}` without precision prints the shortest round-trip digits, like
    // Python's repr (both are correctly rounded "shortest" algorithms).
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if f < 0.0 { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let body = if exp >= 0 {
            let int_len = exp as usize + 1;
            if digits.len() <= int_len {
                format!("{digits}{}.0", "0".repeat(int_len - digits.len()))
            } else {
                format!("{}.{}", &digits[..int_len], &digits[int_len..])
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        };
        format!("{sign}{body}")
    } else {
        let mant = if digits.len() > 1 {
            format!("{}.{}", &digits[..1], &digits[1..])
        } else {
            digits
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{mant}e{esign}{:02}", exp.abs())
    }
}

/// Python's `format(f, "g")` (`f"{x:g}"`): 6 significant digits, trailing zeros
/// dropped, fixed notation when the exponent is in -4..6 (`1`, `1.5`, `0.0001`,
/// `1e-05`, `1e+06`).
pub fn python_format_g(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    // the exponent after rounding to 6 significant digits decides the notation
    let sci = format!("{f:.5e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if (-4..6).contains(&exp) {
        trim(&format!("{f:.*}", (5 - exp) as usize))
    } else {
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{}e{esign}{:02}", trim(mantissa), exp.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_g_is_pythons() {
        // f"{x:g}" in CPython 3.13
        for (x, want) in [
            (1.0, "1"),
            (2.0, "2"),
            (1.5, "1.5"),
            (0.0, "0"),
            (-0.0, "-0"),
            (1000.0, "1000"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (1e6, "1e+06"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.30000000000000004, "0.3"),
            (999999.5, "1e+06"),
            (2.5e-5, "2.5e-05"),
            (12.345678, "12.3457"),
            (-3.25, "-3.25"),
            (1e100, "1e+100"),
        ] {
            assert_eq!(python_format_g(x), want, "{x}");
        }
    }
    use serde_json::json;

    #[test]
    fn float_repr_matches_python() {
        // Expected values from CPython 3.13 `repr(float(x))`.
        let cases: &[(f64, &str)] = &[
            (110.898, "110.898"),
            (1.0, "1.0"),
            (0.5, "0.5"),
            (-2.5, "-2.5"),
            (0.1, "0.1"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000123, "0.000123"),
            (0.0000123, "1.23e-05"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345.6, "123456789012345.6"),
            (9999999999999998.0, "9999999999999998.0"),
            (1e100, "1e+100"),
            (1e-100, "1e-100"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (13.888888, "13.888888"),
            (9.01726, "9.01726"),
            (0.30000000000000004, "0.30000000000000004"),
            (2.0 / 3.0, "0.6666666666666666"),
            (100.0, "100.0"),
            (12.34, "12.34"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
        ];
        for (f, want) in cases {
            assert_eq!(python_float_repr(*f), *want, "repr({f:?})");
        }
        assert_eq!(python_float_repr(f64::NAN), "NaN");
        assert_eq!(python_float_repr(f64::INFINITY), "Infinity");
        assert_eq!(python_float_repr(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn canonical_sorts_keys_and_is_compact() {
        let v = json!({"b": 1, "a": [1, 2.0, "x"], "c": {"z": null, "y": true}});
        assert_eq!(
            canonical_json(&v),
            r#"{"a":[1,2.0,"x"],"b":1,"c":{"y":true,"z":null}}"#
        );
        assert_eq!(
            ledger_json(&v),
            r#"{"a": [1, 2.0, "x"], "b": 1, "c": {"y": true, "z": null}}"#
        );
        assert_eq!(ledger_json(&json!({})), "{}");
        assert_eq!(ledger_json(&json!([])), "[]");
    }

    #[test]
    fn keys_sort_by_code_point() {
        // Python: sorted(["b", "B", "é", "a", "日", "_", "aa"]) — uppercase before
        // lowercase, non-ASCII after ASCII.
        let v = json!({"b": 0, "B": 0, "é": 0, "a": 0, "日": 0, "_": 0, "aa": 0, "𝔸": 0});
        assert_eq!(
            canonical_json(&v),
            r#"{"B":0,"_":0,"a":0,"aa":0,"b":0,"é":0,"日":0,"𝔸":0}"#
        );
    }

    #[test]
    fn strings_escape_like_python_ensure_ascii_false() {
        let s = "q\"b\\n\nr\rt\tb\u{08}f\u{0c}u\u{01}\u{1f}del\u{7f}ls\u{2028}中文😀";
        assert_eq!(
            canonical_json(&json!(s)),
            "\"q\\\"b\\\\n\\nr\\rt\\tb\\bf\\fu\\u0001\\u001fdel\u{7f}ls\u{2028}中文😀\""
        );
    }

    #[test]
    fn numbers_keep_int_float_distinction() {
        let v: Value = serde_json::from_str(
            r#"[1, 1.0, -3, 18446744073709551615, 110.898, 1e-05, 1E16, 0.1]"#,
        )
        .unwrap();
        assert_eq!(
            canonical_json(&v),
            "[1,1.0,-3,18446744073709551615,110.898,1e-05,1e+16,0.1]"
        );
    }

    #[test]
    fn serializable_structs_go_through_value() {
        #[derive(Serialize)]
        struct S {
            z: Option<i32>,
            a: f64,
        }
        assert_eq!(
            to_canonical_json(&S { z: None, a: 1.0 }).unwrap(),
            r#"{"a":1.0,"z":null}"#
        );
    }
}
