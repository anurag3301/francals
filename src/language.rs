use tree_sitter::{Language, Parser, Tree};
use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_franca() -> *const ();
    fn tree_sitter_fdepl() -> *const ();
}

/// The two file kinds this server understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Fidl,
    Fdepl,
}

impl Lang {
    pub fn from_path(path: &str) -> Option<Lang> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".fidl") {
            Some(Lang::Fidl)
        } else if lower.ends_with(".fdepl") {
            Some(Lang::Fdepl)
        } else {
            None
        }
    }

    pub fn from_language_id(id: &str) -> Option<Lang> {
        match id {
            "franca" | "fidl" => Some(Lang::Fidl),
            "fdepl" => Some(Lang::Fdepl),
            _ => None,
        }
    }

    pub fn language(self) -> Language {
        let f = match self {
            Lang::Fidl => tree_sitter_franca,
            Lang::Fdepl => tree_sitter_fdepl,
        };
        unsafe { Language::from(LanguageFn::from_raw(f)) }
    }
}

pub fn parse(lang: Lang, source: &str, old: Option<&Tree>) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&lang.language())
        .expect("grammar ABI matches tree-sitter runtime");
    parser.parse(source, old).expect("parser has no timeout")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_language_from_path() {
        assert_eq!(Lang::from_path("/a/b/Foo.fidl"), Some(Lang::Fidl));
        assert_eq!(Lang::from_path("Foo.FDEPL"), Some(Lang::Fdepl));
        assert_eq!(Lang::from_path("foo.txt"), None);
    }

    #[test]
    fn parses_both_grammars() {
        let t = parse(Lang::Fidl, "package org.example.demo\n", None);
        assert!(!t.root_node().has_error());
        assert_eq!(t.root_node().kind(), "source_file");
        let t = parse(Lang::Fdepl, "package org.example.demo\n", None);
        assert!(!t.root_node().has_error());
    }

    #[test]
    fn reports_errors() {
        let t = parse(Lang::Fidl, "package\n", None);
        assert!(t.root_node().has_error());
    }
}
