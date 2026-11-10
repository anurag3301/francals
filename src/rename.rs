use std::collections::HashMap;

use tower_lsp::lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};

use crate::navigation::{references, target_at};
use crate::workspace::Workspace;

pub fn is_valid_identifier(name: &str) -> bool {
    let body = name.strip_prefix('^').unwrap_or(name);
    let mut chars = body.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The range and current name of the symbol that would be renamed.
pub fn prepare(ws: &Workspace, uri: &Url, pos: Position) -> Option<(Range, String)> {
    let doc = ws.get(uri)?;
    let target = target_at(ws, uri, pos)?;
    let decl = ws.decl(&target.decl)?;
    if decl.name.is_empty() {
        return None;
    }
    let range = doc.lines.range(&doc.text, target.span.start, target.span.end);
    Some((range, doc.text[target.span.clone()].to_string()))
}

pub fn rename(ws: &Workspace, uri: &Url, pos: Position, new_name: &str) -> Result<Option<WorkspaceEdit>, String> {
    if !is_valid_identifier(new_name) {
        return Err(format!("`{new_name}` is not a valid identifier"));
    }
    let Some(target) = target_at(ws, uri, pos) else { return Ok(None) };
    let Some(decl) = ws.decl(&target.decl) else { return Ok(None) };
    if decl.name.is_empty() {
        return Ok(None);
    }
    let bare = new_name.trim_start_matches('^');
    let index = &ws.fidl[&target.decl.uri];
    let clash = index.decls.iter().enumerate().any(|(i, d)| {
        i != target.decl.idx && d.parent == decl.parent && d.name == bare && d.kind != crate::fidl::DeclKind::Argument
    });
    if clash {
        return Err(format!("`{bare}` is already defined in this scope"));
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for loc in references(ws, &target.decl, true) {
        changes.entry(loc.uri).or_default().push(TextEdit { range: loc.range, new_text: new_name.to_string() });
    }
    Ok(Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(name: &str) -> Url {
        Url::parse(&format!("file:///ws/{name}")).unwrap()
    }

    fn setup() -> Workspace {
        let mut ws = Workspace::default();
        ws.open(uri("c.fidl"), "franca", "package c\ntypeCollection T {\n struct Point { Int8 x }\n enumeration E { A }\n}\n".into(), 1);
        ws.open(
            uri("m.fidl"),
            "franca",
            "package m\nimport c.* from \"c.fidl\"\ninterface I {\n attribute T.Point a\n attribute T.Point b\n}\n".into(),
            1,
        );
        ws
    }

    #[test]
    fn validates_identifiers() {
        assert!(is_valid_identifier("Foo_1"));
        assert!(is_valid_identifier("^interface"));
        assert!(!is_valid_identifier("1abc"));
        assert!(!is_valid_identifier("a.b"));
        assert!(!is_valid_identifier(""));
    }

    #[test]
    fn prepares_on_references_and_declarations() {
        let ws = setup();
        let (range, name) = prepare(&ws, &uri("m.fidl"), Position::new(3, 14)).unwrap();
        assert_eq!(name, "Point");
        assert_eq!(range.start, Position::new(3, 13));
        assert!(prepare(&ws, &uri("m.fidl"), Position::new(0, 0)).is_none());
    }

    #[test]
    fn renames_declaration_and_all_references() {
        let ws = setup();
        let edit = rename(&ws, &uri("c.fidl"), Position::new(2, 9), "Vec2").unwrap().unwrap();
        let changes = edit.changes.unwrap();
        assert_eq!(changes[&uri("c.fidl")].len(), 1);
        assert_eq!(changes[&uri("m.fidl")].len(), 2);
        assert!(changes.values().flatten().all(|e| e.new_text == "Vec2"));
        // Qualifier `T.` stays untouched: only the last segment is replaced.
        assert_eq!(changes[&uri("m.fidl")][0].range.start, Position::new(3, 13));
    }

    #[test]
    fn rejects_bad_names_and_clashes() {
        let ws = setup();
        assert!(rename(&ws, &uri("c.fidl"), Position::new(2, 9), "1x").is_err());
        let err = rename(&ws, &uri("c.fidl"), Position::new(2, 9), "E").unwrap_err();
        assert!(err.contains("already defined"));
    }
}
