//! Download and cache the TypeScript 7 native compiler (`tsgo`) that Corsa talks to.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;

/// Pinned `@typescript/native-preview-*` version. Must expose `--api` for Corsa.
pub const TSGO_VERSION: &str = "7.0.0-dev.20260707.2";

pub fn cached_tsgo() -> Option<PathBuf> {
    let exe = tsgo_exe_path().ok()?;
    exe.is_file().then_some(exe)
}

pub fn bundled_tsgo() -> Result<PathBuf, String> {
    if let Some(exe) = cached_tsgo() {
        return Ok(exe);
    }
    let exe = tsgo_exe_path()?;
    download_and_extract(&exe)?;
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(format!(
            "downloaded TypeScript native compiler but {} is missing",
            exe.display()
        ))
    }
}

fn cache_dir() -> Result<PathBuf, String> {
    let root = if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or("cannot locate cache directory")?;
        PathBuf::from(home).join(".cache")
    };
    Ok(root.join("pragmajs").join("tsgo").join(TSGO_VERSION))
}

fn tsgo_exe_path() -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "tsgo.exe" } else { "tsgo" };
    Ok(cache_dir()?.join(name))
}

pub fn platform_package_name() -> Result<&'static str, String> {
    let spec = (std::env::consts::OS, std::env::consts::ARCH);
    match spec {
        ("linux", "x86_64") => Ok("native-preview-linux-x64"),
        ("linux", "aarch64") => Ok("native-preview-linux-arm64"),
        ("macos", "x86_64") => Ok("native-preview-darwin-x64"),
        ("macos", "aarch64") => Ok("native-preview-darwin-arm64"),
        ("windows", "x86_64") => Ok("native-preview-win32-x64"),
        ("windows", "aarch64") => Ok("native-preview-win32-arm64"),
        (os, arch) => Err(format!(
            "no bundled TypeScript native compiler for {os}-{arch}"
        )),
    }
}

fn tarball_url(package: &str) -> String {
    format!(
        "https://registry.npmjs.org/@typescript/{package}/-/{}-{TSGO_VERSION}.tgz",
        package
    )
}

fn download_and_extract(exe: &Path) -> Result<(), String> {
    let package = platform_package_name()?;
    let url = tarball_url(package);
    eprintln!("Downloading TypeScript 7 native compiler {TSGO_VERSION} ({package})…");
    let dest = exe.parent().ok_or("invalid tsgo cache path")?;
    fs::create_dir_all(dest).map_err(|error| error.to_string())?;
    let staging = dest.join(".staging");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;

    let response = ureq::get(&url)
        .call()
        .map_err(|error| format!("download {url}: {error}"))?;
    let decoder = GzDecoder::new(response.into_reader());
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("read tarball: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("tarball entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("tarball path: {error}"))?;
        let Ok(rel) = path.strip_prefix("package/lib") else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = staging.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        entry
            .unpack(&out)
            .map_err(|error| format!("extract {}: {error}", out.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let tsgo = staging.join("tsgo");
        if tsgo.is_file() {
            fs::set_permissions(&tsgo, fs::Permissions::from_mode(0o755))
                .map_err(|error| error.to_string())?;
        }
    }

    // Move extracted files into the version cache.
    for child in fs::read_dir(&staging).map_err(|error| error.to_string())? {
        let child = child.map_err(|error| error.to_string())?;
        let to = dest.join(child.file_name());
        if to.exists() {
            if to.is_dir() {
                fs::remove_dir_all(&to).map_err(|error| error.to_string())?;
            } else {
                fs::remove_file(&to).map_err(|error| error.to_string())?;
            }
        }
        fs::rename(child.path(), &to).or_else(|_| {
            copy_recursive(&child.path(), &to)?;
            if child.path().is_dir() {
                fs::remove_dir_all(child.path()).map_err(|error| error.to_string())?;
            } else {
                fs::remove_file(child.path()).map_err(|error| error.to_string())?;
            }
            Ok::<(), String>(())
        })?;
    }
    fs::remove_dir_all(&staging).ok();
    let _ = io::stderr().flush();
    Ok(())
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        fs::create_dir_all(to).map_err(|error| error.to_string())?;
        for child in fs::read_dir(from).map_err(|error| error.to_string())? {
            let child = child.map_err(|error| error.to_string())?;
            copy_recursive(&child.path(), &to.join(child.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_package_is_known_for_this_host() {
        let name = platform_package_name().expect("supported host");
        assert!(name.starts_with("native-preview-"));
        assert!(tarball_url(name).contains(TSGO_VERSION));
        assert!(tarball_url(name).contains(name));
    }
}
