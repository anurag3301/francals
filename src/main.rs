use francals::server::Backend;
use tower_lsp::{LspService, Server};

const USAGE: &str = "\
francals - language server for Franca IDL (.fidl) and FDeploy (.fdepl)

USAGE:
    francals                      run the language server on stdio
    francals check [-q] <paths>   print diagnostics for files or directories
    francals --version

OPTIONS (check):
    -q    do not print warnings (they are still counted)
";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => println!("francals {}", env!("CARGO_PKG_VERSION")),
        Some("--help" | "-h") => print!("{USAGE}"),
        Some("check") => {
            let quiet = args.iter().any(|a| a == "-q");
            let paths: Vec<_> = args[1..].iter().filter(|a| *a != "-q").map(Into::into).collect();
            if paths.is_empty() {
                eprint!("{USAGE}");
                std::process::exit(2);
            }
            let s = francals::check::run(&paths, quiet);
            eprintln!("{} files, {} errors, {} warnings", s.files, s.errors, s.warnings);
            std::process::exit(if s.errors > 0 { 1 } else { 0 });
        }
        None => {
            let (service, socket) = LspService::new(Backend::new);
            Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
        }
        Some(other) => {
            eprintln!("unknown argument `{other}`\n");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}
