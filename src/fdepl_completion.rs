//! Completion for Franca deployment files.

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position, Url};

use crate::completion::{Tok, keyword_items, lex, partial_len, snippet_items};
use crate::fdepl::FdeplKind;
use crate::fdepl_resolve::host_matches;
use crate::fidl::DeclKind;
use crate::workspace::{DeclId, Workspace};

const HOSTS: &[&str] = &[
    "interface",
    "typeCollections",
    "methods",
    "attributes",
    "broadcasts",
    "arguments",
    "arrays",
    "structs",
    "unions",
    "enumerations",
    "enumerators",
    "typedefs",
    "maps",
    "fields",
    "provider",
    "instances",
];
const MEMBER_KEYWORDS: &[&str] =
    &["method", "attribute", "broadcast", "array", "struct", "union", "typedef", "enumeration", "map"];
const TYPE_KEYWORDS: &[&str] = &["array", "struct", "union", "typedef", "enumeration", "map"];

#[derive(Clone, Default)]
struct Frame {
    /// Keyword of the element: `interface`, `method`, `argument`, ...
    kw: String,
    /// The Franca declaration this block deploys.
    target: Option<DeclId>,
    /// The specification named by the enclosing `define`.
    spec: Option<String>,
    in_specification: bool,
    is_host: bool,
}

fn kind_for(keyword: &str) -> Option<DeclKind> {
    Some(match keyword {
        "method" => DeclKind::Method,
        "attribute" => DeclKind::Attribute,
        "broadcast" => DeclKind::Broadcast,
        "array" => DeclKind::Array,
        "struct" => DeclKind::Struct,
        "union" => DeclKind::Union,
        "typedef" => DeclKind::Typedef,
        "enumeration" => DeclKind::Enumeration,
        "map" => DeclKind::Map,
        _ => return None,
    })
}

fn open_frame(ws: &Workspace, uri: &Url, units: &[String], parent: Option<&Frame>) -> Frame {
    let inherit = |kw: &str, target: Option<DeclId>| Frame {
        kw: kw.to_string(),
        target,
        spec: parent.and_then(|p| p.spec.clone()),
        ..Default::default()
    };
    if let Some(p) = units.iter().rposition(|u| u == "define") {
        let spec = units.get(p + 1).cloned();
        let f = units[p..].iter().position(|u| u == "for").map(|f| f + p);
        let kw = f.and_then(|f| units.get(f + 1)).cloned().unwrap_or_default();
        let target = match (kw.as_str(), f.and_then(|f| units.get(f + 2))) {
            ("interface" | "typeCollection", Some(name)) => ws.fdepl_toplevel(uri, name),
            _ => None,
        };
        return Frame { kw, target, spec, ..Default::default() };
    }
    if units.last().is_some_and(|u| u != "for") && units.iter().any(|u| u == "specification") {
        return Frame { kw: "specification".into(), in_specification: true, ..Default::default() };
    }
    if parent.is_some_and(|p| p.in_specification) {
        let host = units.last().cloned().unwrap_or_default();
        return Frame { kw: host, is_host: true, ..Default::default() };
    }
    let parent_target = parent.and_then(|p| p.target.clone());
    let member = |name: Option<&String>| match (&parent_target, name) {
        (Some(t), Some(n)) => ws.fidl_member(t, n),
        _ => None,
    };
    if let Some(p) = units.iter().rposition(|u| MEMBER_KEYWORDS.contains(&u.as_str())) {
        return inherit(&units[p], member(units.get(p + 1)));
    }
    if let Some(p) = units.iter().rposition(|u| u == "in" || u == "out") {
        return inherit(&units[p], parent_target);
    }
    let parent_kw = parent.map(|p| p.kw.as_str()).unwrap_or_default();
    match parent_kw {
        "in" | "out" => inherit("argument", member(units.last())),
        "struct" | "union" => inherit("field", member(units.last())),
        "enumeration" => inherit("enumerator", member(units.last())),
        "interface" | "typeCollection" | "" => inherit("", None),
        _ => inherit(units.first().map(String::as_str).unwrap_or_default(), None),
    }
}

fn item(label: &str, kind: CompletionItemKind, detail: Option<String>) -> CompletionItem {
    CompletionItem { label: label.to_string(), kind: Some(kind), detail, ..Default::default() }
}

fn children(ws: &Workspace, parent: &DeclId, kinds: &[DeclKind]) -> Vec<CompletionItem> {
    let Some(idx) = ws.fidl.get(&parent.uri) else { return Vec::new() };
    idx.children(parent.idx)
        .filter(|&i| kinds.contains(&idx.decls[i].kind) && !idx.decls[i].name.is_empty())
        .map(|i| {
            let d = &idx.decls[i];
            item(&d.name, CompletionItemKind::FIELD, Some(d.detail.clone()))
        })
        .collect()
}

/// Arguments of the method in its `in` or `out` group.
fn arguments(ws: &Workspace, method: &DeclId, group: &str) -> Vec<CompletionItem> {
    let (Some(doc), Some(idx)) = (ws.get(&method.uri), ws.fidl.get(&method.uri)) else { return Vec::new() };
    let Some(decl) = idx.decls.get(method.idx) else { return Vec::new() };
    let Some(node) = doc.root().descendant_for_byte_range(decl.span.start, decl.span.end) else { return Vec::new() };
    let mut node = node;
    while node.byte_range() != decl.span {
        match node.parent() {
            Some(p) => node = p,
            None => return Vec::new(),
        }
    }
    let Some(range) = node.child_by_field_name(group).map(|g| g.byte_range()) else { return Vec::new() };
    idx.children(method.idx)
        .filter(|&i| {
            let d = &idx.decls[i];
            d.kind == DeclKind::Argument && range.start <= d.span.start && d.span.end <= range.end
        })
        .map(|i| item(&idx.decls[i].name, CompletionItemKind::FIELD, Some(idx.decls[i].detail.clone())))
        .collect()
}

fn spec_names(ws: &Workspace) -> Vec<CompletionItem> {
    let mut names: Vec<&str> = ws
        .fdepl
        .values()
        .flat_map(|i| i.decls.iter())
        .filter(|d| d.kind == FdeplKind::Specification && !d.name.is_empty())
        .map(|d| d.name.as_str())
        .collect();
    names.sort();
    names.dedup();
    names.into_iter().map(|n| item(n, CompletionItemKind::INTERFACE, None)).collect()
}

fn fidl_toplevels(ws: &Workspace, want: DeclKind) -> Vec<CompletionItem> {
    let mut out: Vec<(String, String)> = Vec::new();
    for idx in ws.fidl.values() {
        for d in &idx.decls {
            if d.kind != want {
                continue;
            }
            match (d.name.is_empty(), &idx.package) {
                (false, _) => out.push((d.fqn.clone(), d.detail.clone())),
                (true, Some(pkg)) => out.push((pkg.clone(), "anonymous typeCollection".into())),
                _ => {}
            }
        }
    }
    out.sort();
    out.dedup();
    out.into_iter().map(|(n, d)| item(&n, CompletionItemKind::MODULE, Some(d))).collect()
}

/// Properties of the specification chain that apply to `frame`.
fn property_decls(ws: &Workspace, uri: &Url, frame: &Frame) -> Vec<(String, String)> {
    let Some((su, si)) = frame.spec.as_deref().and_then(|s| ws.find_spec(uri, s)) else { return Vec::new() };
    ws.spec_properties(&su, si)
        .into_iter()
        .filter_map(|(u, i)| {
            let idx = &ws.fdepl[&u];
            let d = &idx.decls[i];
            let host = &idx.decls[d.parent?];
            host_matches(&host.name, &frame.kw).then(|| (d.name.clone(), d.detail.clone()))
        })
        .collect()
}

/// Values suggested for a property by its declared type.
fn value_items(detail: &str) -> Vec<CompletionItem> {
    let ty = detail.split_once(':').map_or("", |(_, t)| t.trim());
    if let Some(inner) = ty.strip_prefix('{').and_then(|t| t.split('}').next()) {
        return inner
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| item(v, CompletionItemKind::ENUM_MEMBER, None))
            .collect();
    }
    if ty.starts_with("Boolean") {
        return ["true", "false"].iter().map(|v| item(v, CompletionItemKind::ENUM_MEMBER, None)).collect();
    }
    Vec::new()
}

pub fn complete(ws: &Workspace, uri: &Url, pos: Position) -> Vec<CompletionItem> {
    let Some(doc) = ws.get(uri) else { return Vec::new() };
    let offset = doc.lines.offset(&doc.text, pos);
    let Some(mut tokens) = lex(&doc.text, offset) else { return Vec::new() };
    let partial = partial_len(&doc.text, offset);
    if matches!(tokens.last(), Some(t) if matches!(t.tok, Tok::Word(_)) && t.end == offset) {
        tokens.pop();
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut recent: Vec<String> = Vec::new();
    let mut joined = false;
    for t in &tokens {
        match &t.tok {
            Tok::Open => {
                let frame = open_frame(ws, uri, &recent, stack.last());
                stack.push(frame);
                recent.clear();
            }
            Tok::Close => {
                stack.pop();
                recent.clear();
            }
            Tok::Word(w) if joined => {
                if let Some(last) = recent.last_mut() {
                    last.push_str(&format!(".{w}"));
                }
                joined = false;
            }
            Tok::Word(w) => recent.push(w.clone()),
            Tok::Dot => joined = true,
            _ => joined = false,
        }
    }
    // A dotted name being typed is not completed (resolution is by full name).
    if matches!(tokens.last(), Some(t) if t.tok == Tok::Dot && t.end == offset - partial) {
        return Vec::new();
    }

    let line_start = doc.text[..offset].rfind('\n').map_or(0, |n| n + 1);
    let mut units: Vec<String> = Vec::new();
    let mut joined = false;
    let mut after_eq = false;
    let mut colon = false;
    for t in tokens.iter().filter(|t| t.start >= line_start) {
        match &t.tok {
            Tok::Word(w) if joined => {
                if let Some(u) = units.last_mut() {
                    u.push_str(&format!(".{w}"));
                }
                joined = false;
            }
            Tok::Word(w) => units.push(w.clone()),
            Tok::Dot => joined = true,
            Tok::Open | Tok::Close => {
                joined = false;
                units.clear();
                after_eq = false;
                colon = false;
            }
            _ => {
                joined = false;
                match &doc.text[t.start..t.end] {
                    "=" => after_eq = true,
                    ":" => colon = true,
                    _ => {}
                }
            }
        }
    }
    let last = units.last().map(String::as_str);

    let Some(frame) = stack.last() else {
        let has_define = units.contains(&"define".to_string());
        return match last {
            None => {
                let mut kw = vec!["package", "import", "define", "specification"];
                if ws.fdepl.get(uri).is_some_and(|i| i.package.is_some()) {
                    kw.retain(|k| *k != "package");
                }
                snippet_items(&kw)
            }
            Some("define" | "extends") => spec_names(ws),
            Some("for") if has_define => keyword_items(&["interface", "typeCollection"]),
            Some("interface") if has_define => fidl_toplevels(ws, DeclKind::Interface),
            Some("typeCollection") if has_define => fidl_toplevels(ws, DeclKind::TypeCollection),
            _ => Vec::new(),
        };
    };

    if frame.in_specification {
        return match last {
            None => keyword_items(&["for"]),
            Some("for") => keyword_items(HOSTS),
            _ => Vec::new(),
        };
    }
    if frame.is_host {
        if colon && units.len() == 1 {
            return keyword_items(&["Integer", "String", "Boolean", "Interface"]);
        }
        return Vec::new();
    }

    let props = property_decls(ws, uri, frame);
    if after_eq {
        let name = units.iter().rev().find(|u| props.iter().any(|(n, _)| n == *u));
        return name
            .and_then(|n| props.iter().find(|(p, _)| p == n))
            .map(|(_, detail)| value_items(detail))
            .unwrap_or_default();
    }

    match last {
        Some("use") => return fidl_toplevels(ws, DeclKind::TypeCollection),
        Some(kw) if units.len() == 1 && MEMBER_KEYWORDS.contains(&kw) => {
            return match (&frame.target, kind_for(kw)) {
                (Some(t), Some(k)) => children(ws, t, &[k]),
                _ => Vec::new(),
            };
        }
        None => {}
        _ => return Vec::new(),
    }

    let mut out: Vec<CompletionItem> =
        props.iter().map(|(n, d)| item(n, CompletionItemKind::PROPERTY, Some(d.clone()))).collect();
    let target = frame.target.as_ref();
    match frame.kw.as_str() {
        "interface" => {
            out.extend(keyword_items(&["use", "method", "attribute", "broadcast"]));
            out.extend(keyword_items(TYPE_KEYWORDS));
        }
        "typeCollection" => {
            out.extend(keyword_items(&["use"]));
            out.extend(keyword_items(TYPE_KEYWORDS));
        }
        "method" => out.extend(keyword_items(&["in", "out"])),
        "broadcast" => out.extend(keyword_items(&["out"])),
        "map" => out.extend(keyword_items(&["key", "value"])),
        group @ ("in" | "out") => out.extend(target.map(|t| arguments(ws, t, group)).unwrap_or_default()),
        "struct" | "union" => out.extend(target.map(|t| children(ws, t, &[DeclKind::Field])).unwrap_or_default()),
        "enumeration" => out.extend(target.map(|t| children(ws, t, &[DeclKind::Enumerator])).unwrap_or_default()),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIDL: &str = "package p\ninterface I {\n method move { in { Int8 to } out { Int8 ok } }\n attribute Int8 origin\n struct Pt { Int8 x }\n}\n";
    const SPEC: &str = "package s\nspecification s.Base { for interface { Port: Integer; Mode: { fast, slow }; Flag: Boolean; } }\nspecification s.Spec extends s.Base { for methods { Timeout: Integer; } }\n";

    /// Completes at the `|` marker in `deploy`.
    fn complete_at(deploy: &str) -> Vec<String> {
        let offset = deploy.find('|').unwrap();
        let text = deploy.replace('|', "");
        let before = &text[..offset];
        let line = before.matches('\n').count() as u32;
        let col = before.len() - before.rfind('\n').map_or(0, |n| n + 1);
        let mut ws = Workspace::default();
        for (n, l, t) in [("a.fidl", "franca", FIDL.to_string()), ("s.fdepl", "fdepl", SPEC.to_string())] {
            ws.open(Url::parse(&format!("file:///ws/{n}")).unwrap(), l, t, 1);
        }
        let u = Url::parse("file:///ws/d.fdepl").unwrap();
        ws.open(u.clone(), "fdepl", text, 1);
        let mut labels: Vec<String> =
            complete(&ws, &u, Position::new(line, col as u32)).into_iter().map(|i| i.label).collect();
        labels.sort();
        labels
    }

    fn has(labels: &[String], want: &[&str]) -> bool {
        want.iter().all(|w| labels.iter().any(|l| l == w))
    }

    #[test]
    fn top_level_keywords_and_specs() {
        let l = complete_at("package d\n|");
        assert!(has(&l, &["import", "define", "specification"]) && !l.contains(&"package".to_string()));
        let l = complete_at("package d\ndefine |");
        assert_eq!(l, ["s.Base", "s.Spec"]);
        assert_eq!(complete_at("package d\ndefine s.Spec for |"), ["interface", "typeCollection"]);
        assert_eq!(complete_at("package d\ndefine s.Spec for interface |"), ["p.I"]);
    }

    #[test]
    fn specification_blocks() {
        assert_eq!(complete_at("package d\nspecification x.S {\n |\n}"), ["for"]);
        assert!(has(&complete_at("package d\nspecification x.S {\n for |\n}"), &["interface", "methods"]));
        assert_eq!(complete_at("package d\nspecification x.S {\n for interface {\n  P: |\n }\n}").len(), 4);
    }

    #[test]
    fn deployment_body_offers_inherited_properties_and_members() {
        let l = complete_at("package d\ndefine s.Spec for interface p.I {\n |\n}");
        assert!(has(&l, &["Port", "Mode", "Flag", "method", "attribute", "use", "struct"]), "{l:?}");
        assert!(!l.contains(&"Timeout".to_string()));
        let l = complete_at("package d\ndefine s.Spec for interface p.I {\n method move {\n  |\n }\n}");
        assert!(has(&l, &["Timeout", "in", "out"]), "{l:?}");
    }

    #[test]
    fn targets_come_from_the_deployed_declaration() {
        let src = "package d\ndefine s.Spec for interface p.I {\n";
        assert_eq!(complete_at(&format!("{src} method |\n}}")), ["move"]);
        assert_eq!(complete_at(&format!("{src} attribute |\n}}")), ["origin"]);
        assert_eq!(complete_at(&format!("{src} struct |\n}}")), ["Pt"]);
        assert_eq!(complete_at(&format!("{src} method move {{\n  in {{\n   |\n  }}\n }}\n}}")), ["to"]);
        assert_eq!(complete_at(&format!("{src} method move {{\n  out {{\n   |\n  }}\n }}\n}}")), ["ok"]);
        assert_eq!(complete_at(&format!("{src} struct Pt {{\n  |\n }}\n}}")), ["x"]);
    }

    #[test]
    fn values_follow_the_declared_type() {
        let src = "package d\ndefine s.Spec for interface p.I {\n";
        assert_eq!(complete_at(&format!("{src} Mode = |\n}}")), ["fast", "slow"]);
        assert_eq!(complete_at(&format!("{src} Flag = |\n}}")), ["false", "true"]);
        assert!(complete_at(&format!("{src} Port = |\n}}")).is_empty());
    }

    #[test]
    fn nothing_inside_comments() {
        assert!(complete_at("package d\n// define |").is_empty());
    }
}
