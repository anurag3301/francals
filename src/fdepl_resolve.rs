//! Name resolution for references inside Franca deployment files.

use tower_lsp::lsp_types::Url;

use crate::fdepl::{FdeplIndex, FdeplKind, FdeplRef, FdeplRefKind};
use crate::fidl::DeclKind;
use crate::workspace::{DeclId, Workspace};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FdeplTarget {
    /// A declaration in a Franca IDL file.
    Fidl(DeclId),
    /// A specification (`specification X { ... }`).
    Spec(Url, usize),
    /// A property declared in a specification.
    Property(Url, usize),
}

impl Workspace {
    pub fn fdepl_decl(&self, uri: &Url, idx: usize) -> Option<&crate::fdepl::FdeplDecl> {
        self.fdepl.get(uri)?.decls.get(idx)
    }

    fn find_fidl(&self, uri: &Url, pkg: Option<&str>, name: &str, accept: impl Fn(DeclKind) -> bool) -> Option<DeclId> {
        let mut names = vec![name.to_string()];
        if let Some(p) = pkg {
            names.push(format!("{p}.{name}"));
        }
        for n in names {
            let Some(list) = self.by_fqn.get(&n) else { continue };
            let ok = |d: &&DeclId| self.decl(d).is_some_and(|d| accept(d.kind));
            if let Some(found) = list.iter().filter(ok).find(|d| &d.uri == uri).or_else(|| list.iter().find(ok)) {
                return Some(found.clone());
            }
        }
        None
    }

    /// An interface or type collection named by a deployment.
    pub fn fdepl_toplevel(&self, uri: &Url, name: &str) -> Option<DeclId> {
        let pkg = self.fdepl.get(uri).and_then(|i| i.package.as_deref());
        self.find_fidl(uri, pkg, name, |k| matches!(k, DeclKind::Interface | DeclKind::TypeCollection))
            .or_else(|| self.find_anonymous_collection(name))
    }

    /// The declaration called `name` directly below `parent`.
    pub fn fidl_member(&self, parent: &DeclId, name: &str) -> Option<DeclId> {
        let fqn = format!("{}.{name}", self.decl(parent)?.fqn);
        let list = self.by_fqn.get(&fqn)?;
        list.iter().find(|d| d.uri == parent.uri).or_else(|| list.first()).cloned()
    }

    /// An anonymous `typeCollection`, which deployments name by its package.
    fn find_anonymous_collection(&self, name: &str) -> Option<DeclId> {
        let mut uris: Vec<&Url> = self.fidl.keys().collect();
        uris.sort();
        uris.into_iter().find_map(|u| {
            let idx = &self.fidl[u];
            if idx.package.as_deref() != Some(name) {
                return None;
            }
            let i = idx.decls.iter().position(|d| d.kind == DeclKind::TypeCollection && d.name.is_empty())?;
            Some(DeclId { uri: u.clone(), idx: i })
        })
    }

    /// Finds a specification by its (possibly package-relative) name.
    pub fn find_spec(&self, uri: &Url, name: &str) -> Option<(Url, usize)> {
        let local_pkg = self.fdepl.get(uri).and_then(|i| i.package.clone());
        let mut names = vec![name.to_string()];
        if let Some(p) = local_pkg {
            names.push(format!("{p}.{name}"));
        }
        let mut uris: Vec<&Url> = self.fdepl.keys().collect();
        uris.sort_by_key(|u| *u != uri);
        for n in names {
            for u in &uris {
                let idx = &self.fdepl[*u];
                if let Some(i) = idx.decls.iter().position(|d| d.kind == FdeplKind::Specification && d.name == n) {
                    return Some(((*u).clone(), i));
                }
            }
        }
        None
    }

    pub fn resolve_fdepl(&self, uri: &Url, ref_idx: usize) -> Option<FdeplTarget> {
        let idx = self.fdepl.get(uri)?;
        self.resolve_fdepl_in(uri, idx, ref_idx, 0)
    }

    fn resolve_fdepl_in(&self, uri: &Url, idx: &FdeplIndex, ref_idx: usize, depth: usize) -> Option<FdeplTarget> {
        let r = idx.refs.get(ref_idx)?;
        match r.kind {
            FdeplRefKind::Spec => {
                let (u, i) = self.find_spec(uri, &r.full_name())?;
                Some(FdeplTarget::Spec(u, i))
            }
            FdeplRefKind::InterfaceValue => {
                let d = self.find_fidl(uri, idx.package.as_deref(), &r.full_name(), |k| k == DeclKind::Interface)?;
                Some(FdeplTarget::Fidl(d))
            }
            FdeplRefKind::Target => match r.parent {
                None => {
                    let d = self.fdepl_toplevel(uri, &r.full_name())?;
                    Some(FdeplTarget::Fidl(d))
                }
                Some(p) if depth < 8 => {
                    let Some(FdeplTarget::Fidl(parent)) = self.resolve_fdepl_in(uri, idx, p, depth + 1) else {
                        return None;
                    };
                    self.fidl_member(&parent, &r.full_name()).map(FdeplTarget::Fidl)
                }
                _ => None,
            },
            FdeplRefKind::Property => {
                let spec = self.resolve_fdepl_in(uri, idx, r.spec?, depth + 1);
                let Some(FdeplTarget::Spec(u, i)) = spec else { return None };
                self.find_property(&u, i, r).map(|(u, i)| FdeplTarget::Property(u, i))
            }
        }
    }

    /// Looks up the declaration of the property set by `r`, following `extends`.
    pub fn find_property(&self, spec_uri: &Url, spec: usize, r: &FdeplRef) -> Option<(Url, usize)> {
        let name = r.full_name();
        let (mut uri, mut spec) = (spec_uri.clone(), spec);
        for _ in 0..16 {
            let idx = self.fdepl.get(&uri)?;
            let mut any = None;
            for (i, d) in idx.decls.iter().enumerate() {
                if d.kind != FdeplKind::Property || d.name != name {
                    continue;
                }
                let Some(host) = d.parent.map(|h| &idx.decls[h]) else { continue };
                if host.parent != Some(spec) {
                    continue;
                }
                if host_matches(&host.name, &r.host) {
                    return Some((uri, i));
                }
                any.get_or_insert(i);
            }
            if let Some(i) = any {
                return Some((uri, i));
            }
            let base = idx.decls[spec].base?;
            let Some(FdeplTarget::Spec(u, i)) = self.resolve_fdepl_in(&uri, idx, base, 1) else { return None };
            (uri, spec) = (u, i);
        }
        None
    }

    /// All property declarations visible through a specification, nearest first.
    pub fn spec_properties(&self, spec_uri: &Url, spec: usize) -> Vec<(Url, usize)> {
        let (mut uri, mut spec) = (spec_uri.clone(), spec);
        let mut out = Vec::new();
        for _ in 0..16 {
            let Some(idx) = self.fdepl.get(&uri) else { break };
            for (i, d) in idx.decls.iter().enumerate() {
                if d.kind == FdeplKind::Property && d.parent.is_some_and(|h| idx.decls[h].parent == Some(spec)) {
                    out.push((uri.clone(), i));
                }
            }
            let Some(base) = idx.decls[spec].base else { break };
            let Some(FdeplTarget::Spec(u, i)) = self.resolve_fdepl_in(&uri, idx, base, 1) else { break };
            (uri, spec) = (u, i);
        }
        out
    }
}

/// Whether a specification `for <host>` section applies to deployments of `keyword`.
pub fn host_matches(host: &str, keyword: &str) -> bool {
    let h = host.to_lowercase().replace('_', "");
    let k = keyword.to_lowercase();
    h == k || h == format!("{k}s") || h == format!("{k}es") || (k.ends_with('y') && h == format!("{}ies", &k[..k.len() - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIDL: &str = "package org.example.demo\ninterface Demo {\n method move {\n  in { Int8 to }\n }\n attribute Int8 origin\n struct Pt { Int8 x }\n}\n";
    const SPEC: &str = "package org.example\nspecification org.example.Base {\n for interface { Port: Integer; }\n}\nspecification org.example.Spec extends org.example.Base {\n for methods { Timeout: Integer; }\n for attributes { Timeout: Integer; }\n}\n";
    const DEPLOY: &str = "package org.example.deploy\nimport \"spec.fdepl\"\ndefine org.example.Spec for interface org.example.demo.Demo {\n Port = 8\n method move {\n  Timeout = 5\n  in { to { } }\n }\n attribute origin { Timeout = 1 }\n struct Pt { x { } }\n}\n";

    fn ws() -> Workspace {
        let mut ws = Workspace::default();
        for (n, l, t) in [("a.fidl", "franca", FIDL), ("spec.fdepl", "fdepl", SPEC), ("d.fdepl", "fdepl", DEPLOY)] {
            ws.open(Url::parse(&format!("file:///ws/{n}")).unwrap(), l, t.into(), 1);
        }
        ws
    }

    fn resolve(ws: &Workspace, kind: FdeplRefKind, name: &str, host: Option<&str>) -> Option<FdeplTarget> {
        let uri = Url::parse("file:///ws/d.fdepl").unwrap();
        let idx = ws.fdepl.get(&uri).unwrap();
        let i = idx
            .refs
            .iter()
            .position(|r| r.kind == kind && r.full_name() == name && host.is_none_or(|h| r.host == h))?;
        ws.resolve_fdepl(&uri, i)
    }

    fn fidl_fqn(ws: &Workspace, t: Option<FdeplTarget>) -> Option<String> {
        match t? {
            FdeplTarget::Fidl(d) => Some(ws.decl(&d)?.fqn.clone()),
            _ => None,
        }
    }

    #[test]
    fn resolves_targets_relative_to_their_parents() {
        let ws = ws();
        let r = |n: &str| fidl_fqn(&ws, resolve(&ws, FdeplRefKind::Target, n, None));
        assert_eq!(r("org.example.demo.Demo").as_deref(), Some("org.example.demo.Demo"));
        assert_eq!(r("move").as_deref(), Some("org.example.demo.Demo.move"));
        assert_eq!(r("to").as_deref(), Some("org.example.demo.Demo.move.to"));
        assert_eq!(r("origin").as_deref(), Some("org.example.demo.Demo.origin"));
        assert_eq!(r("x").as_deref(), Some("org.example.demo.Demo.Pt.x"));
    }

    #[test]
    fn anonymous_type_collections_are_named_by_package() {
        let mut ws = Workspace::default();
        ws.open(Url::parse("file:///ws/t.fidl").unwrap(), "franca", "package a.b\ntypeCollection {\n struct S { Int8 f }\n}\n".into(), 1);
        let u = Url::parse("file:///ws/t.fdepl").unwrap();
        ws.open(u.clone(), "fdepl", "package d\ndefine x.Spec for typeCollection a.b {\n struct S { f { } }\n}\n".into(), 1);
        let idx = &ws.fdepl[&u];
        let fqns: Vec<String> = (0..idx.refs.len())
            .filter(|&i| idx.refs[i].kind == FdeplRefKind::Target)
            .filter_map(|i| match ws.resolve_fdepl(&u, i)? {
                FdeplTarget::Fidl(d) => Some(ws.fidl[&d.uri].decls[d.idx].fqn.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(fqns, ["a.b", "a.b.S", "a.b.S.f"]);
    }

    #[test]
    fn resolves_specifications_and_inherited_properties() {
        let ws = ws();
        let Some(FdeplTarget::Spec(u, i)) = resolve(&ws, FdeplRefKind::Spec, "org.example.Spec", None) else {
            panic!("spec not found")
        };
        assert_eq!(u.path(), "/ws/spec.fdepl");
        assert_eq!(ws.fdepl_decl(&u, i).unwrap().name, "org.example.Spec");
        let Some(FdeplTarget::Property(u, i)) = resolve(&ws, FdeplRefKind::Property, "Port", None) else {
            panic!("inherited property not found")
        };
        let d = ws.fdepl_decl(&u, i).unwrap();
        assert_eq!((d.name.as_str(), d.detail.as_str()), ("Port", "Port: Integer"));
    }

    #[test]
    fn property_lookup_prefers_matching_host() {
        let ws = ws();
        let Some(FdeplTarget::Property(u, i)) = resolve(&ws, FdeplRefKind::Property, "Timeout", Some("attribute")) else {
            panic!("property not found")
        };
        let idx = &ws.fdepl[&u];
        assert_eq!(idx.decls[idx.decls[i].parent.unwrap()].name, "attributes");
    }

    #[test]
    fn unknown_names_do_not_resolve() {
        let mut ws = ws();
        let u = Url::parse("file:///ws/e.fdepl").unwrap();
        ws.open(u.clone(), "fdepl", "package e\ndefine nope.Spec for interface no.Such {\n method m { P = 1 }\n}\n".into(), 1);
        let idx = &ws.fdepl[&u];
        for i in 0..idx.refs.len() {
            assert!(ws.resolve_fdepl(&u, i).is_none());
        }
    }
}
