use francals::server::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("francals {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let (service, socket) = LspService::new(Backend::new);
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
}
