use std::sync::{Arc, RwLock};

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics;
use crate::workspace::Workspace;

pub struct Backend {
    client: Client,
    ws: Arc<RwLock<Workspace>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend { client, ws: Arc::new(RwLock::new(Workspace::default())) }
    }

    async fn publish(&self, uri: Url) {
        let (diags, version) = {
            let ws = self.ws.read().unwrap();
            match ws.get(&uri) {
                Some(doc) => (diagnostics::syntax_diagnostics(doc), Some(doc.version)),
                None => return,
            }
        };
        self.client.publish_diagnostics(uri, diags, version).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "francals".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        let uri = doc.uri.clone();
        let opened = self.ws.write().unwrap().open(doc.uri, &doc.language_id, doc.text, doc.version);
        if opened {
            self.publish(uri).await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        self.ws.write().unwrap().change(&uri, params.content_changes, params.text_document.version);
        self.publish(uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.ws.write().unwrap().close(&uri);
        self.client.publish_diagnostics(uri, vec![], None).await;
    }
}
