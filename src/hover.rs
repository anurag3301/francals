use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::fidl::{Decl, DeclKind};
use crate::navigation::target_at;
use crate::workspace::Workspace;

fn builtin_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "Int8" => "8-bit signed integer",
        "UInt8" => "8-bit unsigned integer",
        "Int16" => "16-bit signed integer",
        "UInt16" => "16-bit unsigned integer",
        "Int32" => "32-bit signed integer",
        "UInt32" => "32-bit unsigned integer",
        "Int64" => "64-bit signed integer",
        "UInt64" => "64-bit unsigned integer",
        "Boolean" => "Boolean value (`true` or `false`)",
        "String" => "UTF-8 string",
        "Float" => "32-bit IEEE 754 floating point number",
        "Double" => "64-bit IEEE 754 floating point number",
        "ByteBuffer" => "Sequence of bytes",
        "Integer" => "Integer with optional interval, e.g. `Integer(0, 100)`",
        "undefined" => "Placeholder type used by `typedef` declarations without a target",
        _ => return None,
    })
}

fn markdown(value: String) -> HoverContents {
    HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value })
}

fn describe(decl: &Decl, owner: Option<&Decl>) -> String {
    let mut out = format!("```franca\n{}\n```", decl.detail);
    if let Some(doc) = &decl.doc {
        out.push_str("\n\n");
        out.push_str(doc);
    }
    let location = match owner {
        Some(o) if !o.name.is_empty() && decl.kind != DeclKind::Interface => {
            format!("{} in `{}`", decl.kind.label(), o.fqn)
        }
        _ => format!("{} `{}`", decl.kind.label(), decl.fqn),
    };
    out.push_str(&format!("\n\n*{location}*"));
    out
}

pub fn hover(ws: &Workspace, uri: &tower_lsp::lsp_types::Url, pos: Position) -> Option<Hover> {
    let doc = ws.get(uri)?;
    if let Some(target) = target_at(ws, uri, pos) {
        let decl = ws.decl(&target.decl)?;
        let index = ws.fidl.get(&target.decl.uri)?;
        let owner = decl.parent.map(|p| &index.decls[p]);
        return Some(Hover {
            contents: markdown(describe(decl, owner)),
            range: Some(doc.lines.range(&doc.text, target.span.start, target.span.end)),
        });
    }
    let offset = doc.lines.offset(&doc.text, pos);
    let node = doc.root().descendant_for_byte_range(offset, offset)?;
    let mut n = node;
    loop {
        if matches!(n.kind(), "basic_type" | "integer_interval") {
            let text = doc.node_text(n);
            let head = text.split('(').next().unwrap_or(text).trim();
            let info = builtin_doc(head)?;
            return Some(Hover {
                contents: markdown(format!("```franca\n{head}\n```\n\n{info}")),
                range: Some(doc.lines.range(&doc.text, n.start_byte(), n.end_byte())),
            });
        }
        n = n.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Url;

    fn text(h: &Hover) -> &str {
        match &h.contents {
            HoverContents::Markup(m) => &m.value,
            _ => panic!("unexpected hover contents"),
        }
    }

    fn setup() -> (Workspace, Url) {
        let uri = Url::parse("file:///ws/a.fidl").unwrap();
        let mut ws = Workspace::default();
        let src = "package org.demo\ntypeCollection T {\n <**\n @description: Point in 2D.\n **>\n struct P { Int8 x }\n}\ninterface I {\n attribute T.P p\n method m { in { UInt8 v } }\n}\n";
        ws.open(uri.clone(), "franca", src.into(), 1);
        (ws, uri)
    }

    #[test]
    fn describes_referenced_declaration() {
        let (ws, uri) = setup();
        let h = hover(&ws, &uri, Position::new(8, 13)).unwrap();
        assert!(text(&h).contains("struct P"), "{}", text(&h));
        assert!(text(&h).contains("Point in 2D."));
        assert!(text(&h).contains("in `org.demo.T`"));
    }

    #[test]
    fn describes_declarations_with_owner() {
        let (ws, uri) = setup();
        let h = hover(&ws, &uri, Position::new(9, 8)).unwrap();
        assert!(text(&h).contains("method m (in: UInt8 v)"), "{}", text(&h));
        assert!(text(&h).contains("org.demo.I"));
    }

    #[test]
    fn explains_builtin_types() {
        let (ws, uri) = setup();
        let h = hover(&ws, &uri, Position::new(9, 20)).unwrap();
        assert!(text(&h).contains("8-bit unsigned"), "{}", text(&h));
    }

    #[test]
    fn nothing_on_keywords() {
        let (ws, uri) = setup();
        assert!(hover(&ws, &uri, Position::new(0, 2)).is_none());
    }
}
