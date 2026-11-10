use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

use crate::document::Document;
use crate::fdepl::FdeplIndex;
use crate::fidl::FidlIndex;
use crate::language::Lang;
use crate::loader;

/// Identifies a declaration: the file it is in and its index in that file's index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeclId {
    pub uri: Url,
    pub idx: usize,
}

/// All documents known to the server, whether open in the editor or
/// loaded from disk.
#[derive(Default)]
pub struct Workspace {
    pub docs: HashMap<Url, Document>,
    pub fidl: HashMap<Url, FidlIndex>,
    pub fdepl: HashMap<Url, FdeplIndex>,
    pub roots: Vec<PathBuf>,
    /// Fully qualified name -> declarations with that name, across all files.
    pub by_fqn: HashMap<String, Vec<DeclId>>,
    fqn_keys: HashMap<Url, Vec<String>>,
    open: HashSet<Url>,
}

const MAX_IMPORTED_FILES: usize = 2000;

pub fn lang_for(uri: &Url, language_id: Option<&str>) -> Option<Lang> {
    Lang::from_path(uri.path()).or_else(|| language_id.and_then(Lang::from_language_id))
}

impl Workspace {
    pub fn open(&mut self, uri: Url, language_id: &str, text: String, version: i32) -> bool {
        let Some(lang) = lang_for(&uri, Some(language_id)) else {
            return false;
        };
        self.open.insert(uri.clone());
        self.insert(uri, Document::new(lang, text, version));
        true
    }

    /// Adds or replaces a document and refreshes its index.
    pub fn insert(&mut self, uri: Url, doc: Document) {
        self.docs.insert(uri.clone(), doc);
        self.reindex(&uri);
    }

    fn reindex(&mut self, uri: &Url) {
        let Some(doc) = self.docs.get(uri) else { return };
        match doc.lang {
            Lang::Fidl => {
                let idx = FidlIndex::build(doc);
                self.register_names(uri, &idx);
                self.fidl.insert(uri.clone(), idx);
            }
            Lang::Fdepl => {
                let idx = FdeplIndex::build(doc);
                self.fdepl.insert(uri.clone(), idx);
            }
        }
    }

    fn register_names(&mut self, uri: &Url, idx: &FidlIndex) {
        if let Some(keys) = self.fqn_keys.remove(uri) {
            for key in keys {
                if let Some(list) = self.by_fqn.get_mut(&key) {
                    list.retain(|d| &d.uri != uri);
                    if list.is_empty() {
                        self.by_fqn.remove(&key);
                    }
                }
            }
        }
        let mut keys = Vec::new();
        for (i, d) in idx.decls.iter().enumerate() {
            if d.name.is_empty() {
                continue;
            }
            self.by_fqn.entry(d.fqn.clone()).or_default().push(DeclId { uri: uri.clone(), idx: i });
            keys.push(d.fqn.clone());
        }
        self.fqn_keys.insert(uri.clone(), keys);
    }

    /// Loads a file from disk unless it is already known.
    pub fn load_file(&mut self, path: &std::path::Path) -> Option<Url> {
        let uri = loader::path_to_uri(path)?;
        if self.docs.contains_key(&uri) {
            return Some(uri);
        }
        let (uri, doc) = loader::read_document(path)?;
        self.insert(uri.clone(), doc);
        Some(uri)
    }

    /// Resolves the URI of an `import ... from "uri"` relative to the importing file.
    pub fn resolve_import(&self, from: &Url, target: &str) -> Option<Url> {
        if let Ok(url) = Url::parse(target) {
            if url.scheme() == "file" {
                return Some(url);
            }
        }
        let mut bases: Vec<PathBuf> = Vec::new();
        if let Ok(p) = from.to_file_path() {
            if let Some(dir) = p.parent() {
                bases.push(dir.to_path_buf());
            }
        }
        bases.extend(self.roots.iter().cloned());
        for base in &bases {
            if let Some(uri) = loader::path_to_uri(&base.join(target)) {
                if self.docs.contains_key(&uri) || uri.to_file_path().is_ok_and(|p| p.is_file()) {
                    return Some(uri);
                }
            }
        }
        // Fall back to a known file whose path ends with the import path.
        let suffix = format!("/{}", loader::normalize(std::path::Path::new(target)).to_string_lossy());
        self.docs.keys().find(|u| u.path().ends_with(&suffix)).cloned()
    }

    /// Loads files reachable through imports of `uri`.
    pub fn load_imports(&mut self, uri: &Url) {
        let mut queue = vec![uri.clone()];
        let mut seen = HashSet::new();
        let mut loaded = 0;
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let targets: Vec<String> = if let Some(idx) = self.fidl.get(&current) {
                idx.imports.iter().map(|i| i.uri.clone()).collect()
            } else if let Some(idx) = self.fdepl.get(&current) {
                idx.imports.iter().filter(|i| i.is_uri && !i.is_external()).map(|i| i.target.clone()).collect()
            } else {
                continue;
            };
            for t in targets {
                let Some(resolved) = self.resolve_import(&current, &t) else { continue };
                if !self.docs.contains_key(&resolved) {
                    let Ok(path) = resolved.to_file_path() else { continue };
                    if loaded >= MAX_IMPORTED_FILES || self.load_file(&path).is_none() {
                        continue;
                    }
                    loaded += 1;
                }
                queue.push(resolved);
            }
        }
    }

    pub fn change(&mut self, uri: &Url, changes: Vec<TextDocumentContentChangeEvent>, version: i32) {
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.apply_changes(changes, version);
            self.reindex(uri);
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
    fn follows_fdepl_imports() {
        let dir = temp_dir("fdepl-imports");
        std::fs::write(dir.join("a.fidl"), "package a\ninterface I { }\n").unwrap();
        std::fs::write(dir.join("spec.fdepl"), "package s\nspecification s.S { for interface { P: Integer; } }\n").unwrap();
        let main = dir.join("main.fdepl");
        std::fs::write(&main, "package m\nimport \"spec.fdepl\"\nimport \"a.fidl\"\n").unwrap();
        let mut ws = Workspace::default();
        let main_uri = ws.load_file(&main).unwrap();
        ws.load_imports(&main_uri);
        let found = ws.by_fqn.contains_key("a.I") && ws.fdepl.len() == 2;
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(found);
    }

    #[test]
    fn rejects_unknown_files() {
        let mut ws = Workspace::default();
        assert!(!ws.open(uri("file:///tmp/a.txt"), "plaintext", String::new(), 1));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("francals-ws-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn registers_qualified_names() {
        let mut ws = Workspace::default();
        let u = uri("file:///tmp/a.fidl");
        ws.open(u.clone(), "franca", "package a.b\ninterface I { method m { } }\n".into(), 1);
        assert!(ws.by_fqn.contains_key("a.b.I.m"));
        let full = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "package a.b\ninterface J { }\n".into(),
        };
        ws.change(&u, vec![full], 2);
        assert!(!ws.by_fqn.contains_key("a.b.I.m"));
        assert!(ws.by_fqn.contains_key("a.b.J"));
    }

    #[test]
    fn follows_imports_from_disk() {
        let dir = temp_dir("imports");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/types.fidl"), "package lib\nimport x.* from \"../more.fidl\"\ntypeCollection T { struct S { Int8 f } }\n").unwrap();
        std::fs::write(dir.join("more.fidl"), "package x\ntypeCollection M { }\n").unwrap();
        let main = dir.join("main.fidl");
        std::fs::write(&main, "package app\nimport lib.* from \"lib/types.fidl\"\n").unwrap();
        let mut ws = Workspace::default();
        let main_uri = ws.load_file(&main).unwrap();
        ws.load_imports(&main_uri);
        let found = ws.by_fqn.contains_key("lib.T.S") && ws.by_fqn.contains_key("x.M");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(found);
    }

    #[test]
    fn resolves_imports_against_workspace_roots() {
        let dir = temp_dir("roots");
        std::fs::create_dir_all(dir.join("shared")).unwrap();
        std::fs::write(dir.join("shared/t.fidl"), "package s\n").unwrap();
        let mut ws = Workspace::default();
        ws.roots.push(dir.clone());
        let from = Url::from_file_path(dir.join("deep/er/x.fidl")).unwrap();
        let resolved = ws.resolve_import(&from, "shared/t.fidl");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(resolved.unwrap().path().ends_with("shared/t.fidl"));
    }
}
