mod common;

use common::Client;
use serde_json::json;

fn project() -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("francals-nav-{}-{:?}", std::process::id(), std::thread::current().id()));
    let dir = dir.to_string_lossy().replace("ThreadId(", "").replace(')', "");
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("common.fidl"),
        "package org.common\ntypeCollection Types {\n struct Point { Int8 x }\n}\n",
    )
    .unwrap();
    let root = url::Url::from_directory_path(&dir).unwrap().to_string();
    (dir, root)
}

const MAIN: &str = "package org.app\nimport org.common.* from \"common.fidl\"\ninterface I {\n attribute Types.Point a\n}\n";

#[test]
fn definition_and_references_across_files() {
    let (dir, root) = project();
    let mut c = Client::start();
    c.initialize(Some(&root));
    let main = format!("{root}main.fidl");
    c.open(&main, "franca", MAIN);

    let def = c.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": main}, "position": {"line": 3, "character": 22}}),
    );
    assert!(def["uri"].as_str().unwrap().ends_with("common.fidl"));
    assert_eq!(def["range"]["start"], json!({"line": 2, "character": 8}));

    let refs = c.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": main},
            "position": {"line": 3, "character": 22},
            "context": {"includeDeclaration": true}
        }),
    );
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(refs.as_array().unwrap().len(), 2);
    c.shutdown();
}

#[test]
fn renames_across_files() {
    let (dir, root) = project();
    let mut c = Client::start();
    c.initialize(Some(&root));
    let main = format!("{root}main.fidl");
    c.open(&main, "franca", MAIN);

    let prep = c.request(
        "textDocument/prepareRename",
        json!({"textDocument": {"uri": main}, "position": {"line": 3, "character": 22}}),
    );
    assert_eq!(prep["placeholder"], "Point");

    let edit = c.request(
        "textDocument/rename",
        json!({"textDocument": {"uri": main}, "position": {"line": 3, "character": 22}, "newName": "Vec2"}),
    );
    std::fs::remove_dir_all(&dir).unwrap();
    let changes = edit["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes.values().flat_map(|v| v.as_array().unwrap()).all(|e| e["newText"] == "Vec2"));
    c.shutdown();
}
