use std::sync::{Arc, RwLock};

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::completion;
use crate::diagnostics;
use crate::format;
use crate::hover;
use crate::language::Lang;
use crate::loader;
use crate::navigation;
use crate::rename;
use crate::symbols;
use crate::workspace::Workspace;

pub struct Backend {
    client: Client,
    ws: Arc<RwLock<Workspace>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend { client, ws: Arc::new(RwLock::new(Workspace::default())) }
    }

    async fn refresh_open_diagnostics(&self) {
        let open: Vec<Url> = {
            let ws = self.ws.read().unwrap();
            ws.docs.keys().filter(|u| ws.is_open(u)).cloned().collect()
        };
        for uri in open {
            self.publish(uri).await;
        }
    }

    async fn publish(&self, uri: Url) {
        let (diags, version) = {
            let ws = self.ws.read().unwrap();
            match ws.get(&uri) {
                Some(doc) => (diagnostics::diagnostics(&ws, &uri), Some(doc.version)),
                None => return,
            }
        };
        self.client.publish_diagnostics(uri, diags, version).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut roots: Vec<std::path::PathBuf> = params
            .workspace_folders
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.uri.to_file_path().ok())
            .collect();
        #[allow(deprecated)]
        if roots.is_empty() {
            roots.extend(params.root_uri.and_then(|u| u.to_file_path().ok()));
        }
        self.ws.write().unwrap().roots = roots;
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        ..Default::default()
                    },
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), ">".into()]),
                    ..Default::default()
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "francals".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let ws = self.ws.clone();
        let roots = ws.read().unwrap().roots.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            for root in roots {
                for path in loader::find_sources(&root) {
                    let Some((uri, doc)) = loader::read_document(&path) else { continue };
                    let mut ws = ws.write().unwrap();
                    if !ws.docs.contains_key(&uri) {
                        ws.insert(uri, doc);
                    }
                }
            }
        })
        .await;
        if loaded.is_ok() {
            self.refresh_open_diagnostics().await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        let uri = doc.uri.clone();
        let opened = {
            let mut ws = self.ws.write().unwrap();
            let opened = ws.open(doc.uri, &doc.language_id, doc.text, doc.version);
            if opened {
                ws.load_imports(&uri);
            }
            opened
        };
        if opened {
            self.publish(uri).await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut ws = self.ws.write().unwrap();
            ws.change(&uri, params.content_changes, params.text_document.version);
            ws.load_imports(&uri);
        }
        self.refresh_open_diagnostics().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.ws.write().unwrap().close(&uri);
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn document_symbol(&self, params: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let ws = self.ws.read().unwrap();
        let Some(doc) = ws.get(&uri) else { return Ok(None) };
        let symbols = match doc.lang {
            Lang::Fidl => ws.fidl.get(&uri).map(|idx| symbols::fidl_symbols(doc, idx)),
            Lang::Fdepl => ws.fdepl.get(&uri).map(|idx| symbols::fdepl_symbols(doc, idx)),
        };
        Ok(symbols.map(DocumentSymbolResponse::Nested))
    }

    async fn symbol(&self, params: WorkspaceSymbolParams) -> Result<Option<Vec<SymbolInformation>>> {
        let ws = self.ws.read().unwrap();
        Ok(Some(symbols::workspace_symbols(&ws, &params.query)))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let p = params.text_document_position_params;
        let ws = self.ws.read().unwrap();
        Ok(navigation::definition(&ws, &p.text_document.uri, p.position).map(GotoDefinitionResponse::Scalar))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let p = params.text_document_position;
        let ws = self.ws.read().unwrap();
        let Some(target) = navigation::target_at(&ws, &p.text_document.uri, p.position) else {
            return Ok(None);
        };
        Ok(Some(navigation::references(&ws, &target.decl, params.context.include_declaration)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let p = params.text_document_position_params;
        let ws = self.ws.read().unwrap();
        Ok(hover::hover(&ws, &p.text_document.uri, p.position))
    }

    async fn prepare_rename(&self, params: TextDocumentPositionParams) -> Result<Option<PrepareRenameResponse>> {
        let ws = self.ws.read().unwrap();
        Ok(rename::prepare(&ws, &params.text_document.uri, params.position)
            .map(|(range, placeholder)| PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let p = params.text_document_position;
        let ws = self.ws.read().unwrap();
        rename::rename(&ws, &p.text_document.uri, p.position, &params.new_name)
            .map_err(tower_lsp::jsonrpc::Error::invalid_params)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let p = params.text_document_position;
        let ws = self.ws.read().unwrap();
        let items = completion::complete(&ws, &p.text_document.uri, p.position);
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let ws = self.ws.read().unwrap();
        Ok(ws.get(&params.text_document.uri).map(|doc| format::format_document(doc, &params.options)))
    }
}
