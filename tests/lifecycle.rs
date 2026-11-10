mod common;

use common::Client;

#[test]
fn initializes_and_reports_syntax_errors() {
    let mut c = Client::start();
    let init = c.initialize(None);
    assert_eq!(init["serverInfo"]["name"], "francals");

    let uri = "file:///tmp/francals-test/broken.fidl";
    c.open(uri, "franca", "package org.example.demo\ninterface Foo {\n  bogus here\n}\n");
    let diags = c.diagnostics(uri);
    assert!(diags.as_array().unwrap().iter().any(|d| d["range"]["start"]["line"] == 2));

    c.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "package org.example.demo\ninterface Foo {\n}\n"}]
        }),
    );
    let diags = c.wait_for("textDocument/publishDiagnostics", |p| p["uri"] == uri && p["version"] == 2);
    assert!(diags["diagnostics"].as_array().unwrap().is_empty());
    c.shutdown();
}

#[test]
fn returns_document_symbols() {
    let mut c = Client::start();
    c.initialize(None);
    let uri = "file:///tmp/francals-test/sym.fidl";
    c.open(uri, "franca", "package a\ninterface I {\n  attribute Int8 x\n}\n");
    let res = c.request("textDocument/documentSymbol", serde_json::json!({"textDocument": {"uri": uri}}));
    assert_eq!(res[0]["name"], "I");
    assert_eq!(res[0]["children"][0]["name"], "x");
    c.shutdown();
}

#[test]
fn indexes_workspace_folder_and_imports() {
    let dir = std::env::temp_dir().join(format!("francals-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.fidl"), "package p\ntypeCollection T { struct S { Int8 f } }\n").unwrap();
    let root = url::Url::from_directory_path(&dir).unwrap().to_string();
    let mut c = Client::start();
    c.initialize(Some(&root));
    let uri = format!("{root}b.fidl");
    c.open(&uri, "franca", "package q\nimport p.* from \"a.fidl\"\n");
    let res = c.request("workspace/symbol", serde_json::json!({"query": "S"}));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(res.as_array().unwrap().iter().any(|s| s["name"] == "S" && s["location"]["uri"].as_str().unwrap().ends_with("a.fidl")));
    c.shutdown();
}

#[test]
fn completes_keywords_and_types() {
    let mut c = Client::start();
    c.initialize(None);
    let uri = "file:///tmp/francals-test/comp.fidl";
    c.open(uri, "franca", "package a\ninterface I {\n  attribute \n}\n");
    let res = c.request(
        "textDocument/completion",
        serde_json::json!({"textDocument": {"uri": uri}, "position": {"line": 2, "character": 12}}),
    );
    let labels: Vec<&str> = res.as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"UInt8"));
    c.shutdown();
}
