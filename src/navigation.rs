//! Go to definition and find references.

use tower_lsp::lsp_types::{Location, Position, Url};

use crate::fidl::Span;
use crate::workspace::{DeclId, Workspace};

pub fn location(ws: &Workspace, uri: &Url, span: Span) -> Option<Location> {
    let doc = ws.get(uri)?;
    Some(Location::new(uri.clone(), doc.lines.range(&doc.text, span.start, span.end)))
}

/// What the cursor is on, if it names a declaration.
pub struct Target {
    pub decl: DeclId,
    /// The identifier under the cursor.
    pub span: Span,
}

pub fn target_at(ws: &Workspace, uri: &Url, pos: Position) -> Option<Target> {
    let doc = ws.get(uri)?;
    let idx = ws.fidl.get(uri)?;
    let offset = doc.lines.offset(&doc.text, pos);
    if let Some(i) = idx.decl_at(offset) {
        let span = idx.decls[i].name_span.clone();
        return Some(Target { decl: DeclId { uri: uri.clone(), idx: i }, span });
    }
    let r = idx.ref_at(offset)?;
    let reference = &idx.refs[r];
    let resolved = ws.resolve(uri, r)?;
    let span = reference
        .segments
        .iter()
        .find(|s| s.span.start <= offset && offset <= s.span.end)
        .map(|s| s.span.clone())
        .unwrap_or_else(|| reference.segments[resolved.matched - 1].span.clone());
    Some(Target { decl: resolved.target, span })
}

pub fn definition(ws: &Workspace, uri: &Url, pos: Position) -> Option<Location> {
    if let Some(loc) = import_target(ws, uri, pos) {
        return Some(loc);
    }
    let target = target_at(ws, uri, pos)?;
    let decl = ws.decl(&target.decl)?;
    location(ws, &target.decl.uri, decl.name_span.clone())
}

/// Cursor on the path of an `import ... from "file"`.
fn import_target(ws: &Workspace, uri: &Url, pos: Position) -> Option<Location> {
    let doc = ws.get(uri)?;
    let idx = ws.fidl.get(uri)?;
    let offset = doc.lines.offset(&doc.text, pos);
    let import = idx.imports.iter().find(|i| i.span.start <= offset && offset <= i.span.end)?;
    let target = ws.resolve_import(uri, &import.uri)?;
    ws.docs.get(&target)?;
    Some(Location::new(target, Default::default()))
}

/// Every identifier that refers to `target`, optionally with its declaration.
pub fn references(ws: &Workspace, target: &DeclId, include_declaration: bool) -> Vec<Location> {
    let mut out = Vec::new();
    if include_declaration {
        if let Some(d) = ws.decl(target) {
            out.extend(location(ws, &target.uri, d.name_span.clone()));
        }
    }
    let mut uris: Vec<&Url> = ws.fidl.keys().collect();
    uris.sort();
    for uri in uris {
        let idx = &ws.fidl[uri];
        for r in &idx.refs {
            let Some(resolved) = ws.resolve_in(uri, idx, r) else { continue };
            if &resolved.target != target {
                continue;
            }
            let span = r.segments[resolved.matched - 1].span.clone();
            out.extend(location(ws, uri, span));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(name: &str) -> Url {
        Url::parse(&format!("file:///ws/{name}")).unwrap()
    }

    fn setup() -> Workspace {
        let mut ws = Workspace::default();
        ws.open(
            uri("common.fidl"),
            "franca",
            "package org.common\ntypeCollection Types {\n struct Point { Int8 x }\n}\n".into(),
            1,
        );
        ws.open(
            uri("main.fidl"),
            "franca",
            "package org.app\nimport org.common.* from \"common.fidl\"\ninterface I {\n attribute Types.Point a\n method m {\n  in { Types.Point p }\n }\n}\n".into(),
            1,
        );
        ws
    }

    #[test]
    fn goes_to_definition_across_files() {
        let ws = setup();
        // `Point` on line 3 of main.fidl
        let loc = definition(&ws, &uri("main.fidl"), Position::new(3, 22)).unwrap();
        assert_eq!(loc.uri, uri("common.fidl"));
        assert_eq!(loc.range.start, Position::new(2, 8));
        assert_eq!(loc.range.end, Position::new(2, 13));
    }

    #[test]
    fn definition_of_import_opens_file() {
        let ws = setup();
        let loc = definition(&ws, &uri("main.fidl"), Position::new(1, 30)).unwrap();
        assert_eq!(loc.uri, uri("common.fidl"));
    }

    #[test]
    fn finds_all_references_with_declaration() {
        let ws = setup();
        let target = target_at(&ws, &uri("common.fidl"), Position::new(2, 9)).unwrap();
        let with = references(&ws, &target.decl, true);
        assert_eq!(with.len(), 3);
        let without = references(&ws, &target.decl, false);
        assert_eq!(without.len(), 2);
        assert!(without.iter().all(|l| l.uri == uri("main.fidl")));
        // Only the `Point` segment is reported, not `Types.Point`.
        assert_eq!(without[0].range.end.character - without[0].range.start.character, 5);
    }

    #[test]
    fn nothing_under_whitespace() {
        let ws = setup();
        assert!(definition(&ws, &uri("main.fidl"), Position::new(0, 0)).is_none());
    }
}
