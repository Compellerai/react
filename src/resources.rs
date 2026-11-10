//! Locating resource files that ship alongside the binary (presets, models,
//! images, texture atlases).
//!
//! A resource is addressed by its path relative to the repository root, e.g.
//! `public/images/compeller_logo.png`. The same relative path is tried in each
//! place the app can run from, in this order:
//!
//! 1. next to the executable (standalone distribution)
//! 2. `Contents/Resources` of a macOS `.app` bundle
//! 3. the repository root, when the executable is `target/<profile>/<bin>`
//!    (`cargo run` / `cargo build`)
//! 4. the working directory, then its parent

use std::path::{Path, PathBuf};

/// Every location `relative_path` could live at, in priority order.
/// The paths are not checked for existence.
pub fn candidates(relative_path: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        if let Some(exe_dir) = exe.parent() {
            paths.extend(candidates_for_exe_dir(exe_dir, relative_path));
        }
    }

    paths.push(PathBuf::from(relative_path));
    paths.push(PathBuf::from("..").join(relative_path));
    paths
}

/// The first existing location of `relative_path`, if any.
pub fn find(relative_path: &str) -> Option<PathBuf> {
    candidates(relative_path).into_iter().find(|p| p.exists())
}

/// Executable-relative candidates, split out so the layout rules are testable
/// without depending on where the test binary happens to live.
fn candidates_for_exe_dir(exe_dir: &Path, relative_path: &str) -> Vec<PathBuf> {
    let mut paths = vec![
        exe_dir.join(relative_path),
        exe_dir.join("../Resources").join(relative_path),
    ];
    // <repo>/target/<profile>/<bin>  ->  <repo>
    if let Some(repo_root) = exe_dir.parent().and_then(Path::parent) {
        paths.push(repo_root.join(relative_path));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_build_resolves_to_repo_root() {
        let paths = candidates_for_exe_dir(Path::new("/repo/target/release"), "presets");
        assert!(paths.contains(&PathBuf::from("/repo/presets")));
    }

    #[test]
    fn app_bundle_resolves_to_resources() {
        let paths = candidates_for_exe_dir(
            Path::new("/Applications/REACT.app/Contents/MacOS"),
            "public/images/compeller_logo.png",
        );
        assert_eq!(
            paths[1],
            PathBuf::from("/Applications/REACT.app/Contents/MacOS/../Resources/public/images/compeller_logo.png")
        );
    }

    #[test]
    fn executable_dir_is_tried_first() {
        let paths = candidates_for_exe_dir(Path::new("/opt/react"), "models/classes.json");
        assert_eq!(paths[0], PathBuf::from("/opt/react/models/classes.json"));
    }

    #[test]
    fn finds_resources_from_the_repo_when_run_by_cargo() {
        // Test binaries live in target/<profile>/deps, one level deeper than the
        // app binary, so the working-directory candidates (cargo runs tests from
        // the manifest dir) are what resolve here.
        assert!(find("assets/qs_numbers_atlas.png").is_some());
        assert!(find("public/images/compeller_icon_square.png").is_some());
        assert!(find("presets").is_some());
        assert!(find("does/not/exist.png").is_none());
    }
}
