//! Reading `.fidl` / `.fdepl` files from disk.

use std::path::{Component, Path, PathBuf};

use tower_lsp::lsp_types::Url;

use crate::document::Document;
use crate::language::Lang;

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".hg", ".svn", "build", ".cache"];
const MAX_FILES: usize = 50_000;

/// Removes `.` and `..` components without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn path_to_uri(path: &Path) -> Option<Url> {
    Url::from_file_path(normalize(path)).ok()
}

pub fn read_document(path: &Path) -> Option<(Url, Document)> {
    let lang = Lang::from_path(path.to_str()?)?;
    let text = std::fs::read_to_string(path).ok()?;
    Some((path_to_uri(path)?, Document::new(lang, text, 0)))
}

/// Finds every Franca source below `root`.
pub fn find_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ty) = entry.file_type() else { continue };
            if ty.is_dir() {
                let name = entry.file_name();
                if !SKIP_DIRS.iter().any(|s| name == *s) {
                    stack.push(path);
                }
            } else if ty.is_file() && Lang::from_path(&path.to_string_lossy()).is_some() {
                out.push(path);
                if out.len() >= MAX_FILES {
                    return out;
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_dot_segments() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d.fidl")), PathBuf::from("/a/c/d.fidl"));
    }

    #[test]
    fn finds_sources_and_skips_ignored_dirs() {
        let dir = std::env::temp_dir().join(format!("francals-loader-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("a.fidl"), "package a\n").unwrap();
        std::fs::write(dir.join("sub/b.fdepl"), "package b\n").unwrap();
        std::fs::write(dir.join("sub/c.txt"), "").unwrap();
        std::fs::write(dir.join("node_modules/d.fidl"), "package d\n").unwrap();
        let found = find_sources(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let names: Vec<_> = found.iter().map(|p| p.file_name().unwrap().to_str().unwrap().to_string()).collect();
        assert_eq!(names, ["a.fidl", "b.fdepl"]);
    }
}
