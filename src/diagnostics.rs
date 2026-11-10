use std::collections::HashSet;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Url};
use tree_sitter::Node;

use crate::document::Document;
use crate::fidl::{DeclKind, RefKind, Span};
use crate::language::Lang;
use crate::workspace::Workspace;

pub const SOURCE: &str = "francals";

/// Syntax errors reported by tree-sitter, as LSP diagnostics.
pub fn syntax_diagnostics(doc: &Document) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    collect(doc, doc.root(), &mut out);
    out
}

/// Syntax errors plus, for Franca files, unresolved names and duplicates.
pub fn diagnostics(ws: &Workspace, uri: &Url) -> Vec<Diagnostic> {
    let Some(doc) = ws.get(uri) else { return Vec::new() };
    let mut out = syntax_diagnostics(doc);
    if doc.lang == Lang::Fidl && !doc.root().has_error() {
        out.extend(fidl_semantic(ws, uri, doc));
    }
    out
}

fn diag(doc: &Document, span: Span, severity: DiagnosticSeverity, message: String) -> Diagnostic {
    Diagnostic {
        range: doc.lines.range(&doc.text, span.start, span.end),
        severity: Some(severity),
        source: Some(SOURCE.into()),
        message,
        ..Default::default()
    }
}

fn fidl_semantic(ws: &Workspace, uri: &Url, doc: &Document) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(idx) = ws.fidl.get(uri) else { return out };

    for import in &idx.imports {
        if ws.resolve_import(uri, &import.uri).is_none() {
            out.push(diag(
                doc,
                import.uri_span.clone(),
                DiagnosticSeverity::WARNING,
                format!("Cannot find imported file `{}`", import.uri),
            ));
        }
    }

    for r in &idx.refs {
        if matches!(r.kind, RefKind::Type | RefKind::Base) && ws.resolve_in(uri, idx, r).is_none() {
            out.push(diag(
                doc,
                r.span(),
                DiagnosticSeverity::WARNING,
                format!("Cannot resolve `{}`", r.name(r.segments.len())),
            ));
        }
    }

    let mut seen = HashSet::new();
    for d in &idx.decls {
        if d.name.is_empty() || d.kind == DeclKind::Argument {
            continue;
        }
        if !seen.insert(d.fqn.clone()) {
            out.push(diag(
                doc,
                d.name_span.clone(),
                DiagnosticSeverity::ERROR,
                format!("Duplicate definition of `{}`", d.name),
            ));
        }
    }
    out
}

fn collect(doc: &Document, node: Node, out: &mut Vec<Diagnostic>) {
    if !node.has_error() {
        return;
    }
    if node.is_missing() {
        let name = node.kind();
        out.push(make(doc, node, format!("Missing `{name}`")));
        return;
    }
    if node.is_error() {
        let found = first_token(doc, node);
        let message = match found {
            Some(tok) => format!("Unexpected `{tok}`"),
            None => "Syntax error".to_string(),
        };
        out.push(make(doc, node, message));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(doc, child, out);
    }
}

fn first_token(doc: &Document, node: Node) -> Option<String> {
    let mut n = node;
    while let Some(child) = n.child(0) {
        n = child;
    }
    let text = doc.node_text(n).trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(30).collect())
}

fn make(doc: &Document, node: Node, message: String) -> Diagnostic {
    // Long error regions are reduced to their first line so the whole
    // rest of the file is not underlined.
    let start = node.start_byte();
    let mut end = node.end_byte().max(start);
    if let Some(nl) = doc.text[start..end].find('\n') {
        end = start + nl;
    }
    if end == start {
        end = (start + 1).min(doc.text.len());
        while end < doc.text.len() && !doc.text.is_char_boundary(end) {
            end += 1;
        }
    }
    Diagnostic {
        range: doc.lines.range(&doc.text, start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some(SOURCE.into()),
        message,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(lang: Lang, src: &str) -> Vec<Diagnostic> {
        syntax_diagnostics(&Document::new(lang, src.into(), 0))
    }

    #[test]
    fn valid_files_have_none() {
        let src = "package org.example.demo\ninterface Foo {\n  method bar {\n  }\n}\n";
        assert!(diags(Lang::Fidl, src).is_empty());
        assert!(diags(Lang::Fdepl, "package org.example.demo\n").is_empty());
    }

    #[test]
    fn reports_unexpected_token_position() {
        let src = "package org.example.demo\ninterface Foo {\n  bogus here\n}\n";
        let d = diags(Lang::Fidl, src);
        assert!(!d.is_empty());
        assert_eq!(d[0].range.start.line, 2);
        assert!(d[0].message.contains("bogus"), "{}", d[0].message);
    }

    #[test]
    fn reports_missing_closing_brace() {
        let d = diags(Lang::Fidl, "package a\ninterface Foo {\n");
        assert!(d.iter().any(|d| d.message.starts_with("Missing")), "{d:?}");
    }

    #[test]
    fn reports_errors_in_fdepl() {
        let d = diags(Lang::Fdepl, "package a\ndefine x.Y for interface {\n}\n");
        assert!(!d.is_empty());
    }

    fn semantic(src: &str) -> Vec<Diagnostic> {
        let uri = Url::parse("file:///ws/a.fidl").unwrap();
        let mut ws = Workspace::default();
        ws.open(uri.clone(), "franca", src.into(), 1);
        diagnostics(&ws, &uri)
    }

    #[test]
    fn warns_about_unresolved_types() {
        let d = semantic("package a\ninterface I {\n attribute Nope x\n}\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "Cannot resolve `Nope`");
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(d[0].range.start.line, 2);
    }

    #[test]
    fn warns_about_missing_import_files() {
        let d = semantic("package a\nimport b.* from \"missing.fidl\"\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("missing.fidl"));
    }

    #[test]
    fn flags_duplicate_definitions() {
        let d = semantic("package a\ninterface I {\n attribute Int8 x\n method x { }\n}\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "Duplicate definition of `x`");
        assert_eq!(d[0].range.start.line, 3);
    }

    #[test]
    fn same_argument_names_in_different_methods_are_fine() {
        let src = "package a\ninterface I {\n method m { in { Int8 v } out { Int8 v } }\n method n { in { Int8 v } }\n}\n";
        assert!(semantic(src).is_empty());
    }

    #[test]
    fn resolved_types_are_quiet() {
        let src = "package a\ntypeCollection T { struct S { Int8 f } }\ninterface I { attribute T.S s attribute Integer(0, 5) n }\n";
        assert!(semantic(src).is_empty());
    }
}
