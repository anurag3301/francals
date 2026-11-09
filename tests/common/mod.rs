#![allow(dead_code)]
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

pub struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: i64,
    pending: Vec<Value>,
}

impl Client {
    pub fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_francals"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn francals");
        let stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            loop {
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if out.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.strip_prefix("Content-Length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut buf = vec![0; len];
                if out.read_exact(&mut buf).is_err() {
                    return;
                }
                if tx.send(serde_json::from_slice(&buf).unwrap()).is_err() {
                    return;
                }
            }
        });
        Client { child, stdin, rx, next_id: 1, pending: Vec::new() }
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.stdin.flush().unwrap();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            msg["params"] = params;
        }
        self.send(msg);
        loop {
            let msg = self.rx.recv_timeout(Duration::from_secs(20)).expect("response timed out");
            if msg.get("id") == Some(&json!(id)) && msg.get("method").is_none() {
                assert!(msg.get("error").is_none(), "error response: {msg}");
                return msg["result"].clone();
            }
            self.pending.push(msg);
        }
    }

    /// Waits for a notification with the given method, optionally matching `pred`.
    pub fn wait_for(&mut self, method: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(i) = self.pending.iter().position(|m| m["method"] == method && pred(&m["params"])) {
            return self.pending.remove(i)["params"].clone();
        }
        loop {
            let msg = self.rx.recv_timeout(Duration::from_secs(20)).expect("notification timed out");
            if msg["method"] == method && pred(&msg["params"]) {
                return msg["params"].clone();
            }
            self.pending.push(msg);
        }
    }

    pub fn initialize(&mut self, root: Option<&str>) -> Value {
        let folders = match root {
            Some(r) => json!([{"uri": r, "name": "ws"}]),
            None => Value::Null,
        };
        let result = self.request(
            "initialize",
            json!({"processId": null, "rootUri": root, "workspaceFolders": folders, "capabilities": {}}),
        );
        self.notify("initialized", json!({}));
        result
    }

    pub fn open(&mut self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": language_id, "version": 1, "text": text}}),
        );
    }

    pub fn diagnostics(&mut self, uri: &str) -> Value {
        self.wait_for("textDocument/publishDiagnostics", |p| p["uri"] == uri)["diagnostics"].clone()
    }

    pub fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.send(json!({"jsonrpc": "2.0", "method": "exit"}));
        let Client { mut child, stdin, .. } = self;
        drop(stdin);
        let _ = child.wait();
    }
}
