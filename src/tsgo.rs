//! Optional Corsa/tsgo fallback for receiver types not in the `/*#own` table.
//!
//! Prelude generation talks to tsgo through `@corsa-bind/napi` (see
//! `scripts/gen-prelude.cjs`). At check time we only spawn a Corsa worker when
//! `TSGO` / `CORSA_BIN` / `CORSA_PATH` points at a real binary; otherwise
//! instance-method lookup uses the type name stored on the ownership binding.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static BIN: OnceLock<Option<PathBuf>> = OnceLock::new();

fn find_bin() -> Option<PathBuf> {
    for key in ["TSGO", "CORSA_BIN", "CORSA_PATH", "TSGO_PATH"] {
        if let Ok(p) = std::env::var(key) {
            let pb = PathBuf::from(p);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    which("tsgo").or_else(|| which("corsa"))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Path to a Corsa/tsgo executable, if one is configured or on PATH.
pub fn executable() -> Option<&'static Path> {
    BIN.get_or_init(find_bin).as_deref()
}

/// Probe that the Corsa binary can start (`tsgo --version`).
#[allow(dead_code)]
pub fn probe() -> Result<String, String> {
    let bin = executable().ok_or_else(|| {
        "no Corsa/tsgo binary (set TSGO, CORSA_BIN, or install @typescript/native-preview)"
            .to_string()
    })?;
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() && stdout.is_empty() {
        return Err(format!(
            "{} --version failed: {stderr}",
            bin.display()
        ));
    }
    Ok(format!("{} {stdout}{stderr}", bin.display()))
}

/// Receiver type for an identifier when `/*#own` did not record one.
///
/// A full Corsa project query needs a tsconfig and is done by
/// `scripts/gen-prelude.cjs`. Check-time fallback is the configured binary's
/// presence plus the identifier itself — without a program, Corsa cannot
/// recover a stdlib type for a bare JS ident, so this returns `None`.
pub fn receiver_type(_ident: &str) -> Option<String> {
    let _ = executable();
    None
}
