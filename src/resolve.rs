//! Name resolution for Franca IDL references.

use tower_lsp::lsp_types::Url;

use crate::fidl::{DeclKind, FidlIndex, Ref, RefKind};
use crate::workspace::{DeclId, Workspace};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub target: DeclId,
    /// How many leading segments of the reference name the target covers.
    pub matched: usize,
}

fn accepts(kind: RefKind, d: DeclKind) -> bool {
    match kind {
        RefKind::Type => d.is_type(),
        RefKind::Base => d.is_type() || d == DeclKind::Interface,
        RefKind::Value => true,
        RefKind::Member => matches!(
            d,
            DeclKind::Method | DeclKind::Broadcast | DeclKind::Attribute | DeclKind::State | DeclKind::Variable
        ),
    }
}

impl Workspace {
    /// First declaration named `fqn` that satisfies `kind`; prefers `uri`'s own file.
    pub fn lookup(&self, fqn: &str, kind: RefKind, uri: &Url) -> Option<DeclId> {
        let list = self.by_fqn.get(fqn)?;
        let ok = |d: &&DeclId| self.fidl.get(&d.uri).is_some_and(|i| accepts(kind, i.decls[d.idx].kind));
        list.iter().filter(ok).find(|d| &d.uri == uri).or_else(|| list.iter().find(ok)).cloned()
    }

    pub fn decl(&self, id: &DeclId) -> Option<&crate::fidl::Decl> {
        self.fidl.get(&id.uri)?.decls.get(id.idx)
    }

    /// Namespaces searched for names inside the interface `scope`, innermost first.
    pub(crate) fn scope_chain(&self, uri: &Url, idx: &FidlIndex, scope: usize) -> Vec<String> {
        let mut out = vec![idx.decls[scope].fqn.clone()];
        let mut current = DeclId { uri: uri.clone(), idx: scope };
        for _ in 0..16 {
            let Some(index) = self.fidl.get(&current.uri) else { break };
            let decl = &index.decls[current.idx];
            if decl.kind != DeclKind::Interface {
                break;
            }
            let Some(base) = decl.base else { break };
            let Some(r) = self.resolve_in(&current.uri, index, &index.refs[base]) else { break };
            if out.contains(&self.decl(&r.target).map(|d| d.fqn.clone()).unwrap_or_default()) {
                break;
            }
            match self.decl(&r.target) {
                Some(d) => out.push(d.fqn.clone()),
                None => break,
            }
            current = r.target;
        }
        out
    }

    pub(crate) fn candidates(&self, uri: &Url, idx: &FidlIndex, r: &Ref, name: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(scope) = r.scope {
            for ns in self.scope_chain(uri, idx, scope) {
                out.push(format!("{ns}.{name}"));
            }
        }
        if r.kind == RefKind::Member {
            return out;
        }
        if let Some(pkg) = &idx.package {
            out.push(format!("{pkg}.{name}"));
        }
        for import in &idx.imports {
            let Some(ns) = &import.namespace else { continue };
            if let Some(prefix) = import.wildcard() {
                out.push(format!("{prefix}.{name}"));
            } else if let Some((prefix, last)) = ns.rsplit_once('.')
                && (name == last || name.starts_with(&format!("{last}.")))
            {
                out.push(format!("{prefix}.{name}"));
            }
        }
        out.push(name.to_string());
        out
    }

    pub fn resolve_in(&self, uri: &Url, idx: &FidlIndex, r: &Ref) -> Option<Resolved> {
        let longest = r.segments.len();
        let shortest = if r.kind == RefKind::Value { 1 } else { longest };
        for matched in (shortest..=longest).rev() {
            let name = r.name(matched);
            for cand in self.candidates(uri, idx, r, &name) {
                if let Some(target) = self.lookup(&cand, r.kind, uri) {
                    return Some(Resolved { target, matched });
                }
            }
        }
        None
    }

    pub fn resolve(&self, uri: &Url, ref_idx: usize) -> Option<Resolved> {
        let idx = self.fidl.get(uri)?;
        self.resolve_in(uri, idx, idx.refs.get(ref_idx)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_with(files: &[(&str, &str)]) -> Workspace {
        let mut ws = Workspace::default();
        for (name, text) in files {
            let uri = Url::parse(&format!("file:///ws/{name}")).unwrap();
            ws.open(uri, "franca", text.to_string(), 1);
        }
        ws
    }

    fn target_fqn(ws: &Workspace, file: &str, needle: &str) -> Option<String> {
        let uri = Url::parse(&format!("file:///ws/{file}")).unwrap();
        let idx = ws.fidl.get(&uri)?;
        let i = idx.refs.iter().position(|r| r.name(r.segments.len()) == needle)?;
        let r = ws.resolve(&uri, i)?;
        Some(ws.decl(&r.target)?.fqn.clone())
    }

    const COMMON: &str =
        "package org.common\ntypeCollection Types {\n struct Point { Int8 x }\n enumeration Mode { A B }\n}\n";

    #[test]
    fn resolves_through_wildcard_import() {
        let ws = ws_with(&[
            ("common.fidl", COMMON),
            (
                "main.fidl",
                "package org.app\nimport org.common.* from \"common.fidl\"\ninterface I {\n attribute Types.Point p\n}\n",
            ),
        ]);
        assert_eq!(target_fqn(&ws, "main.fidl", "Types.Point").as_deref(), Some("org.common.Types.Point"));
    }

    #[test]
    fn resolves_explicit_import_and_fully_qualified() {
        let ws = ws_with(&[
            ("common.fidl", COMMON),
            (
                "main.fidl",
                "package org.app\nimport org.common.Types from \"common.fidl\"\ninterface I {\n attribute Types.Mode m\n attribute org.common.Types.Point p\n}\n",
            ),
        ]);
        assert_eq!(target_fqn(&ws, "main.fidl", "Types.Mode").as_deref(), Some("org.common.Types.Mode"));
        assert_eq!(target_fqn(&ws, "main.fidl", "org.common.Types.Point").as_deref(), Some("org.common.Types.Point"));
    }

    #[test]
    fn resolves_local_types_and_inherited_ones() {
        let ws = ws_with(&[(
            "a.fidl",
            "package a\ninterface Base {\n struct Inner { Int8 f }\n}\ninterface Child extends Base {\n attribute Inner i\n attribute Own o\n struct Own { Int8 f }\n}\n",
        )]);
        assert_eq!(target_fqn(&ws, "a.fidl", "Inner").as_deref(), Some("a.Base.Inner"));
        assert_eq!(target_fqn(&ws, "a.fidl", "Own").as_deref(), Some("a.Child.Own"));
        assert_eq!(target_fqn(&ws, "a.fidl", "Base").as_deref(), Some("a.Base"));
    }

    #[test]
    fn unknown_names_do_not_resolve() {
        let ws = ws_with(&[("a.fidl", "package a\ninterface I {\n attribute Missing m\n}\n")]);
        assert_eq!(target_fqn(&ws, "a.fidl", "Missing"), None);
    }

    #[test]
    fn type_references_ignore_non_types() {
        let ws = ws_with(&[("a.fidl", "package a\ninterface I {\n method I2 { }\n attribute I2 x\n}\n")]);
        assert_eq!(target_fqn(&ws, "a.fidl", "I2"), None);
    }

    #[test]
    fn contract_members_resolve_inside_their_interface() {
        let ws = ws_with(&[(
            "a.fidl",
            "package a\ninterface I {\n method m { }\n contract {\n  PSM {\n   initial s\n   state s { on call m -> s }\n  }\n }\n}\n",
        )]);
        assert_eq!(target_fqn(&ws, "a.fidl", "m").as_deref(), Some("a.I.m"));
    }
}
