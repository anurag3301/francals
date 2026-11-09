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
