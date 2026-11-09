fn main() {
    for name in ["franca", "fdepl"] {
        let dir = format!("grammars/{name}");
        cc::Build::new()
            .include(&dir)
            .file(format!("{dir}/parser.c"))
            .flag_if_supported("-Wno-unused-parameter")
            .flag_if_supported("-Wno-unused-but-set-variable")
            .flag_if_supported("-Wno-trigraphs")
            .compile(&format!("tree-sitter-{name}"));
        println!("cargo:rerun-if-changed={dir}/parser.c");
    }
}
