use tower_lsp::lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind};

use crate::document::Document;
use crate::fdepl::{FdeplIndex, FdeplKind};
use crate::fidl::{DeclKind, FidlIndex};
use crate::workspace::Workspace;

fn symbol_kind(kind: DeclKind) -> SymbolKind {
    match kind {
        DeclKind::TypeCollection => SymbolKind::NAMESPACE,
        DeclKind::Interface => SymbolKind::INTERFACE,
        DeclKind::Attribute => SymbolKind::PROPERTY,
        DeclKind::Method => SymbolKind::METHOD,
        DeclKind::Broadcast => SymbolKind::EVENT,
        DeclKind::Constant => SymbolKind::CONSTANT,
        DeclKind::Array => SymbolKind::ARRAY,
        DeclKind::Typedef => SymbolKind::CLASS,
        DeclKind::Struct | DeclKind::Union => SymbolKind::STRUCT,
        DeclKind::Enumeration => SymbolKind::ENUM,
        DeclKind::Map => SymbolKind::OBJECT,
        DeclKind::Field => SymbolKind::FIELD,
        DeclKind::Enumerator | DeclKind::State => SymbolKind::ENUM_MEMBER,
        DeclKind::Argument | DeclKind::Variable => SymbolKind::VARIABLE,
    }
}

/// Hierarchical outline of a Franca IDL file.
pub fn fidl_symbols(doc: &Document, idx: &FidlIndex) -> Vec<DocumentSymbol> {
    build(doc, idx, None)
}

#[allow(deprecated)]
fn build(doc: &Document, idx: &FidlIndex, parent: Option<usize>) -> Vec<DocumentSymbol> {
    (0..idx.decls.len())
        .filter(|&i| idx.decls[i].parent == parent)
        .map(|i| {
            let d = &idx.decls[i];
            let name = if d.name.is_empty() { "typeCollection".to_string() } else { d.name.clone() };
            let children = build(doc, idx, Some(i));
            DocumentSymbol {
                name,
                detail: Some(d.kind.label().to_string()),
                kind: symbol_kind(d.kind),
                tags: None,
                deprecated: None,
                range: doc.lines.range(&doc.text, d.span.start, d.span.end),
                selection_range: doc.lines.range(&doc.text, d.name_span.start, d.name_span.end),
                children: if children.is_empty() { None } else { Some(children) },
            }
        })
        .collect()
}

fn fdepl_symbol_kind(kind: FdeplKind, keyword: &str) -> SymbolKind {
    match kind {
        FdeplKind::Specification => SymbolKind::INTERFACE,
        FdeplKind::Host => SymbolKind::NAMESPACE,
        FdeplKind::Property => SymbolKind::PROPERTY,
        FdeplKind::Deployment => SymbolKind::MODULE,
        FdeplKind::Member => match keyword {
            "method" => SymbolKind::METHOD,
            "broadcast" => SymbolKind::EVENT,
            "attribute" => SymbolKind::PROPERTY,
            "struct" | "union" => SymbolKind::STRUCT,
            "enumeration" => SymbolKind::ENUM,
            "enumerator" => SymbolKind::ENUM_MEMBER,
            "field" | "argument" => SymbolKind::FIELD,
            _ => SymbolKind::OBJECT,
        },
    }
}

/// Hierarchical outline of a Franca deployment file.
pub fn fdepl_symbols(doc: &Document, idx: &FdeplIndex) -> Vec<DocumentSymbol> {
    build_fdepl(doc, idx, None)
}

#[allow(deprecated)]
fn build_fdepl(doc: &Document, idx: &FdeplIndex, parent: Option<usize>) -> Vec<DocumentSymbol> {
    (0..idx.decls.len())
        .filter(|&i| idx.decls[i].parent == parent)
        .map(|i| {
            let d = &idx.decls[i];
            let children = build_fdepl(doc, idx, Some(i));
            let detail = if d.keyword.is_empty() { d.kind.label().to_string() } else { d.keyword.clone() };
            DocumentSymbol {
                name: if d.name.is_empty() { detail.clone() } else { d.name.clone() },
                detail: Some(detail),
                kind: fdepl_symbol_kind(d.kind, &d.keyword),
                tags: None,
                deprecated: None,
                range: doc.lines.range(&doc.text, d.span.start, d.span.end),
                selection_range: doc.lines.range(&doc.text, d.name_span.start, d.name_span.end),
                children: if children.is_empty() { None } else { Some(children) },
            }
        })
        .collect()
}

/// Declarations across the workspace whose name contains `query`.
#[allow(deprecated)]
pub fn workspace_symbols(ws: &Workspace, query: &str) -> Vec<SymbolInformation> {
    let needle = query.to_lowercase();
    let mut out = Vec::new();
    for (uri, idx) in &ws.fidl {
        let Some(doc) = ws.get(uri) else { continue };
        for d in &idx.decls {
            if d.name.is_empty() || !d.name.to_lowercase().contains(&needle) {
                continue;
            }
            if matches!(d.kind, DeclKind::Argument | DeclKind::Field | DeclKind::Enumerator) && needle.is_empty() {
                continue;
            }
            out.push(SymbolInformation {
                name: d.name.clone(),
                kind: symbol_kind(d.kind),
                tags: None,
                deprecated: None,
                location: Location::new(uri.clone(), doc.lines.range(&doc.text, d.name_span.start, d.name_span.end)),
                container_name: d.parent.map(|p| idx.decls[p].fqn.clone()),
            });
            if out.len() >= 500 {
                return out;
            }
        }
    }
    for (uri, idx) in &ws.fdepl {
        let Some(doc) = ws.get(uri) else { continue };
        for d in &idx.decls {
            let wanted = matches!(d.kind, FdeplKind::Specification | FdeplKind::Deployment);
            if !wanted || d.name.is_empty() || !d.name.to_lowercase().contains(&needle) {
                continue;
            }
            out.push(SymbolInformation {
                name: d.name.clone(),
                kind: fdepl_symbol_kind(d.kind, &d.keyword),
                tags: None,
                deprecated: None,
                location: Location::new(uri.clone(), doc.lines.range(&doc.text, d.name_span.start, d.name_span.end)),
                container_name: None,
            });
            if out.len() >= 500 {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Lang;
    use tower_lsp::lsp_types::Url;

    #[test]
    fn nests_members_under_containers() {
        let src = "package a\ninterface I {\n attribute Int8 x\n struct S { Int8 f }\n}\n";
        let doc = Document::new(Lang::Fidl, src.into(), 0);
        let idx = FidlIndex::build(&doc);
        let syms = fidl_symbols(&doc, &idx);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "I");
        assert_eq!(syms[0].kind, SymbolKind::INTERFACE);
        let kids = syms[0].children.as_ref().unwrap();
        assert_eq!(kids.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["x", "S"]);
        assert_eq!(kids[1].children.as_ref().unwrap()[0].name, "f");
        assert_eq!(kids[0].selection_range.start.line, 2);
    }

    #[test]
    fn fdepl_outline_nests_deployments() {
        let doc = Document::new(Lang::Fdepl, crate::fdepl::SAMPLE.into(), 0);
        let idx = FdeplIndex::build(&doc);
        let syms = fdepl_symbols(&doc, &idx);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["org.example.Spec", "org.example.demo.Demo"]);
        let demo = syms[1].children.as_ref().unwrap();
        assert_eq!(demo.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["move", "origin"]);
        assert_eq!(demo[0].kind, SymbolKind::METHOD);
    }

    #[test]
    fn workspace_search_includes_deployments() {
        let mut ws = Workspace::default();
        let u = Url::parse("file:///tmp/d.fdepl").unwrap();
        ws.open(u, "fdepl", crate::fdepl::SAMPLE.into(), 1);
        let found = workspace_symbols(&ws, "demo");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SymbolKind::MODULE);
    }
}
