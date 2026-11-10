//! `francals check`: print diagnostics for files without an editor.

use std::path::PathBuf;

use tower_lsp::lsp_types::DiagnosticSeverity;

use crate::diagnostics;
use crate::loader;
use crate::workspace::Workspace;

pub struct Summary {
    pub files: usize,
    pub errors: usize,
    pub warnings: usize,
}

/// Loads every source below `paths` (files or directories) and prints diagnostics to stdout.
pub fn run(paths: &[PathBuf], quiet_warnings: bool) -> Summary {
    let mut ws = Workspace::default();
    let mut files = Vec::new();
    for p in paths {
        let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        if abs.is_dir() {
            ws.roots.push(abs.clone());
            files.extend(loader::find_sources(&abs));
        } else {
            if let Some(parent) = abs.parent() {
                ws.roots.push(parent.to_path_buf());
            }
            files.push(abs);
        }
    }
    let mut uris = Vec::new();
    for f in &files {
        if let Some(uri) = ws.load_file(f) {
            uris.push((uri, f.clone()));
        }
    }
    for (uri, _) in uris.clone() {
        ws.load_imports(&uri);
    }

    let mut summary = Summary { files: uris.len(), errors: 0, warnings: 0 };
    for (uri, path) in &uris {
        for d in diagnostics::diagnostics(&ws, uri) {
            let is_error = d.severity == Some(DiagnosticSeverity::ERROR);
            if is_error {
                summary.errors += 1;
            } else {
                summary.warnings += 1;
                if quiet_warnings {
                    continue;
                }
            }
            println!(
                "{}:{}:{}: {}: {}",
                path.display(),
                d.range.start.line + 1,
                d.range.start.character + 1,
                if is_error { "error" } else { "warning" },
                d.message
            );
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_errors_and_warnings() {
        let dir = std::env::temp_dir().join(format!("francals-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.fidl"), "package a\ninterface I { }\n").unwrap();
        std::fs::write(dir.join("bad.fidl"), "package b\ninterface J { attribute Nope x }\ninterface K {\n").unwrap();
        std::fs::write(dir.join("warn.fidl"), "package c\ninterface L { attribute Nope x }\n").unwrap();
        let s = run(&[dir.clone()], true);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(s.files, 3);
        assert_eq!(s.errors, 1);
        assert_eq!(s.warnings, 1);
    }
}
