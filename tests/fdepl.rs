mod common;

use common::Client;
use serde_json::json;

const FIDL: &str = "package org.demo\ninterface Demo {\n method move { in { Int8 to } }\n}\n";
const SPEC: &str =
    "package spec\nspecification spec.Spec { for interface { Port: Integer; } for methods { Timeout: Integer; } }\n";
const DEPLOY: &str = "package dep\nimport \"spec.fdepl\"\nimport \"demo.fidl\"\ndefine spec.Spec for interface org.demo.Demo {\n Port = 1\n method move {\n  Timeout = 5\n }\n}\n";

fn project(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("francals-fdepl-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("demo.fidl"), FIDL).unwrap();
    std::fs::write(dir.join("spec.fdepl"), SPEC).unwrap();
    let root = url::Url::from_directory_path(&dir).unwrap().to_string();
    (dir, root)
}

#[test]
fn deployment_resolves_through_imports() {
    let (dir, root) = project("imports");
    let mut c = Client::start();
    c.initialize(Some(&root));
    let uri = format!("{root}deploy.fdepl");
    c.open(&uri, "fdepl", DEPLOY);
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.as_array().unwrap().len(), 0, "{diags}");

    let def = c.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": uri}, "position": {"line": 5, "character": 10}}),
    );
    assert!(def["uri"].as_str().unwrap().ends_with("demo.fidl"));
    assert_eq!(def["range"]["start"]["line"], 2);

    let hover =
        c.request("textDocument/hover", json!({"textDocument": {"uri": uri}, "position": {"line": 6, "character": 3}}));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(hover["contents"]["value"].as_str().unwrap().contains("Timeout: Integer"));
    c.shutdown();
}

#[test]
fn renaming_a_declaration_updates_deployments() {
    let (dir, root) = project("rename");
    let mut c = Client::start();
    c.initialize(Some(&root));
    let uri = format!("{root}deploy.fdepl");
    c.open(&uri, "fdepl", DEPLOY);
    let edit = c.request(
        "textDocument/rename",
        json!({"textDocument": {"uri": uri}, "position": {"line": 5, "character": 10}, "newName": "go"}),
    );
    let symbols = c.request("textDocument/documentSymbol", json!({"textDocument": {"uri": uri}}));
    std::fs::remove_dir_all(&dir).unwrap();
    let changes = edit["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 2, "{edit}");
    assert_eq!(symbols.as_array().unwrap().len(), 1);
    c.shutdown();
}
