//! Completion for Franca IDL.
//!
//! The context is derived from a light-weight token scan up to the cursor
//! rather than from the syntax tree, since code is usually incomplete while
//! it is being typed.

use std::collections::BTreeSet;

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position, Url};

use crate::fidl::{DeclKind, FidlIndex, Ref, RefKind, Segment};
use crate::workspace::{DeclId, Workspace};

const TOP_KEYWORDS: &[&str] = &["package", "import", "typeCollection", "interface"];
const TYPE_DEF_KEYWORDS: &[&str] = &["array", "typedef", "struct", "union", "enumeration", "map"];
const BASIC_TYPES: &[&str] = &[
    "Int8", "UInt8", "Int16", "UInt16", "Int32", "UInt32", "Int64", "UInt64", "Boolean", "String",
    "Float", "Double", "ByteBuffer", "Integer", "undefined",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    Top,
    Interface,
    TypeCollection,
    Struct,
    Enumeration,
    Map,
    Method,
    Broadcast,
    In,
    Out,
    Contract,
    Vars,
    Psm,
    State,
    Other,
}

#[derive(Debug)]
struct Context {
    block: Block,
    /// Words (dotted names count as one) on the cursor's line before the cursor.
    units: Vec<String>,
    /// Dotted qualifier immediately before the word being typed, e.g. `a.b` in `a.b.Na|`.
    qualifier: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    Open,
    Close,
    Dot,
    Arrow,
    Other,
}

struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

/// Tokenizes `text[..end]`. Returns `None` if `end` is inside a comment,
/// annotation or string, where completion makes no sense.
fn lex(text: &str, end: usize) -> Option<Vec<Token>> {
    let src = &text[..end];
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if src[i..].starts_with("//") {
            match src[i..].find('\n') {
                Some(n) => i += n + 1,
                None => return None,
            }
        } else if src[i..].starts_with("/*") {
            i += src[i + 2..].find("*/")? + 4;
        } else if src[i..].starts_with("<**") {
            i += src[i + 3..].find("**>")? + 6;
        } else if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            loop {
                match b.get(j) {
                    None => return None,
                    Some(b'\\') => j += 2,
                    Some(&q) if q == c => break,
                    Some(_) => j += 1,
                }
            }
            i = j + 1;
        } else if c == b'{' {
            out.push(Token { tok: Tok::Open, start: i, end: i + 1 });
            i += 1;
        } else if c == b'}' {
            out.push(Token { tok: Tok::Close, start: i, end: i + 1 });
            i += 1;
        } else if c == b'.' {
            out.push(Token { tok: Tok::Dot, start: i, end: i + 1 });
            i += 1;
        } else if src[i..].starts_with("->") {
            out.push(Token { tok: Tok::Arrow, start: i, end: i + 2 });
            i += 2;
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'^' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            out.push(Token { tok: Tok::Word(src[i..j].to_string()), start: i, end: j });
            i = j;
        } else {
            let len = src[i..].chars().next().map_or(1, char::len_utf8);
            out.push(Token { tok: Tok::Other, start: i, end: i + len });
            i += len;
        }
    }
    Some(out)
}

fn block_for(words: &[String]) -> Block {
    for w in words.iter().rev() {
        let b = match w.as_str() {
            "interface" => Block::Interface,
            "typeCollection" => Block::TypeCollection,
            "struct" | "union" => Block::Struct,
            "enumeration" | "error" => Block::Enumeration,
            "map" => Block::Map,
            "method" => Block::Method,
            "broadcast" => Block::Broadcast,
            "in" => Block::In,
            "out" => Block::Out,
            "contract" => Block::Contract,
            "vars" => Block::Vars,
            "PSM" => Block::Psm,
            "state" => Block::State,
            _ => continue,
        };
        return Some(b).unwrap();
    }
    Block::Other
}

fn context(text: &str, offset: usize) -> Option<Context> {
    let mut tokens = lex(text, offset)?;
    // Drop the word being typed.
    if matches!(tokens.last(), Some(t) if matches!(t.tok, Tok::Word(_)) && t.end == offset) {
        tokens.pop();
    }

    let mut stack: Vec<Block> = Vec::new();
    let mut recent: Vec<String> = Vec::new();
    for t in &tokens {
        match &t.tok {
            Tok::Open => {
                stack.push(block_for(&recent));
                recent.clear();
            }
            Tok::Close => {
                stack.pop();
                recent.clear();
            }
            Tok::Word(w) => recent.push(w.clone()),
            _ => {}
        }
    }
    let block = stack.last().copied().unwrap_or(Block::Top);

    let line_start = text[..offset].rfind('\n').map_or(0, |n| n + 1);
    let mut units: Vec<String> = Vec::new();
    let mut joined = false;
    for t in tokens.iter().filter(|t| t.start >= line_start) {
        match &t.tok {
            Tok::Word(w) => {
                if joined {
                    units.last_mut().unwrap().push_str(&format!(".{w}"));
                    joined = false;
                } else {
                    units.push(w.clone());
                }
            }
            Tok::Dot => joined = true,
            Tok::Arrow => {
                joined = false;
                units.push("->".into());
            }
            Tok::Open | Tok::Close => {
                joined = false;
                units.clear();
            }
            Tok::Other => joined = false,
        }
    }

    // Qualifier: tokens `word . word .` directly before the partial word.
    let mut qualifier = None;
    if matches!(tokens.last(), Some(t) if t.tok == Tok::Dot && t.end == offset - partial_len(text, offset)) {
        let mut parts: Vec<&str> = Vec::new();
        let mut k = tokens.len() - 1;
        while k >= 1 && tokens[k].tok == Tok::Dot {
            if let Tok::Word(w) = &tokens[k - 1].tok {
                parts.push(w.as_str());
                if k >= 2 { k -= 2 } else { break }
            } else {
                break;
            }
        }
        if !parts.is_empty() {
            parts.reverse();
            qualifier = Some(parts.join("."));
            // The qualifier is part of the current unit; drop it.
            if units.last().is_some_and(|u| u.ends_with(&parts.join("."))) {
                units.pop();
            }
        }
    }
    Some(Context { block, units, qualifier })
}

fn partial_len(text: &str, offset: usize) -> usize {
    text[..offset].bytes().rev().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'^').count()
}

fn keyword_items(words: &[&str]) -> Vec<CompletionItem> {
    words
        .iter()
        .map(|w| CompletionItem {
            label: w.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

fn item_kind(kind: DeclKind) -> CompletionItemKind {
    match kind {
        DeclKind::Interface => CompletionItemKind::INTERFACE,
        DeclKind::TypeCollection => CompletionItemKind::MODULE,
        DeclKind::Struct | DeclKind::Union => CompletionItemKind::STRUCT,
        DeclKind::Enumeration => CompletionItemKind::ENUM,
        DeclKind::Enumerator => CompletionItemKind::ENUM_MEMBER,
        DeclKind::Method => CompletionItemKind::METHOD,
        DeclKind::Broadcast => CompletionItemKind::EVENT,
        DeclKind::Attribute | DeclKind::Field => CompletionItemKind::FIELD,
        DeclKind::Constant => CompletionItemKind::CONSTANT,
        DeclKind::Variable | DeclKind::Argument => CompletionItemKind::VARIABLE,
        DeclKind::State => CompletionItemKind::ENUM_MEMBER,
        DeclKind::Array | DeclKind::Typedef | DeclKind::Map => CompletionItemKind::CLASS,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// Types, plus containers so that `Types.` can be typed.
    Types,
    /// Types and interfaces (`extends`, `manages`).
    Bases,
    Any,
}

fn wants(want: Want, kind: DeclKind) -> bool {
    match want {
        Want::Types => kind.is_type() || matches!(kind, DeclKind::TypeCollection | DeclKind::Interface),
        Want::Bases => kind.is_type() || kind == DeclKind::Interface,
        Want::Any => true,
    }
}

fn type_context(ctx: &Context) -> Option<Want> {
    let last = ctx.units.last().map(String::as_str);
    match last {
        Some("extends" | "manages") => return Some(Want::Bases),
        Some("of" | "is" | "to" | "attribute" | "const") => return Some(Want::Types),
        Some("error") if matches!(ctx.block, Block::Method) => return Some(Want::Types),
        _ => {}
    }
    match ctx.block {
        Block::Struct | Block::In | Block::Out | Block::Vars | Block::Map if ctx.units.len() % 2 == 0 => {
            Some(Want::Types)
        }
        _ => None,
    }
}

pub fn complete(ws: &Workspace, uri: &Url, pos: Position) -> Vec<CompletionItem> {
    let (Some(doc), Some(idx)) = (ws.get(uri), ws.fidl.get(uri)) else { return Vec::new() };
    let offset = doc.lines.offset(&doc.text, pos);
    let Some(ctx) = context(&doc.text, offset) else { return Vec::new() };
    let scope = idx.container_at(offset.saturating_sub(1));

    if let Some(q) = &ctx.qualifier {
        let want = type_context(&ctx).unwrap_or(Want::Any);
        return qualified(ws, uri, idx, scope, q, want);
    }

    if let Some(want) = type_context(&ctx) {
        let mut items = if want == Want::Bases { Vec::new() } else { keyword_items(BASIC_TYPES) };
        items.extend(visible_decls(ws, uri, idx, scope, want));
        return items;
    }

    let last = ctx.units.last().map(String::as_str);
    match (ctx.block, last) {
        (Block::Top, None) => {
            let mut kw: Vec<&str> = TOP_KEYWORDS.to_vec();
            if idx.package.is_some() {
                kw.retain(|k| *k != "package");
            }
            keyword_items(&kw)
        }
        (Block::Interface, None) => keyword_items(&[
            "version", "attribute", "method", "broadcast", "contract", "const", "public", "array", "typedef",
            "struct", "union", "enumeration", "map",
        ]),
        (Block::TypeCollection, None) => {
            keyword_items(&["version", "const", "public", "array", "typedef", "struct", "union", "enumeration", "map"])
        }
        (Block::Interface | Block::TypeCollection, Some("public")) => keyword_items(TYPE_DEF_KEYWORDS),
        (Block::Method, None) => keyword_items(&["in", "out", "error"]),
        (Block::Broadcast, None) => keyword_items(&["out"]),
        (Block::Contract, None) => keyword_items(&["vars", "PSM"]),
        (Block::Psm, None) => keyword_items(&["initial", "state"]),
        (Block::State, None) => keyword_items(&["on"]),
        (Block::State, Some("on")) => keyword_items(&["call", "respond", "error", "signal", "set", "update"]),
        (Block::Psm, Some("initial")) | (Block::State, Some("->")) => {
            members(ws, uri, idx, scope, &[DeclKind::State])
        }
        (Block::State, Some("call" | "respond" | "error" | "signal")) => {
            members(ws, uri, idx, scope, &[DeclKind::Method, DeclKind::Broadcast])
        }
        (Block::State, Some("set" | "update")) => members(ws, uri, idx, scope, &[DeclKind::Attribute]),
        _ => Vec::new(),
    }
}

/// Members of the interface around the cursor, including inherited ones.
fn members(ws: &Workspace, uri: &Url, idx: &FidlIndex, scope: Option<usize>, kinds: &[DeclKind]) -> Vec<CompletionItem> {
    let Some(scope) = scope else { return Vec::new() };
    let mut items = Vec::new();
    for ns in ws.scope_chain(uri, idx, scope) {
        for (name, id) in children_of(ws, &ns) {
            if let Some(d) = ws.decl(&id) {
                if kinds.contains(&d.kind) && d.name == name {
                    items.push(decl_item(&name, d.kind, Some(d.detail.clone()), &name));
                }
            }
        }
    }
    items
}

fn decl_item(label: &str, kind: DeclKind, detail: Option<String>, insert: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(item_kind(kind)),
        detail,
        insert_text: Some(insert.to_string()),
        ..Default::default()
    }
}

/// Direct children of the namespace `ns`: declarations and sub-namespaces.
fn children_of(ws: &Workspace, ns: &str) -> Vec<(String, DeclId)> {
    let prefix = format!("{ns}.");
    let mut out = Vec::new();
    for (fqn, ids) in &ws.by_fqn {
        let Some(rest) = fqn.strip_prefix(&prefix) else { continue };
        if rest.contains('.') {
            continue;
        }
        if let Some(id) = ids.first() {
            out.push((rest.to_string(), id.clone()));
        }
    }
    out
}

fn qualified(
    ws: &Workspace,
    uri: &Url,
    idx: &FidlIndex,
    scope: Option<usize>,
    qualifier: &str,
    want: Want,
) -> Vec<CompletionItem> {
    let segments: Vec<Segment> =
        qualifier.split('.').map(|s| Segment { text: s.to_string(), span: 0..0 }).collect();
    let r = Ref { segments, kind: RefKind::Value, scope };
    let mut namespaces: BTreeSet<String> = ws.candidates(uri, idx, &r, qualifier).into_iter().collect();
    // Sub-packages such as `org.` -> `example`.
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for ns in std::mem::take(&mut namespaces) {
        for (name, id) in children_of(ws, &ns) {
            let Some(d) = ws.decl(&id) else { continue };
            if d.name == name && wants(want, d.kind) && seen.insert(name.clone()) {
                items.push(decl_item(&name, d.kind, Some(d.detail.clone()), &name));
            }
        }
        let prefix = format!("{ns}.");
        for fqn in ws.by_fqn.keys() {
            if let Some(rest) = fqn.strip_prefix(&prefix) {
                let head = rest.split('.').next().unwrap_or(rest);
                if rest.contains('.') && !ws.by_fqn.contains_key(&format!("{prefix}{head}")) && seen.insert(head.to_string()) {
                    items.push(CompletionItem {
                        label: head.to_string(),
                        kind: Some(CompletionItemKind::MODULE),
                        detail: Some("package".into()),
                        ..Default::default()
                    });
                }
            }
        }
    }
    items
}

/// Declarations usable by their short name (or the shortest name that resolves).
fn visible_decls(ws: &Workspace, uri: &Url, idx: &FidlIndex, scope: Option<usize>, want: Want) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut ids: Vec<&DeclId> = ws.by_fqn.values().flatten().collect();
    ids.sort_by(|a, b| (a.uri.as_str(), a.idx).cmp(&(b.uri.as_str(), b.idx)));
    for id in ids {
        let Some(index) = ws.fidl.get(&id.uri) else { continue };
        let d = &index.decls[id.idx];
        if d.name.is_empty() || !wants(want, d.kind) {
            continue;
        }
        // Types nested deeper than one container are reached through it.
        let mut names = vec![d.name.clone()];
        if let Some(p) = d.parent.map(|p| &index.decls[p]) {
            if !p.name.is_empty() {
                names.push(format!("{}.{}", p.name, d.name));
            }
        }
        names.push(d.fqn.clone());
        let best = names.into_iter().find(|n| {
            let segments = n.split('.').map(|s| Segment { text: s.to_string(), span: 0..0 }).collect();
            let r = Ref { segments, kind: RefKind::Value, scope };
            ws.resolve_in(uri, idx, &r).is_some_and(|res| &res.target == id)
        });
        let Some(best) = best else { continue };
        let mut item = decl_item(&best, d.kind, Some(d.fqn.clone()), &best);
        item.sort_text = Some(format!("{}{}", best.matches('.').count(), best));
        items.push(item);
        if items.len() >= 2000 {
            break;
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMON: &str = "package org.common\ntypeCollection Types {\n struct Point { Int8 x }\n enumeration Mode { A }\n}\n";

    /// Completes at the `|` marker in `src`.
    fn complete_at(src: &str) -> Vec<String> {
        let offset = src.find('|').unwrap();
        let text = src.replacen('|', "", 1);
        let uri = Url::parse("file:///ws/main.fidl").unwrap();
        let mut ws = Workspace::default();
        ws.open(Url::parse("file:///ws/common.fidl").unwrap(), "franca", COMMON.into(), 1);
        ws.open(uri.clone(), "franca", text.clone(), 1);
        let doc = ws.get(&uri).unwrap();
        let pos = doc.lines.position(&doc.text, offset);
        let mut labels: Vec<String> = complete(&ws, &uri, pos).into_iter().map(|i| i.label).collect();
        labels.sort();
        labels
    }

    fn has(labels: &[String], l: &str) -> bool {
        labels.iter().any(|x| x == l)
    }

    #[test]
    fn top_level_keywords() {
        let l = complete_at("package a\n|");
        assert!(has(&l, "interface") && has(&l, "import") && has(&l, "typeCollection"));
        assert!(!has(&l, "package"));
        assert!(has(&complete_at("|"), "package"));
    }

    #[test]
    fn interface_member_keywords() {
        let l = complete_at("package a\ninterface I {\n  |\n}\n");
        assert!(has(&l, "attribute") && has(&l, "method") && has(&l, "broadcast") && has(&l, "struct"));
        assert!(!has(&l, "interface"));
    }

    #[test]
    fn method_sections() {
        let l = complete_at("package a\ninterface I {\n method m {\n  |\n }\n}\n");
        assert_eq!(l, ["error", "in", "out"]);
    }

    #[test]
    fn types_after_attribute() {
        let l = complete_at("package a\nimport org.common.* from \"common.fidl\"\ninterface I {\n attribute |\n}\n");
        assert!(has(&l, "UInt8") && has(&l, "Types") && has(&l, "Types.Point"));
    }

    #[test]
    fn types_in_argument_lists_only_at_type_position() {
        let at_type = complete_at("package a\ninterface I {\n method m { in { | } }\n}\n");
        assert!(has(&at_type, "Int8"));
        let at_name = complete_at("package a\ninterface I {\n method m { in { Int8 | } }\n}\n");
        assert!(at_name.is_empty());
        let second = complete_at("package a\ninterface I {\n method m { in { Int8 a | } }\n}\n");
        assert!(has(&second, "String"));
    }

    #[test]
    fn qualified_members() {
        let l = complete_at(
            "package a\nimport org.common.* from \"common.fidl\"\ninterface I {\n attribute Types.|\n}\n",
        );
        assert_eq!(l, ["Mode", "Point"]);
        let l = complete_at("package a\ninterface I {\n attribute org.|\n}\n");
        assert_eq!(l, ["common"]);
    }

    #[test]
    fn unqualified_names_use_the_shortest_resolvable_form() {
        let l = complete_at("package org.common\ninterface I {\n attribute |\n}\n");
        assert!(has(&l, "Types.Point"));
        let l = complete_at("package a\ninterface I {\n attribute |\n}\n");
        assert!(has(&l, "org.common.Types.Point"));
    }

    #[test]
    fn contract_completions() {
        let src = "package a\ninterface I {\n method m { }\n attribute Int8 x\n contract {\n  PSM {\n   initial s\n   state s {\n    on |\n   }\n  }\n }\n}\n";
        assert_eq!(complete_at(src), ["call", "error", "respond", "set", "signal", "update"]);
        let l = complete_at(&src.replace("on |", "on call |"));
        assert_eq!(l, ["m"]);
        let l = complete_at(&src.replace("on |", "on set |"));
        assert_eq!(l, ["x"]);
        let l = complete_at(&src.replace("on |", "on call m -> |"));
        assert_eq!(l, ["s"]);
    }

    #[test]
    fn extends_offers_interfaces() {
        let src = "package a\ninterface Base { }\ninterface Child extends | {\n}\n";
        assert!(has(&complete_at(src), "Base"));
    }

    #[test]
    fn silent_in_comments_and_strings() {
        assert!(complete_at("package a\n// inter|\n").is_empty());
        assert!(complete_at("package a\n<** @description: x |").is_empty());
        assert!(complete_at("package a\nimport b.* from \"x|\"").is_empty());
    }
}
