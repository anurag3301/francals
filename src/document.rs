use tower_lsp::lsp_types::TextDocumentContentChangeEvent;
use tree_sitter::{InputEdit, Node, Tree};

use crate::language::{self, Lang};
use crate::text::LineIndex;

/// An open or indexed source file together with its syntax tree.
pub struct Document {
    pub lang: Lang,
    pub text: String,
    pub version: i32,
    pub tree: Tree,
    pub lines: LineIndex,
}

impl Document {
    pub fn new(lang: Lang, text: String, version: i32) -> Self {
        let tree = language::parse(lang, &text, None);
        let lines = LineIndex::new(&text);
        Document { lang, text, version, tree, lines }
    }

    /// Applies LSP content changes (full or incremental) and reparses,
    /// reusing the previous tree for incremental edits.
    pub fn apply_changes(&mut self, changes: Vec<TextDocumentContentChangeEvent>, version: i32) {
        for change in changes {
            match change.range {
                Some(range) => {
                    let start = self.lines.offset(&self.text, range.start);
                    let end = self.lines.offset(&self.text, range.end).max(start);
                    let edit = InputEdit {
                        start_byte: start,
                        old_end_byte: end,
                        new_end_byte: start + change.text.len(),
                        start_position: self.lines.point(start),
                        old_end_position: self.lines.point(end),
                        // Fixed up below once the new text is known.
                        new_end_position: self.lines.point(start),
                    };
                    self.text.replace_range(start..end, &change.text);
                    let new_lines = LineIndex::new(&self.text);
                    let edit = InputEdit {
                        new_end_position: new_lines.point(edit.new_end_byte),
                        ..edit
                    };
                    self.tree.edit(&edit);
                    self.lines = new_lines;
                    self.tree = language::parse(self.lang, &self.text, Some(&self.tree));
                }
                None => {
                    self.text = change.text;
                    self.lines = LineIndex::new(&self.text);
                    self.tree = language::parse(self.lang, &self.text, None);
                }
            }
        }
        self.version = version;
    }

    pub fn root(&self) -> Node<'_> {
        self.tree.root_node()
    }

    pub fn node_text(&self, node: Node) -> &str {
        &self.text[node.byte_range()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position, Range};

    fn change(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(sl, sc), Position::new(el, ec))),
            range_length: None,
            text: text.into(),
        }
    }

    #[test]
    fn incremental_edit_matches_full_parse() {
        let mut doc = Document::new(Lang::Fidl, "package a.b\ninterface X {\n}\n".into(), 1);
        doc.apply_changes(vec![change(1, 10, 1, 11, "Renamed")], 2);
        assert_eq!(doc.text, "package a.b\ninterface Renamed {\n}\n");
        let fresh = language::parse(Lang::Fidl, &doc.text, None);
        assert_eq!(doc.root().to_sexp(), fresh.root_node().to_sexp());
        assert_eq!(doc.version, 2);
    }

    #[test]
    fn full_replacement() {
        let mut doc = Document::new(Lang::Fidl, "package a\n".into(), 1);
        let full = TextDocumentContentChangeEvent { range: None, range_length: None, text: "package b\n".into() };
        doc.apply_changes(vec![full], 5);
        assert_eq!(doc.text, "package b\n");
    }

    #[test]
    fn multiple_changes_apply_in_order() {
        let mut doc = Document::new(Lang::Fidl, "package a\n".into(), 1);
        doc.apply_changes(vec![change(0, 8, 0, 9, "bc"), change(0, 8, 0, 10, "x.y")], 2);
        assert_eq!(doc.text, "package x.y\n");
        assert!(!doc.root().has_error());
    }
}
