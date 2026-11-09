use std::collections::{HashMap, HashSet};

use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

use crate::document::Document;
use crate::language::Lang;

/// All documents known to the server, whether open in the editor or
/// loaded from disk.
#[derive(Default)]
pub struct Workspace {
    pub docs: HashMap<Url, Document>,
    open: HashSet<Url>,
}

pub fn lang_for(uri: &Url, language_id: Option<&str>) -> Option<Lang> {
    Lang::from_path(uri.path()).or_else(|| language_id.and_then(Lang::from_language_id))
}

impl Workspace {
    pub fn open(&mut self, uri: Url, language_id: &str, text: String, version: i32) -> bool {
        let Some(lang) = lang_for(&uri, Some(language_id)) else {
            return false;
        };
        self.open.insert(uri.clone());
        self.docs.insert(uri, Document::new(lang, text, version));
        true
    }

    pub fn change(&mut self, uri: &Url, changes: Vec<TextDocumentContentChangeEvent>, version: i32) {
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.apply_changes(changes, version);
        }
    }

    pub fn close(&mut self, uri: &Url) {
        self.open.remove(uri);
    }

    pub fn is_open(&self, uri: &Url) -> bool {
        self.open.contains(uri)
    }

    pub fn get(&self, uri: &Url) -> Option<&Document> {
        self.docs.get(uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn open_change_close() {
        let mut ws = Workspace::default();
        let u = uri("file:///tmp/a.fidl");
        assert!(ws.open(u.clone(), "franca", "package a\n".into(), 1));
        assert!(ws.is_open(&u));
        let full = TextDocumentContentChangeEvent { range: None, range_length: None, text: "package b\n".into() };
        ws.change(&u, vec![full], 2);
        assert_eq!(ws.get(&u).unwrap().text, "package b\n");
        ws.close(&u);
        assert!(!ws.is_open(&u));
        assert!(ws.get(&u).is_some());
    }

    #[test]
    fn rejects_unknown_files() {
        let mut ws = Workspace::default();
        assert!(!ws.open(uri("file:///tmp/a.txt"), "plaintext", String::new(), 1));
    }
}
