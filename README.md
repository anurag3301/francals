# francals

A language server for [Franca IDL](https://github.com/franca/franca) (`.fidl`) and Franca deployment (`.fdepl`) files, written in Rust on top of [tower-lsp](https://github.com/ebkalderon/tower-lsp) and [tree-sitter](https://tree-sitter.github.io).

The grammars come from [tree-sitter-franca](https://github.com/anurag3301/tree-sitter-franca) and are vendored under `grammars/`.

## Features

| | `.fidl` | `.fdepl` |
|---|---|---|
| Syntax diagnostics | yes | yes |
| Unresolved names, missing imports | yes | yes |
| Property value type checks | | yes |
| Duplicate definitions | yes | |
| Document and workspace symbols | yes | yes |
| Go to definition (across imports) | yes | yes (specs, targets, properties, imports) |
| Find references | yes (includes deployments) | yes |
| Hover | yes (signature, `@description`, builtin types) | yes (property declarations, targets) |
| Completion | keywords with block snippets, types (with docs), qualified names, contract members | specs, properties, values, targets |
| Document highlight | yes | yes |
| Code actions | add missing import | add missing import |
| Rename | yes (across files, includes deployments) | via the deployed declaration |
| Formatting | yes | yes |

Imports are followed on disk, so definitions work even for files that are not open. Workspace folders are scanned on startup.

## Install

```sh
cargo install --path .
```

This needs a Rust toolchain and a C compiler (for the vendored parsers).

## Editor setup

### Neovim (0.11+)

```lua
vim.filetype.add({ extension = { fidl = 'franca', fdepl = 'fdepl' } })

vim.lsp.config('francals', {
  cmd = { 'francals' },
  filetypes = { 'franca', 'fdepl' },
  root_markers = { '.git' },
})
vim.lsp.enable('francals')
```

Any other editor works the same way: run `francals` with no arguments and talk LSP over stdio.

## Command line

`francals check` prints the same diagnostics without an editor, which is handy for CI:

```sh
francals check path/to/interfaces      # files or directories
francals check -q path/to/interfaces   # count warnings but only print errors
```

The exit code is 1 when any error was found.

## Notes on name resolution

Lookup is deliberately lenient: a name resolves if it is visible through the enclosing scope (including inherited interfaces), the package, an import, or as a fully qualified name, and any file loaded in the workspace counts. Unresolved types are reported as warnings, not errors.

Deployments inside `define ... for interface X` are matched against the Franca declaration `X`; `platform:` and `classpath:` imports cannot be followed, so specification warnings are suppressed in files that use them.

## Development

```sh
cargo test
cargo clippy --all-targets
cargo fmt --check
```

If the grammars change, copy the regenerated `src/parser.c` and `src/tree_sitter/*.h` into `grammars/franca` or `grammars/fdepl`.

## Licence

MIT
