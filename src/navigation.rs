//! Go to definition and find references.

use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Location, Position, Range, Url};

use crate::fdepl::FdeplRefKind;
use crate::fdepl_resolve::FdeplTarget;
use crate::fidl::Span;
use crate::language::Lang;
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
    if doc.lang == Lang::Fdepl {
        return fdepl_target_at(ws, uri, pos);
    }
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

/// The Franca declaration an fdepl identifier refers to.
fn fdepl_target_at(ws: &Workspace, uri: &Url, pos: Position) -> Option<Target> {
    let doc = ws.get(uri)?;
    let idx = ws.fdepl.get(uri)?;
    let offset = doc.lines.offset(&doc.text, pos);
    let r = idx.ref_at(offset)?;
    match ws.resolve_fdepl(uri, r)? {
        FdeplTarget::Fidl(decl) => {
            let span = idx.refs[r].segments.last()?.span.clone();
            Some(Target { decl, span })
        }
        _ => None,
    }
}

fn fdepl_definition(ws: &Workspace, uri: &Url, pos: Position) -> Option<Location> {
    let doc = ws.get(uri)?;
    let idx = ws.fdepl.get(uri)?;
    let offset = doc.lines.offset(&doc.text, pos);
    if let Some(import) = idx.imports.iter().find(|i| i.span.start <= offset && offset <= i.span.end) {
        if import.is_external() {
            return None;
        }
        let target = ws.resolve_import(uri, &import.target)?;
        ws.docs.get(&target)?;
        return Some(Location::new(target, Default::default()));
    }
    let r = idx.ref_at(offset)?;
    match ws.resolve_fdepl(uri, r)? {
        FdeplTarget::Fidl(id) => location(ws, &id.uri, ws.decl(&id)?.name_span.clone()),
        FdeplTarget::Spec(u, i) | FdeplTarget::Property(u, i) => {
            location(ws, &u, ws.fdepl_decl(&u, i)?.name_span.clone())
        }
    }
}

pub fn definition(ws: &Workspace, uri: &Url, pos: Position) -> Option<Location> {
    if ws.get(uri)?.lang == Lang::Fdepl {
        return fdepl_definition(ws, uri, pos);
    }
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
    if include_declaration && let Some(d) = ws.decl(target) {
        out.extend(location(ws, &target.uri, d.name_span.clone()));
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
    let mut uris: Vec<&Url> = ws.fdepl.keys().collect();
    uris.sort();
    for uri in uris {
        for (i, r) in ws.fdepl[uri].refs.iter().enumerate() {
            if !matches!(r.kind, FdeplRefKind::Target | FdeplRefKind::InterfaceValue) {
                continue;
            }
            if ws.resolve_fdepl(uri, i) == Some(FdeplTarget::Fidl(target.clone())) {
                out.extend(location(ws, uri, r.segments.last().unwrap().span.clone()));
            }
        }
    }
    out
}

/// Occurrences of the symbol under the cursor within the same file.
pub fn highlights(ws: &Workspace, uri: &Url, pos: Position) -> Vec<DocumentHighlight> {
    let Some(doc) = ws.get(uri) else { return Vec::new() };
    let make = |range: Range, kind| DocumentHighlight { range, kind: Some(kind) };
    let mut out: Vec<DocumentHighlight> = Vec::new();
    if let Some(target) = target_at(ws, uri, pos) {
        let decl_span = ws.decl(&target.decl).map(|d| d.name_span.clone());
        for loc in references(ws, &target.decl, true) {
            if &loc.uri != uri {
                continue;
            }
            let start = doc.lines.offset(&doc.text, loc.range.start);
            let kind = if decl_span.as_ref().is_some_and(|s| s.start == start) {
                DocumentHighlightKind::WRITE
            } else {
                DocumentHighlightKind::READ
            };
            out.push(make(loc.range, kind));
        }
        return out;
    }
    if doc.lang != Lang::Fdepl {
        return out;
    }
    let Some(idx) = ws.fdepl.get(uri) else { return out };
    let offset = doc.lines.offset(&doc.text, pos);
    let wanted = match idx.ref_at(offset) {
        Some(r) => ws.resolve_fdepl(uri, r),
        None => idx.decl_at(offset).map(|i| match idx.decls[i].kind {
            crate::fdepl::FdeplKind::Property => FdeplTarget::Property(uri.clone(), i),
            _ => FdeplTarget::Spec(uri.clone(), i),
        }),
    };
    let Some(wanted) = wanted else { return out };
    let (u, i) = match &wanted {
        FdeplTarget::Spec(u, i) | FdeplTarget::Property(u, i) => (u, *i),
        FdeplTarget::Fidl(_) => return out,
    };
    if u == uri {
        let span = idx.decls[i].name_span.clone();
        out.push(make(doc.lines.range(&doc.text, span.start, span.end), DocumentHighlightKind::WRITE));
    }
    for (n, r) in idx.refs.iter().enumerate() {
        if ws.resolve_fdepl(uri, n).as_ref() == Some(&wanted) {
            let span = r.span();
            out.push(make(doc.lines.range(&doc.text, span.start, span.end), DocumentHighlightKind::READ));
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

    fn fdepl_setup() -> Workspace {
        let mut ws = setup();
        ws.open(
            uri("spec.fdepl"),
            "fdepl",
            "package s\nspecification s.Spec {\n for interface { Port: Integer; }\n}\n".into(),
            1,
        );
        ws.open(
            uri("d.fdepl"),
            "fdepl",
            "package d\nimport \"spec.fdepl\"\ndefine s.Spec for interface org.app.I {\n Port = 1\n method m { }\n}\n"
                .into(),
            1,
        );
        ws
    }

    #[test]
    fn fdepl_definitions() {
        let ws = fdepl_setup();
        let d = uri("d.fdepl");
        let spec = definition(&ws, &d, Position::new(2, 10)).unwrap();
        assert_eq!((spec.uri, spec.range.start.line), (uri("spec.fdepl"), 1));
        let iface = definition(&ws, &d, Position::new(2, 35)).unwrap();
        assert_eq!((iface.uri, iface.range.start.line), (uri("main.fidl"), 2));
        let method = definition(&ws, &d, Position::new(4, 8)).unwrap();
        assert_eq!((method.uri, method.range.start.line), (uri("main.fidl"), 4));
        let prop = definition(&ws, &d, Position::new(3, 2)).unwrap();
        assert_eq!((prop.uri, prop.range.start.line), (uri("spec.fdepl"), 2));
        let import = definition(&ws, &d, Position::new(1, 10)).unwrap();
        assert_eq!(import.uri, uri("spec.fdepl"));
    }

    #[test]
    fn fidl_references_include_deployments() {
        let ws = fdepl_setup();
        let target = target_at(&ws, &uri("main.fidl"), Position::new(2, 10)).unwrap();
        let refs = references(&ws, &target.decl, false);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].uri, uri("d.fdepl"));
        let from_fdepl = target_at(&ws, &uri("d.fdepl"), Position::new(2, 35)).unwrap();
        assert_eq!(from_fdepl.decl, target.decl);
    }

    #[test]
    fn highlights_occurrences_in_the_file_only() {
        let ws = setup();
        let h = highlights(&ws, &uri("main.fidl"), Position::new(3, 22));
        assert_eq!(h.len(), 2);
        assert!(h.iter().all(|x| x.kind == Some(DocumentHighlightKind::READ)));
        let h = highlights(&ws, &uri("common.fidl"), Position::new(2, 9));
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].kind, Some(DocumentHighlightKind::WRITE));
    }

    #[test]
    fn highlights_fdepl_specifications() {
        let mut ws = fdepl_setup();
        ws.open(
            uri("e.fdepl"),
            "fdepl",
            "package e\ndefine s.Spec for interface org.app.I { }\ndefine s.Spec for typeCollection org.app.Types { }\n".into(),
            1,
        );
        let h = highlights(&ws, &uri("e.fdepl"), Position::new(1, 10));
        assert_eq!(h.len(), 2);
    }
}
