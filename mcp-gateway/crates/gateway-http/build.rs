use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MCP_GATEWAY_BUNDLED_RG_PATH");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let generated = out_dir.join("bundled_tools.rs");

    let Some(path) = env::var_os("MCP_GATEWAY_BUNDLED_RG_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    else {
        fs::write(
            generated,
            "const BUNDLED_RIPGREP: Option<BundledTool> = None;\n",
        )
        .expect("write bundled tools metadata");
        return;
    };

    println!("cargo:rerun-if-changed={}", path.display());

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .expect("bundled ripgrep path has a UTF-8 file name")
        .to_string();
    let path_literal = path
        .canonicalize()
        .unwrap_or_else(|_| path.clone())
        .to_string_lossy()
        .replace('\\', "/");
    let source = format!(
        "const BUNDLED_RIPGREP: Option<BundledTool> = Some(BundledTool {{ file_name: {file_name:?}, bytes: include_bytes!(r#\"{path_literal}\"#) }});\n"
    );

    fs::write(generated, source).expect("write bundled tools metadata");
}
