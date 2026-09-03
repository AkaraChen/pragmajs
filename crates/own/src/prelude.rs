//! Runtime builtin ownership preludes (Node / Bun / Deno).
//!
//! Signatures are hand-annotated from published type packages; TS `.d.ts`
//! files do not encode unique/affine/&mut.

use crate::annot::{parse_fn_sig_str, FnSig};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Which runtime builtin set to load. Default is Node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Runtime {
    #[default]
    Node,
    Bun,
    Deno,
    None,
}

impl Runtime {
    pub fn parse(s: &str) -> Option<Runtime> {
        match s {
            "node" => Some(Runtime::Node),
            "bun" => Some(Runtime::Bun),
            "deno" => Some(Runtime::Deno),
            "none" | "off" | "no" => Some(Runtime::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Node => "node",
            Runtime::Bun => "bun",
            Runtime::Deno => "deno",
            Runtime::None => "none",
        }
    }
}

const NODE_OWN: &str = include_str!("../preludes/node.own");
const BUN_OWN: &str = include_str!("../preludes/bun.own");
const DENO_OWN: &str = include_str!("../preludes/deno.own");

static NODE_SIGS: OnceLock<HashMap<String, FnSig>> = OnceLock::new();
static BUN_SIGS: OnceLock<HashMap<String, FnSig>> = OnceLock::new();
static DENO_SIGS: OnceLock<HashMap<String, FnSig>> = OnceLock::new();

/// Ownership signatures for `runtime`. Empty for [`Runtime::None`].
pub fn signatures(runtime: Runtime) -> HashMap<String, FnSig> {
    match runtime {
        Runtime::None => HashMap::new(),
        Runtime::Node => NODE_SIGS.get_or_init(|| load(NODE_OWN, true)).clone(),
        Runtime::Deno => DENO_SIGS.get_or_init(|| load(DENO_OWN, false)).clone(),
        Runtime::Bun => BUN_SIGS
            .get_or_init(|| {
                let mut m = load(NODE_OWN, true);
                m.extend(load(BUN_OWN, false));
                m
            })
            .clone(),
    }
}

fn load(text: &str, fs_aliases: bool) -> HashMap<String, FnSig> {
    let mut map = HashMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, sigsrc) = split_name_sig(line)
            .unwrap_or_else(|| panic!("prelude line {}: missing signature: {line}", i + 1));
        let sig = parse_fn_sig_str(sigsrc)
            .unwrap_or_else(|e| panic!("prelude line {}: {e}: {line}", i + 1));
        map.insert(name.to_string(), sig);
    }
    if fs_aliases {
        let extra: Vec<(String, FnSig)> = map
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("fs.")
                    .filter(|rest| !rest.starts_with("promises."))
                    .map(|rest| (rest.to_string(), v.clone()))
            })
            .collect();
        for (k, v) in extra {
            map.entry(k).or_insert(v);
        }
        let stream: Vec<(String, FnSig)> = map
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("WriteStream#").map(|meth| {
                    let sig = strip_this_param(v);
                    vec![
                        (format!("process.stdout.{meth}"), sig.clone()),
                        (format!("process.stderr.{meth}"), sig),
                    ]
                })
            })
            .flatten()
            .collect();
        for (k, v) in stream {
            map.entry(k).or_insert(v);
        }
    }
    map
}

fn strip_this_param(sig: &FnSig) -> FnSig {
    let params = if sig.params.first().map(|(n, _)| n == "this").unwrap_or(false) {
        sig.params.iter().skip(1).cloned().collect()
    } else {
        sig.params.clone()
    };
    FnSig {
        params,
        ret: sig.ret.clone(),
    }
}

fn split_name_sig(line: &str) -> Option<(&str, &str)> {
    let open = line.find('(')?;
    let name = line[..open].trim();
    if name.is_empty() {
        return None;
    }
    Some((name, line[open..].trim()))
}

/// Names present in a runtime prelude (for tests and inventory dumps).
#[cfg(test)]
fn names(runtime: Runtime) -> Vec<String> {
    let mut v: Vec<String> = signatures(runtime).keys().cloned().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_prelude_has_real_builtins() {
        let n = names(Runtime::Node);
        for need in [
            "console.log",
            "fs.readFile",
            "readFile",
            "Buffer.from",
            "path.join",
            "setTimeout",
            "fs.closeSync",
            "process.exit",
            "child_process.spawn",
        ] {
            assert!(n.iter().any(|s| s == need), "missing {need} in {n:?}");
        }
        assert!(!n.iter().any(|s| s == "Bun.file"));
        assert!(!n.iter().any(|s| s == "Deno.readFile"));
    }

    #[test]
    fn bun_and_deno_are_distinct() {
        let bun = names(Runtime::Bun);
        let deno = names(Runtime::Deno);
        let node = names(Runtime::Node);
        assert!(bun.iter().any(|s| s == "Bun.file"));
        assert!(bun.iter().any(|s| s == "console.log")); // bun includes node
        assert!(deno.iter().any(|s| s == "Deno.readFile"));
        assert!(!node.iter().any(|s| s == "Bun.file"));
        assert!(!node.iter().any(|s| s == "Deno.readFile"));
        assert!(!deno.iter().any(|s| s == "Bun.file"));
        assert!(!bun.iter().any(|s| s == "Deno.readFile"));
        assert!(names(Runtime::None).is_empty());
        for need in ["Buffer#toString", "FileHandle#close"] {
            assert!(
                node.iter().any(|s| s == need),
                "missing instance method {need}"
            );
        }
        assert!(bun.iter().any(|s| s == "BunFile#text"));
        assert!(deno.iter().any(|s| s == "FsFile#close"));
    }
}
