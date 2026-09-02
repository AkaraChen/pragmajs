//! Browser/WASM bindings for the playground.

use crate::{check_source_with, Runtime};
use wasm_bindgen::prelude::*;

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Check `source` and return a JSON array of diagnostics.
#[wasm_bindgen]
pub fn check(filename: &str, source: &str, runtime: &str) -> String {
    let rt = Runtime::parse(runtime).unwrap_or_default();
    let result = check_source_with(filename, source, rt);
    let mut out = String::from("[");
    for (i, d) in result.diagnostics.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let (line, col) = d.line_col(source);
        out.push_str(&format!(
            "{{\"line\":{line},\"col\":{col},\"kind\":\"{}\",\"message\":\"{}\"}}",
            d.kind.slug(),
            json_escape(&d.message)
        ));
    }
    out.push(']');
    out
}
