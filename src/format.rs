//! Whitespace formatter for Franca IDL and FDeploy.
//!
//! Lines are re-indented by brace depth, trailing whitespace is removed,
//! runs of blank lines are collapsed and blank lines just inside a block
//! are dropped. Text inside comments and annotations keeps its relative
//! layout; only the whole block is shifted.

use tower_lsp::lsp_types::{FormattingOptions, TextEdit};

use crate::document::Document;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Code,
    BlockComment,
    Annotation,
}

struct Indent {
    unit: String,
    tab_width: usize,
}

impl Indent {
    fn new(options: &FormattingOptions) -> Self {
        let width = options.tab_size.max(1) as usize;
        let unit = if options.insert_spaces { " ".repeat(width) } else { "\t".to_string() };
        Indent { unit, tab_width: width }
    }

    fn columns(&self, line: &str) -> usize {
        let mut cols = 0;
        for c in line.chars() {
            match c {
                ' ' => cols += 1,
                '\t' => cols += self.tab_width,
                _ => break,
            }
        }
        cols
    }
}

/// Formats `text` without looking at its syntax.
pub fn format_text(text: &str, options: &FormattingOptions) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let indent = Indent::new(options);
    let mut depth: usize = 0;
    let mut state = State::Code;
    let mut shift: isize = 0;
    let mut out: Vec<String> = Vec::new();

    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r').trim_end();
        if line.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let trimmed = line.trim_start();

        if state != State::Code {
            let cols = (indent.columns(line) as isize + shift).max(0) as usize;
            let level = cols / indent.tab_width;
            let rest = cols % indent.tab_width;
            out.push(format!("{}{}{}", indent.unit.repeat(level), " ".repeat(rest), trimmed));
            state = scan(trimmed, state, &mut depth, &mut 0);
            continue;
        }

        let mut leading_closers = 0;
        let before = depth;
        let after_state = scan(trimmed, state, &mut depth, &mut leading_closers);
        let level = before.saturating_sub(leading_closers);
        out.push(format!("{}{}", indent.unit.repeat(level), trimmed));
        if after_state != State::Code {
            shift = (level * indent.tab_width) as isize - indent.columns(line) as isize;
        }
        state = after_state;
    }

    // Collapse blank runs; drop blanks after `{` and before `}`; trim the ends.
    let mut result: Vec<String> = Vec::new();
    for line in out {
        if line.is_empty() {
            match result.last() {
                None => continue,
                Some(prev) if prev.is_empty() || prev.trim_end().ends_with('{') => continue,
                _ => {}
            }
        } else if line.trim_start().starts_with('}') && result.last().is_some_and(String::is_empty) {
            result.pop();
        }
        result.push(line);
    }
    while result.last().is_some_and(String::is_empty) {
        result.pop();
    }
    if result.is_empty() {
        return String::new();
    }
    result.push(String::new());
    result.join(eol)
}

/// Walks one line, updating `depth` and counting braces closing before any other token.
fn scan(line: &str, mut state: State, depth: &mut usize, leading_closers: &mut usize) -> State {
    let b = line.as_bytes();
    let mut i = 0;
    let mut only_closers = true;
    while i < b.len() {
        match state {
            State::BlockComment => {
                if line[i..].starts_with("*/") {
                    state = State::Code;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            State::Annotation => {
                if line[i..].starts_with("**>") {
                    state = State::Code;
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            State::Code => {}
        }
        let c = b[i];
        if line[i..].starts_with("//") {
            break;
        } else if line[i..].starts_with("/*") {
            state = State::BlockComment;
            only_closers = false;
            i += 2;
        } else if line[i..].starts_with("<**") {
            state = State::Annotation;
            only_closers = false;
            i += 3;
        } else if c == b'"' || c == b'\'' {
            only_closers = false;
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
        } else if c == b'{' {
            *depth += 1;
            only_closers = false;
            i += 1;
        } else if c == b'}' {
            *depth = depth.saturating_sub(1);
            if only_closers {
                *leading_closers += 1;
            }
            i += 1;
        } else {
            if !c.is_ascii_whitespace() {
                only_closers = false;
            }
            i += 1;
        }
    }
    state
}

/// A whole-document edit, or no edits when the text is already formatted
/// or has syntax errors.
pub fn format_document(doc: &Document, options: &FormattingOptions) -> Vec<TextEdit> {
    if doc.root().has_error() {
        return Vec::new();
    }
    let formatted = format_text(&doc.text, options);
    if formatted == doc.text {
        return Vec::new();
    }
    let end = doc.lines.position(&doc.text, doc.text.len());
    vec![TextEdit {
        range: tower_lsp::lsp_types::Range::new(tower_lsp::lsp_types::Position::new(0, 0), end),
        new_text: formatted,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Lang;

    fn opts() -> FormattingOptions {
        FormattingOptions { tab_size: 4, insert_spaces: true, ..Default::default() }
    }

    fn fmt(s: &str) -> String {
        format_text(s, &opts())
    }

    #[test]
    fn indents_by_brace_depth() {
        let src = "package a\ninterface I {\nmethod m {\nin {\nInt8 x\n}\n}\n}\n";
        let want = "package a\ninterface I {\n    method m {\n        in {\n            Int8 x\n        }\n    }\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn is_idempotent() {
        let src = "package a\n\n\ninterface I {  \n\n   attribute Int8 x   \n\n\n   method m { }\n\n}\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once);
        assert_eq!(once, "package a\n\ninterface I {\n    attribute Int8 x\n\n    method m { }\n}\n");
    }

    #[test]
    fn keeps_comment_layout_but_shifts_block() {
        let src = "package a\ninterface I {\n<**\n   @description: Hello\n      world\n**>\nattribute Int8 x\n}\n";
        let want = "package a\ninterface I {\n    <**\n       @description: Hello\n          world\n    **>\n    attribute Int8 x\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn braces_in_strings_and_comments_do_not_count() {
        let src = "package a\nimport x.* from \"a{b.fidl\"\n// {\ninterface I {\nattribute Int8 x // }\n}\n";
        let want = "package a\nimport x.* from \"a{b.fidl\"\n// {\ninterface I {\n    attribute Int8 x // }\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn supports_tabs_and_crlf() {
        let o = FormattingOptions { tab_size: 4, insert_spaces: false, ..Default::default() };
        assert_eq!(format_text("interface I {\r\nmethod m { }\r\n}\r\n", &o), "interface I {\r\n\tmethod m { }\r\n}\r\n");
    }

    #[test]
    fn skips_files_with_syntax_errors() {
        let doc = Document::new(Lang::Fidl, "package a\ninterface I {\n".into(), 0);
        assert!(format_document(&doc, &opts()).is_empty());
    }

    #[test]
    fn formats_fdepl() {
        let src = "package a\ndefine x.Y for interface a.I {\nSomeProp = 1\nmethod m {\nOther = true\n}\n}\n";
        let doc = Document::new(Lang::Fdepl, src.into(), 0);
        let edits = format_document(&doc, &opts());
        assert_eq!(
            edits[0].new_text,
            "package a\ndefine x.Y for interface a.I {\n    SomeProp = 1\n    method m {\n        Other = true\n    }\n}\n"
        );
    }
}
