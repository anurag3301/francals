//! Symbol index for Franca deployment (`.fdepl`) files.

use tree_sitter::Node;

use crate::document::Document;
use crate::fidl::{Segment, Span, collapse, identifier_name, unquote};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdeplKind {
    Specification,
    /// A `for <host> { ... }` section of a specification.
    Host,
    Property,
    /// `define <spec> for interface|typeCollection|<extension> ...`
    Deployment,
    /// Deployment of a member or type below a `define`.
    Member,
}

impl FdeplKind {
    pub fn label(self) -> &'static str {
        match self {
            FdeplKind::Specification => "specification",
            FdeplKind::Host => "host",
            FdeplKind::Property => "property",
            FdeplKind::Deployment => "deployment",
            FdeplKind::Member => "member",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FdeplDecl {
    pub name: String,
    pub kind: FdeplKind,
    /// Keyword of the element (`interface`, `method`, `struct`, ...) or host name.
    pub keyword: String,
    pub parent: Option<usize>,
    pub name_span: Span,
    pub span: Span,
    pub detail: String,
    /// For specifications: the `extends` reference.
    pub base: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct FdeplImport {
    pub target: String,
    /// `true` for `import "uri"`, `false` for `import qualified.name`.
    pub is_uri: bool,
    pub span: Span,
    pub target_span: Span,
}

impl FdeplImport {
    /// Platform and classpath URIs are resolved by Eclipse, not by us.
    pub fn is_external(&self) -> bool {
        !self.is_uri || (self.target.contains(':') && !self.target.starts_with("file:"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdeplRefKind {
    /// The specification named in `define <spec>` or `extends`.
    Spec,
    /// The Franca element a deployment is for.
    Target,
    /// `&name` inside a property value.
    InterfaceValue,
    /// A property assignment (`Name = value`).
    Property,
}

#[derive(Clone, Debug)]
pub struct FdeplRef {
    pub segments: Vec<Segment>,
    pub kind: FdeplRefKind,
    /// For targets: the enclosing target, whose name this one is relative to.
    pub parent: Option<usize>,
    /// For properties: the specification reference of the enclosing `define`.
    pub spec: Option<usize>,
    /// For properties: keyword of the element the property is set on.
    pub host: String,
    pub is_top_level: bool,
}

impl FdeplRef {
    pub fn span(&self) -> Span {
        self.segments[0].span.start..self.segments.last().unwrap().span.end
    }

    pub fn name(&self, segments: usize) -> String {
        let parts: Vec<&str> = self.segments[..segments].iter().map(|s| s.text.as_str()).collect();
        parts.join(".")
    }

    pub fn full_name(&self) -> String {
        self.name(self.segments.len())
    }
}

#[derive(Clone, Debug, Default)]
pub struct FdeplIndex {
    pub package: Option<String>,
    pub imports: Vec<FdeplImport>,
    pub decls: Vec<FdeplDecl>,
    pub refs: Vec<FdeplRef>,
}

impl FdeplIndex {
    pub fn build(doc: &Document) -> FdeplIndex {
        let mut b = Builder { doc, index: FdeplIndex::default() };
        b.visit(doc.root(), &Ctx::default());
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
}

#[derive(Clone, Default)]
struct Ctx {
    parent_decl: Option<usize>,
    parent_target: Option<usize>,
    spec: Option<usize>,
    host: String,
    overwrite: bool,
}

struct Builder<'a> {
    doc: &'a Document,
    index: FdeplIndex,
}

impl<'a> Builder<'a> {
    fn text(&self, node: Node) -> &'a str {
        &self.doc.text[node.byte_range()]
    }

    fn visit(&mut self, node: Node, ctx: &Ctx) {
        match node.kind() {
            "package_declaration" => {
                if let Some(n) = node.child_by_field_name("name") {
                    self.index.package = Some(self.qualified_text(n));
                }
            }
            "import_declaration" => self.import(node),
            "specification" => self.specification(node, ctx),
            "specification_host" => {
                let host = node.child_by_field_name("host");
                let name = host.map(|h| self.text(h).to_string()).unwrap_or_default();
                let id = self.push_decl(node, host, name.clone(), FdeplKind::Host, name, ctx.parent_decl, None);
                let ctx2 = Ctx { parent_decl: Some(id), ..ctx.clone() };
                self.visit_children(node, &ctx2);
            }
            "property_declaration" => {
                let name = node.child_by_field_name("name");
                let text = collapse(self.text(node).trim_end_matches(';'));
                let n = name.map(|n| self.text(n).to_string()).unwrap_or_default();
                self.push_decl(node, name, n, FdeplKind::Property, String::new(), ctx.parent_decl, Some(text));
            }
            "type_collection_deployment" | "interface_deployment" | "extension_root" => {
                self.root_deployment(node, ctx)
            }
            "attribute_deployment" | "method_deployment" | "broadcast_deployment" | "array_deployment"
            | "struct_deployment" | "union_deployment" | "typedef_deployment" | "enumeration_deployment"
            | "map_deployment" | "field_deployment" | "argument_deployment" | "enumerator_deployment" => {
                self.member(node, ctx)
            }
            "extension_element" => {
                let tag = node.child_by_field_name("tag");
                let name = tag.map(|t| self.text(t).to_string()).unwrap_or_default();
                let id = self.push_decl(node, tag, name.clone(), FdeplKind::Member, name.clone(), ctx.parent_decl, None);
                let ctx2 = Ctx { parent_decl: Some(id), host: name, ..ctx.clone() };
                self.visit_children(node, &ctx2);
            }
            "type_overwrites" => {
                let ctx2 = Ctx { overwrite: true, ..ctx.clone() };
                self.visit_children(node, &ctx2);
            }
            "map_key" | "map_value" => {
                let ctx2 = Ctx { host: node.kind().trim_start_matches("map_").to_string(), ..ctx.clone() };
                self.visit_children(node, &ctx2);
            }
            "property" => {
                if let Some(n) = node.child_by_field_name("name") {
                    self.index.refs.push(FdeplRef {
                        segments: vec![segment(self.text(n), n.byte_range())],
                        kind: FdeplRefKind::Property,
                        parent: None,
                        spec: ctx.spec,
                        host: ctx.host.clone(),
                        is_top_level: false,
                    });
                }
                self.visit_children(node, ctx);
            }
            "interface_reference" => {
                let mut cursor = node.walk();
                let q = node.children(&mut cursor).find(|c| c.kind() == "qualified_name");
                if let Some(q) = q {
                    self.add_ref(q, FdeplRefKind::InterfaceValue, None, ctx, true);
                }
            }
            _ => self.visit_children(node, ctx),
        }
    }

    fn visit_children(&mut self, node: Node, ctx: &Ctx) {
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for c in children {
            self.visit(c, ctx);
        }
    }

    fn qualified_text(&self, node: Node) -> String {
        let mut cursor = node.walk();
        let parts: Vec<&str> = node
            .children(&mut cursor)
            .filter(|c| c.kind() == "identifier")
            .map(|c| identifier_name(self.text(c)))
            .collect();
        parts.join(".")
    }

    fn import(&mut self, node: Node) {
        if let Some(uri) = node.child_by_field_name("uri") {
            self.index.imports.push(FdeplImport {
                target: unquote(self.text(uri)),
                is_uri: true,
                span: node.byte_range(),
                target_span: uri.start_byte() + 1..uri.end_byte().saturating_sub(1).max(uri.start_byte() + 1),
            });
        } else if let Some(name) = node.child_by_field_name("name") {
            self.index.imports.push(FdeplImport {
                target: self.qualified_text(name),
                is_uri: false,
                span: node.byte_range(),
                target_span: name.byte_range(),
            });
        }
    }

    fn add_ref(&mut self, node: Node, kind: FdeplRefKind, parent: Option<usize>, ctx: &Ctx, top: bool) -> Option<usize> {
        let mut cursor = node.walk();
        let segments: Vec<Segment> = node
            .children(&mut cursor)
            .filter(|c| c.kind() == "identifier")
            .map(|c| segment(self.text(c), c.byte_range()))
            .collect();
        if segments.is_empty() {
            return None;
        }
        self.index.refs.push(FdeplRef {
            segments,
            kind,
            parent,
            spec: ctx.spec,
            host: ctx.host.clone(),
            is_top_level: top,
        });
        Some(self.index.refs.len() - 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_decl(
        &mut self,
        node: Node,
        name_node: Option<Node>,
        name: String,
        kind: FdeplKind,
        keyword: String,
        parent: Option<usize>,
        detail: Option<String>,
    ) -> usize {
        let name_span = name_node.map(|n| n.byte_range()).unwrap_or_else(|| node.start_byte()..node.start_byte());
        let detail = detail.unwrap_or_else(|| {
            let head = self.text(node).split('{').next().unwrap_or("");
            collapse(head)
        });
        self.index.decls.push(FdeplDecl {
            name,
            kind,
            keyword,
            parent,
            name_span,
            span: node.byte_range(),
            detail,
            base: None,
        });
        self.index.decls.len() - 1
    }

    fn specification(&mut self, node: Node, ctx: &Ctx) {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| self.qualified_text(n)).unwrap_or_default();
        let mut base = None;
        if let Some(b) = node.child_by_field_name("base") {
            base = self.add_ref(b, FdeplRefKind::Spec, None, ctx, true);
        }
        let id = self.push_decl(node, name_node, name, FdeplKind::Specification, String::new(), ctx.parent_decl, None);
        self.index.decls[id].base = base;
        let ctx2 = Ctx { parent_decl: Some(id), ..ctx.clone() };
        self.visit_children(node, &ctx2);
    }

    fn root_deployment(&mut self, node: Node, ctx: &Ctx) {
        let spec = node
            .child_by_field_name("spec")
            .and_then(|s| self.add_ref(s, FdeplRefKind::Spec, None, ctx, true));
        let (keyword, target_node) = match node.kind() {
            "type_collection_deployment" => ("typeCollection".to_string(), node.child_by_field_name("target")),
            "interface_deployment" => ("interface".to_string(), node.child_by_field_name("target")),
            _ => {
                let tag = node.child_by_field_name("tag").map(|t| self.text(t).to_string()).unwrap_or_default();
                (tag, node.child_by_field_name("target"))
            }
        };
        let is_extension = node.kind() == "extension_root";
        let target_ref = match (target_node, is_extension) {
            (Some(t), false) => self.add_ref(t, FdeplRefKind::Target, None, ctx, true),
            _ => None,
        };
        let name_node = target_node.or_else(|| node.child_by_field_name("tag"));
        let name = match target_node {
            Some(t) => self.qualified_text(t),
            None => keyword.clone(),
        };
        let id = self.push_decl(node, name_node, name, FdeplKind::Deployment, keyword.clone(), ctx.parent_decl, None);
        let ctx2 = Ctx {
            parent_decl: Some(id),
            parent_target: target_ref,
            spec,
            host: keyword,
            overwrite: false,
        };
        self.visit_children(node, &ctx2);
    }

    fn member(&mut self, node: Node, ctx: &Ctx) {
        let keyword = node.kind().trim_end_matches("_deployment").to_string();
        let mut target = node.child_by_field_name("target");
        // Operations wrap their name: `operation_name: qualified_name (':' selector)?`.
        if let Some(t) = target {
            if t.kind() == "operation_name" {
                let mut cursor = t.walk();
                target = t.children(&mut cursor).find(|c| c.kind() == "qualified_name");
            }
        }
        let mut target_ref = ctx.parent_target;
        let mut name = String::new();
        if let Some(t) = target {
            name = self.qualified_text(t);
            if !ctx.overwrite {
                target_ref = self.add_ref(t, FdeplRefKind::Target, ctx.parent_target, ctx, false).or(ctx.parent_target);
            }
        }
        let id = self.push_decl(node, target, name, FdeplKind::Member, keyword.clone(), ctx.parent_decl, None);
        let ctx2 = Ctx { parent_decl: Some(id), parent_target: target_ref, host: keyword, ..ctx.clone() };
        self.visit_children(node, &ctx2);
    }
}

fn segment(text: &str, span: Span) -> Segment {
    Segment { text: identifier_name(text).to_string(), span }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Lang;

    pub const SAMPLE: &str = r#"package org.example.deploy

import "platform:/plugin/x/spec.fdepl"
import "local.fidl"

specification org.example.Spec extends org.example.Base {
    for interface {
        Port: Integer (optional);
        Mode: { fast, slow } (default: fast);
    }
    for methods {
        Timeout: Integer;
    }
}

define org.example.Spec for interface org.example.demo.Demo {
    Port = 8
    method move {
        Timeout = 5
        in {
            to {
                Mode = fast
            }
        }
    }
    attribute origin {
        #struct {
            x { Mode = slow }
        }
    }
}
"#;

    fn index() -> FdeplIndex {
        FdeplIndex::build(&Document::new(Lang::Fdepl, SAMPLE.into(), 0))
    }

    #[test]
    fn collects_package_imports_and_specification() {
        let idx = index();
        assert_eq!(idx.package.as_deref(), Some("org.example.deploy"));
        assert_eq!(idx.imports.len(), 2);
        assert!(idx.imports[0].is_external());
        assert!(!idx.imports[1].is_external());
        let spec = idx.decls.iter().find(|d| d.kind == FdeplKind::Specification).unwrap();
        assert_eq!(spec.name, "org.example.Spec");
        assert!(spec.base.is_some());
        let props: Vec<_> = idx.decls.iter().filter(|d| d.kind == FdeplKind::Property).map(|d| d.name.as_str()).collect();
        assert_eq!(props, ["Port", "Mode", "Timeout"]);
        let mode = idx.decls.iter().find(|d| d.name == "Mode").unwrap();
        assert_eq!(mode.detail, "Mode: { fast, slow } (default: fast)");
    }

    #[test]
    fn collects_deployment_tree() {
        let idx = index();
        let names: Vec<(&str, &str)> = idx
            .decls
            .iter()
            .filter(|d| matches!(d.kind, FdeplKind::Deployment | FdeplKind::Member))
            .map(|d| (d.keyword.as_str(), d.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("interface", "org.example.demo.Demo"),
                ("method", "move"),
                ("argument", "to"),
                ("attribute", "origin"),
                ("field", "x"),
            ]
        );
    }

    #[test]
    fn links_targets_to_their_enclosing_target() {
        let idx = index();
        let targets: Vec<&FdeplRef> = idx.refs.iter().filter(|r| r.kind == FdeplRefKind::Target).collect();
        assert_eq!(targets.len(), 4, "overwrite fields are not resolved");
        assert!(targets[0].parent.is_none() && targets[0].is_top_level);
        let method = idx.refs.iter().position(|r| r.full_name() == "move").unwrap();
        assert_eq!(idx.refs[idx.refs[method].parent.unwrap()].full_name(), "org.example.demo.Demo");
        let arg = idx.refs.iter().find(|r| r.full_name() == "to").unwrap();
        assert_eq!(arg.parent, Some(method));
    }

    #[test]
    fn property_references_carry_spec_and_host() {
        let idx = index();
        let timeout = idx.refs.iter().find(|r| r.kind == FdeplRefKind::Property && r.full_name() == "Timeout").unwrap();
        assert_eq!(timeout.host, "method");
        let spec = timeout.spec.unwrap();
        assert_eq!(idx.refs[spec].kind, FdeplRefKind::Spec);
        assert_eq!(idx.refs[spec].full_name(), "org.example.Spec");
        let port = idx.refs.iter().find(|r| r.full_name() == "Port").unwrap();
        assert_eq!(port.host, "interface");
        let x = idx.refs.iter().find(|r| r.kind == FdeplRefKind::Property && r.host == "field").unwrap();
        assert_eq!(x.full_name(), "Mode");
    }

    #[test]
    fn extension_roots_have_no_target_reference() {
        let src = "package a\ndefine x.Spec for provider Foo as bar {\n Port = 1\n instance Inst {\n  Id = 2\n }\n}\n";
        let idx = FdeplIndex::build(&Document::new(Lang::Fdepl, src.into(), 0));
        assert!(!idx.refs.iter().any(|r| r.kind == FdeplRefKind::Target));
        let root = idx.decls.iter().find(|d| d.kind == FdeplKind::Deployment).unwrap();
        assert_eq!(root.keyword, "provider");
        let id = idx.refs.iter().find(|r| r.full_name() == "Id").unwrap();
        assert_eq!(id.host, "instance");
    }

    #[test]
    fn interface_references_in_values() {
        let src = "package a\ndefine x.Spec for typeCollection b.T {\n Ref = &b.Iface\n}\n";
        let idx = FdeplIndex::build(&Document::new(Lang::Fdepl, src.into(), 0));
        let r = idx.refs.iter().find(|r| r.kind == FdeplRefKind::InterfaceValue).unwrap();
        assert_eq!(r.full_name(), "b.Iface");
    }
}
