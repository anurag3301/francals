use tower_lsp::lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind};

use crate::document::Document;
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Lang;

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
}
