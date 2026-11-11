//! Symbol index for Franca IDL (`.fidl`) files.

use std::ops::Range;

use tree_sitter::Node;

use crate::document::Document;

pub type Span = Range<usize>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeclKind {
    TypeCollection,
    Interface,
    Attribute,
    Method,
    Broadcast,
    Constant,
    Array,
    Typedef,
    Struct,
    Union,
    Enumeration,
    Map,
    Field,
    Enumerator,
    Argument,
    Variable,
    State,
}

impl DeclKind {
    /// Declarations that can be named where a type is expected.
    pub fn is_type(self) -> bool {
        matches!(
            self,
            DeclKind::Array
                | DeclKind::Typedef
                | DeclKind::Struct
                | DeclKind::Union
                | DeclKind::Enumeration
                | DeclKind::Map
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            DeclKind::TypeCollection => "typeCollection",
            DeclKind::Interface => "interface",
            DeclKind::Attribute => "attribute",
            DeclKind::Method => "method",
            DeclKind::Broadcast => "broadcast",
            DeclKind::Constant => "constant",
            DeclKind::Array => "array",
            DeclKind::Typedef => "typedef",
            DeclKind::Struct => "struct",
            DeclKind::Union => "union",
            DeclKind::Enumeration => "enumeration",
            DeclKind::Map => "map",
            DeclKind::Field => "field",
            DeclKind::Enumerator => "enumerator",
            DeclKind::Argument => "argument",
            DeclKind::Variable => "variable",
            DeclKind::State => "state",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Decl {
    pub name: String,
    pub kind: DeclKind,
    /// Fully qualified name. Members of an anonymous type collection live
    /// directly in the package namespace.
    pub fqn: String,
    pub parent: Option<usize>,
    pub name_span: Span,
    pub span: Span,
    pub detail: String,
    pub doc: Option<String>,
    /// Index into `FidlIndex::refs` of the `extends` clause, if any.
    pub base: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Import {
    /// `org.example.*` or `org.example.Name`; `None` for `import model`.
    pub namespace: Option<String>,
    pub uri: String,
    pub uri_span: Span,
    pub span: Span,
}

impl Import {
    pub fn wildcard(&self) -> Option<&str> {
        self.namespace.as_deref().and_then(|n| n.strip_suffix(".*"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// A type name in a declaration.
    Type,
    /// The target of `extends` or `manages`.
    Base,
    /// A name inside an expression or a contract.
    Value,
    /// A member of the enclosing interface (method, attribute, state...).
    Member,
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Ref {
    pub segments: Vec<Segment>,
    pub kind: RefKind,
    /// Enclosing interface or type collection.
    pub scope: Option<usize>,
}

impl Ref {
    pub fn span(&self) -> Span {
        self.segments[0].span.start..self.segments.last().unwrap().span.end
    }

    pub fn name(&self, segments: usize) -> String {
        let parts: Vec<&str> = self.segments[..segments].iter().map(|s| s.text.as_str()).collect();
        parts.join(".")
    }
}

#[derive(Clone, Debug, Default)]
pub struct FidlIndex {
    pub package: Option<String>,
    pub package_span: Option<Span>,
    pub imports: Vec<Import>,
    pub decls: Vec<Decl>,
    pub refs: Vec<Ref>,
}

pub fn identifier_name(text: &str) -> &str {
    text.strip_prefix('^').unwrap_or(text)
}

impl FidlIndex {
    pub fn build(doc: &Document) -> FidlIndex {
        let mut b = Builder { doc, index: FidlIndex::default() };
        b.visit(doc.root(), None, None);
        b.index
    }

    pub fn decl_at(&self, offset: usize) -> Option<usize> {
        self.decls.iter().position(|d| d.name_span.start <= offset && offset <= d.name_span.end)
    }

    pub fn ref_at(&self, offset: usize) -> Option<usize> {
        self.refs.iter().position(|r| {
            let s = r.span();
            s.start <= offset && offset <= s.end
        })
    }

    /// Index of the innermost interface or type collection around `offset`.
    pub fn container_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, d) in self.decls.iter().enumerate() {
            if matches!(d.kind, DeclKind::Interface | DeclKind::TypeCollection)
                && d.span.start <= offset
                && offset <= d.span.end
            {
                best = Some(i);
            }
        }
        best
    }

    pub fn children(&self, parent: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.decls.len()).filter(move |&i| self.decls[i].parent == Some(parent))
    }
}

struct Builder<'a> {
    doc: &'a Document,
    index: FidlIndex,
}

impl<'a> Builder<'a> {
    fn text(&self, node: Node) -> &'a str {
        &self.doc.text[node.byte_range()]
    }

    fn visit(&mut self, node: Node, parent: Option<usize>, scope: Option<usize>) {
        let kind = node.kind();
        match kind {
            "package_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.index.package = Some(self.qualified(name));
                    self.index.package_span = Some(name.byte_range());
                }
                return;
            }
            "import_declaration" => {
                self.import(node);
                return;
            }
            "type_reference" => {
                self.add_ref(node, RefKind::Type, scope);
                return;
            }
            "element_reference" => {
                if let Some(q) = node.named_child(0) {
                    self.add_ref(q, RefKind::Value, scope);
                }
                return;
            }
            "selector_reference" => {
                if let Some(n) = node.child_by_field_name("name") {
                    self.add_identifier_ref(n, RefKind::Member, scope);
                }
                return;
            }
            "trigger" => {
                if let Some(n) = node.child_by_field_name("attribute") {
                    self.add_identifier_ref(n, RefKind::Member, scope);
                }
                self.visit_children(node, parent, scope);
                return;
            }
            "state_graph" => {
                if let Some(n) = node.child_by_field_name("initial") {
                    self.add_identifier_ref(n, RefKind::Member, scope);
                }
            }
            "transition" => {
                if let Some(n) = node.child_by_field_name("target") {
                    self.add_identifier_ref(n, RefKind::Member, scope);
                }
            }
            "error_clause" => {
                if let Some(q) = node.child_by_field_name("enumeration") {
                    self.add_ref(q, RefKind::Type, scope);
                }
            }
            _ => {}
        }

        let decl_kind = match kind {
            "type_collection" => Some(DeclKind::TypeCollection),
            "interface_declaration" => Some(DeclKind::Interface),
            "attribute_declaration" => Some(DeclKind::Attribute),
            "method_declaration" => Some(DeclKind::Method),
            "broadcast_declaration" => Some(DeclKind::Broadcast),
            "constant_declaration" => Some(DeclKind::Constant),
            "array_declaration" => Some(DeclKind::Array),
            "typedef_declaration" => Some(DeclKind::Typedef),
            "struct_declaration" => Some(DeclKind::Struct),
            "union_declaration" => Some(DeclKind::Union),
            "enumeration_declaration" => Some(DeclKind::Enumeration),
            "map_declaration" => Some(DeclKind::Map),
            "field" => Some(DeclKind::Field),
            "enumerator" => Some(DeclKind::Enumerator),
            "argument" => Some(DeclKind::Argument),
            "variable_declaration" => Some(DeclKind::Variable),
            "state" => Some(DeclKind::State),
            _ => None,
        };
        let Some(decl_kind) = decl_kind else {
            self.visit_children(node, parent, scope);
            return;
        };

        // `extends` and `manages` are looked up from the enclosing scope,
        // not from inside the declaration they belong to.
        let base_scope = if decl_kind == DeclKind::Interface { None } else { scope };
        let mut base = None;
        if let Some(b) = node.child_by_field_name("base") {
            base = Some(self.index.refs.len());
            self.add_ref(b, RefKind::Base, base_scope);
        }
        let mut cursor = node.walk();
        let managed: Vec<Node> = node.children_by_field_name("managed", &mut cursor).collect();
        for m in managed {
            self.add_ref(m, RefKind::Base, base_scope);
        }

        let id = self.push_decl(node, decl_kind, parent, base);
        let child_scope = match decl_kind {
            DeclKind::Interface | DeclKind::TypeCollection => Some(id),
            _ => scope,
        };
        self.visit_children(node, Some(id), child_scope);
    }

    fn visit_children(&mut self, node: Node, parent: Option<usize>, scope: Option<usize>) {
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children {
            self.visit(child, parent, scope);
        }
    }

    fn qualified(&self, node: Node) -> String {
        let mut cursor = node.walk();
        let parts: Vec<&str> = node
            .children(&mut cursor)
            .filter(|c| c.kind() == "identifier")
            .map(|c| identifier_name(self.text(c)))
            .collect();
        parts.join(".")
    }

    fn import(&mut self, node: Node) {
        let Some(uri) = node.child_by_field_name("uri") else { return };
        let namespace = node.child_by_field_name("namespace").map(|n| {
            let mut s: String = self.text(n).split_whitespace().collect();
            if let Some(stripped) = s.strip_suffix(".*") {
                s = format!("{stripped}.*");
            }
            s
        });
        let raw = self.text(uri);
        let uri_span = uri.start_byte() + 1..uri.end_byte().saturating_sub(1).max(uri.start_byte() + 1);
        self.index.imports.push(Import { namespace, uri: unquote(raw), uri_span, span: node.byte_range() });
    }

    fn add_ref(&mut self, node: Node, kind: RefKind, scope: Option<usize>) {
        let mut cursor = node.walk();
        let segments: Vec<Segment> = node
            .children(&mut cursor)
            .filter(|c| c.kind() == "identifier")
            .map(|c| Segment { text: identifier_name(self.text(c)).to_string(), span: c.byte_range() })
            .collect();
        if segments.is_empty() {
            return;
        }
        self.index.refs.push(Ref { segments, kind, scope });
    }

    fn add_identifier_ref(&mut self, node: Node, kind: RefKind, scope: Option<usize>) {
        let segment = Segment { text: identifier_name(self.text(node)).to_string(), span: node.byte_range() };
        self.index.refs.push(Ref { segments: vec![segment], kind, scope });
    }

    fn push_decl(&mut self, node: Node, kind: DeclKind, parent: Option<usize>, base: Option<usize>) -> usize {
        let name_node = node.child_by_field_name("name");
        let (name, name_span) = match name_node {
            Some(n) => (identifier_name(self.text(n)).to_string(), n.byte_range()),
            None => {
                // Anonymous type collections have no name; point at the keyword.
                let mut cursor = node.walk();
                let kw = node.children(&mut cursor).find(|c| c.kind() == "typeCollection");
                (String::new(), kw.map(|k| k.byte_range()).unwrap_or(node.byte_range()))
            }
        };
        let prefix = match parent {
            Some(p) => Some(self.index.decls[p].fqn.clone()),
            None => self.index.package.clone(),
        };
        let fqn = match (&prefix, name.is_empty()) {
            (Some(p), false) => format!("{p}.{name}"),
            (Some(p), true) => p.clone(),
            (None, _) => name.clone(),
        };
        let detail = self.signature(node, kind);
        let doc = self.description(node);
        self.index.decls.push(Decl { name, kind, fqn, parent, name_span, span: node.byte_range(), detail, doc, base });
        self.index.decls.len() - 1
    }

    fn annotation_block<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        node.children(&mut cursor).find(|c| c.kind() == "annotation_block")
    }

    fn description(&self, node: Node) -> Option<String> {
        let block = self.annotation_block(node)?;
        let mut cursor = block.walk();
        for ann in block.children(&mut cursor).filter(|c| c.kind() == "annotation") {
            let tag = ann.child_by_field_name("tag").map(|t| self.text(t)).unwrap_or("");
            if tag.trim_start_matches('@').trim_end_matches(':').trim() != "description" {
                continue;
            }
            let text = ann.child_by_field_name("text").map(|t| self.text(t)).unwrap_or("");
            let lines: Vec<&str> = text.lines().map(|l| l.trim().trim_start_matches('*').trim()).collect();
            let joined = lines.join("\n").trim().to_string();
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        None
    }

    /// One-line human readable declaration, used for hover.
    fn signature(&self, node: Node, kind: DeclKind) -> String {
        let start = self.annotation_block(node).map(|b| b.end_byte()).unwrap_or(node.start_byte());
        let full = &self.doc.text[start..node.end_byte()];
        match kind {
            DeclKind::Method | DeclKind::Broadcast => self.operation_signature(node, kind),
            DeclKind::Interface
            | DeclKind::TypeCollection
            | DeclKind::Struct
            | DeclKind::Union
            | DeclKind::Enumeration
            | DeclKind::Map => {
                let head = full.split('{').next().unwrap_or(full);
                collapse(head)
            }
            _ => {
                let s = collapse(full);
                if s.chars().count() > 160 { s.chars().take(160).collect::<String>() + "…" } else { s }
            }
        }
    }

    fn operation_signature(&self, node: Node, kind: DeclKind) -> String {
        let mut out = String::from(kind.label());
        if let Some(n) = node.child_by_field_name("name") {
            out.push(' ');
            out.push_str(self.text(n));
        }
        if let Some(s) = node.child_by_field_name("selector") {
            out.push_str(&format!(":{}", self.text(s)));
        }
        let group = |field: &str, label: &str| -> Option<String> {
            let g = node.child_by_field_name(field)?;
            let mut cursor = g.walk();
            let args: Vec<String> = g
                .children(&mut cursor)
                .filter(|c| c.kind() == "argument")
                .map(|a| {
                    let ty = a.child_by_field_name("type").map(|t| collapse(self.text(t))).unwrap_or_default();
                    let suffix = if a.children(&mut a.walk()).any(|c| c.kind() == "array_suffix") { "[]" } else { "" };
                    let name = a.child_by_field_name("name").map(|n| self.text(n)).unwrap_or("");
                    format!("{ty}{suffix} {name}")
                })
                .collect();
            Some(format!("{label}: {}", args.join(", ")))
        };
        let mut parts: Vec<String> = Vec::new();
        parts.extend(group("in", "in"));
        parts.extend(group("out", "out"));
        let mut cursor = node.walk();
        if let Some(err) = node.children(&mut cursor).find(|c| c.kind() == "error_clause") {
            match err.child_by_field_name("enumeration") {
                Some(e) => parts.push(format!("error: {}", collapse(self.text(e)))),
                None => parts.push("error: { … }".to_string()),
            }
        }
        if !parts.is_empty() {
            out.push_str(&format!(" ({})", parts.join("; ")));
        }
        out
    }
}

pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn unquote(raw: &str) -> String {
    let b = raw.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Lang;

    pub const SAMPLE: &str = r#"package org.example.demo

import org.example.common.* from "common.fidl"
import model "other.fidl"

typeCollection Types {
    version { major 1 minor 0 }
    struct Point {
        Int32 x
        Int32 y
    }
    enumeration Color { RED GREEN = 2 }
    typedef Alias is Point
    const Int32 LIMIT = 10
}

<**
    @description: A demo interface.
**>
interface Demo extends Base {
    attribute Types.Point origin readonly
    method move {
        in { Types.Point to  UInt8[] steps }
        out { Boolean ok }
        error Types.Color
    }
    broadcast moved {
        out { Types.Point where }
    }
}
"#;

    fn index(src: &str) -> FidlIndex {
        FidlIndex::build(&Document::new(Lang::Fidl, src.into(), 0))
    }

    fn find<'a>(idx: &'a FidlIndex, fqn: &str) -> &'a Decl {
        idx.decls.iter().find(|d| d.fqn == fqn).unwrap_or_else(|| panic!("no decl {fqn}"))
    }

    #[test]
    fn collects_package_and_imports() {
        let idx = index(SAMPLE);
        assert_eq!(idx.package.as_deref(), Some("org.example.demo"));
        assert_eq!(idx.imports.len(), 2);
        assert_eq!(idx.imports[0].wildcard(), Some("org.example.common"));
        assert_eq!(idx.imports[0].uri, "common.fidl");
        assert_eq!(idx.imports[1].namespace, None);
        assert_eq!(idx.imports[1].uri, "other.fidl");
    }

    #[test]
    fn builds_qualified_names_and_hierarchy() {
        let idx = index(SAMPLE);
        let point = find(&idx, "org.example.demo.Types.Point");
        assert_eq!(point.kind, DeclKind::Struct);
        let x = find(&idx, "org.example.demo.Types.Point.x");
        assert_eq!(idx.decls[x.parent.unwrap()].name, "Point");
        assert_eq!(find(&idx, "org.example.demo.Types.Color.GREEN").kind, DeclKind::Enumerator);
        assert_eq!(find(&idx, "org.example.demo.Demo.move").kind, DeclKind::Method);
        assert_eq!(find(&idx, "org.example.demo.Demo.move.ok").kind, DeclKind::Argument);
    }

    #[test]
    fn anonymous_type_collection_shares_package_namespace() {
        let idx = index("package a.b\ntypeCollection {\n struct S { Int8 f }\n}\n");
        assert_eq!(find(&idx, "a.b.S").kind, DeclKind::Struct);
        let tc = idx.decls.iter().find(|d| d.kind == DeclKind::TypeCollection).unwrap();
        assert!(tc.name.is_empty());
    }

    #[test]
    fn records_references() {
        let idx = index(SAMPLE);
        let names: Vec<(String, RefKind)> = idx.refs.iter().map(|r| (r.name(r.segments.len()), r.kind)).collect();
        assert!(names.contains(&("Base".to_string(), RefKind::Base)));
        assert!(names.contains(&("Types.Point".to_string(), RefKind::Type)));
        assert!(names.contains(&("Types.Color".to_string(), RefKind::Type)));
        let base = idx.refs.iter().find(|r| r.kind == RefKind::Base).unwrap();
        assert_eq!(base.scope, None);
        let demo = idx.decls.iter().position(|d| d.name == "Demo").unwrap();
        assert!(idx.refs.iter().filter(|r| r.kind == RefKind::Type).any(|r| r.scope == Some(demo)));
    }

    #[test]
    fn builds_hover_details() {
        let idx = index(SAMPLE);
        assert_eq!(find(&idx, "org.example.demo.Demo").detail, "interface Demo extends Base");
        assert_eq!(
            find(&idx, "org.example.demo.Demo.move").detail,
            "method move (in: Types.Point to, UInt8[] steps; out: Boolean ok; error: Types.Color)"
        );
        assert_eq!(find(&idx, "org.example.demo.Types.LIMIT").detail, "const Int32 LIMIT = 10");
        assert_eq!(find(&idx, "org.example.demo.Demo").doc.as_deref(), Some("A demo interface."));
    }

    #[test]
    fn finds_contract_references() {
        let src = "package a\ninterface I {\n method m { }\n contract {\n  vars { Int8 v }\n  PSM {\n   initial s1\n   state s1 { on call m -> s2 }\n   state s2 { on set v -> s1 }\n  }\n }\n}\n";
        let idx = index(src);
        let mut members: Vec<String> =
            idx.refs.iter().filter(|r| r.kind == RefKind::Member).map(|r| r.name(1)).collect();
        members.sort();
        assert_eq!(members, vec!["m", "s1", "s1", "s2", "v"]);
        assert_eq!(find(&idx, "a.I.s1").kind, DeclKind::State);
        assert_eq!(find(&idx, "a.I.v").kind, DeclKind::Variable);
    }
}
