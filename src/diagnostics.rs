use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};
use tree_sitter::Node;

use crate::document::Document;

pub const SOURCE: &str = "francals";

/// Syntax errors reported by tree-sitter, as LSP diagnostics.
pub fn syntax_diagnostics(doc: &Document) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    collect(doc, doc.root(), &mut out);
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
    use crate::language::Lang;

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
}
