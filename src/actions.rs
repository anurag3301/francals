//! Code actions: quick fixes for names that live in files that are not imported.

use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::fdepl_resolve::FdeplTarget;
use crate::language::Lang;
use crate::loader::normalize;
use crate::workspace::Workspace;

/// `target` expressed relative to the directory of `from`, with `/` separators.
pub fn relative_path(from: &Path, target: &Path) -> String {
    let from_dir = normalize(from.parent().unwrap_or(Path::new("/")));
    let from_dir: Vec<_> = from_dir.components().collect();
    let target = normalize(target);
    let target: Vec<_> = target.components().collect();
    let common = from_dir.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = from_dir[common..].iter().map(|_| "..".to_string()).collect();
    parts.extend(target[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

fn import_path(from: &Url, target: &Url) -> Option<String> {
    let (f, t): (PathBuf, PathBuf) = (from.to_file_path().ok()?, target.to_file_path().ok()?);
    Some(relative_path(&f, &t))
}

/// Where a new import line goes: after the last import, else after the package.
fn insert_position(ws: &Workspace, uri: &Url) -> Option<Position> {
    let doc = ws.get(uri)?;
    let root = doc.root();
    let mut cursor = root.walk();
    let mut end = None;
    for c in root.children(&mut cursor) {
        if matches!(c.kind(), "package_declaration" | "import_declaration") {
            end = Some(c.end_byte());
        }
    }
    let line = doc.lines.position(&doc.text, end?).line + 1;
    Some(Position::new(line, 0))
}

fn imported(ws: &Workspace, uri: &Url) -> Vec<Url> {
    let mut out = Vec::new();
    if let Some(idx) = ws.fidl.get(uri) {
        out.extend(idx.imports.iter().filter_map(|i| ws.resolve_import(uri, &i.uri)));
    }
    if let Some(idx) = ws.fdepl.get(uri) {
        out.extend(idx.imports.iter().filter(|i| i.is_uri).filter_map(|i| ws.resolve_import(uri, &i.target)));
    }
    out
}

/// The file defining the name under the cursor and the import line it needs.
fn missing_import(ws: &Workspace, uri: &Url, pos: Position) -> Option<(String, String)> {
    let doc = ws.get(uri)?;
    let offset = doc.lines.offset(&doc.text, pos);
    let (target_uri, line, name) = match doc.lang {
        Lang::Fidl => {
            let idx = ws.fidl.get(uri)?;
            let r = idx.ref_at(offset)?;
            let resolved = ws.resolve(uri, r)?;
            let decl = ws.decl(&resolved.target)?;
            let pkg = ws.fidl.get(&resolved.target.uri)?.package.clone()?;
            let path = import_path(uri, &resolved.target.uri)?;
            (resolved.target.uri.clone(), format!("import {pkg}.* from \"{path}\""), decl.name.clone())
        }
        Lang::Fdepl => {
            let idx = ws.fdepl.get(uri)?;
            let r = idx.ref_at(offset)?;
            let (u, name) = match ws.resolve_fdepl(uri, r)? {
                FdeplTarget::Fidl(id) if idx.refs[r].parent.is_none() => (id.uri.clone(), ws.decl(&id)?.name.clone()),
                FdeplTarget::Spec(u, i) => (u.clone(), ws.fdepl_decl(&u, i)?.name.clone()),
                _ => return None,
            };
            let path = import_path(uri, &u)?;
            (u, format!("import \"{path}\""), name)
        }
    };
    if &target_uri == uri || imported(ws, uri).contains(&target_uri) {
        return None;
    }
    Some((line, name))
}

pub fn code_actions(ws: &Workspace, uri: &Url, range: Range) -> Vec<CodeActionOrCommand> {
    let Some((line, name)) = missing_import(ws, uri, range.start) else { return Vec::new() };
    let Some(at) = insert_position(ws, uri) else { return Vec::new() };
    let edit = TextEdit { range: Range::new(at, at), new_text: format!("{line}\n") };
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add import for `{name}`"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some([(uri.clone(), vec![edit])].into_iter().collect()),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(n: &str) -> Url {
        Url::parse(&format!("file:///ws/{n}")).unwrap()
    }

    fn edit_text(actions: &[CodeActionOrCommand]) -> (String, u32) {
        let CodeActionOrCommand::CodeAction(a) = &actions[0] else { panic!() };
        let e = &a.edit.as_ref().unwrap().changes.as_ref().unwrap().values().next().unwrap()[0];
        (e.new_text.clone(), e.range.start.line)
    }

    #[test]
    fn computes_relative_paths() {
        assert_eq!(relative_path(Path::new("/a/b/x.fidl"), Path::new("/a/b/y.fidl")), "y.fidl");
        assert_eq!(relative_path(Path::new("/a/b/x.fidl"), Path::new("/a/c/d/y.fidl")), "../c/d/y.fidl");
    }

    #[test]
    fn offers_import_for_unimported_type() {
        let mut ws = Workspace::default();
        ws.open(uri("lib/c.fidl"), "franca", "package org.c\ntypeCollection T { struct S { Int8 f } }\n".into(), 1);
        let src = "package org.m\ninterface I {\n attribute org.c.T.S s\n}\n";
        ws.open(uri("m.fidl"), "franca", src.into(), 1);
        let acts = code_actions(&ws, &uri("m.fidl"), Range::new(Position::new(2, 19), Position::new(2, 19)));
        assert_eq!(edit_text(&acts), ("import org.c.* from \"lib/c.fidl\"\n".to_string(), 1));
    }

    #[test]
    fn no_action_when_already_imported() {
        let mut ws = Workspace::default();
        ws.open(uri("c.fidl"), "franca", "package org.c\ntypeCollection T { struct S { Int8 f } }\n".into(), 1);
        let src = "package org.m\nimport org.c.* from \"c.fidl\"\ninterface I {\n attribute T.S s\n}\n";
        ws.open(uri("m.fidl"), "franca", src.into(), 1);
        let p = Position::new(3, 12);
        assert!(code_actions(&ws, &uri("m.fidl"), Range::new(p, p)).is_empty());
    }

    #[test]
    fn offers_import_for_fdepl_targets() {
        let mut ws = Workspace::default();
        ws.open(uri("a.fidl"), "franca", "package org.a\ninterface I { }\n".into(), 1);
        let src = "package d\ndefine x.Spec for interface org.a.I {\n}\n";
        ws.open(uri("d.fdepl"), "fdepl", src.into(), 1);
        let p = Position::new(1, 30);
        let acts = code_actions(&ws, &uri("d.fdepl"), Range::new(p, p));
        assert_eq!(edit_text(&acts), ("import \"a.fidl\"\n".to_string(), 1));
    }
}
