use std::path::{Path, PathBuf};

fn main() {
    let core_dir = std::fs::canonicalize("../core").expect("Cannot find crates/core");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("core_embed.rs");

    let files = collect_files(&core_dir);

    let mut code = String::from("pub const CORE_FILES: &[(&str, &str)] = &[\n");
    for (rel, abs) in &files {
        // Use include_str! with absolute paths — resolved at compile time
        code.push_str(&format!(
            "    (\"{}\", include_str!(\"{}\")),\n",
            rel,
            abs.display()
        ));
        println!("cargo:rerun-if-changed={}", abs.display());
    }
    code.push_str("];\n\n");

    // Also embed lib/prelude.lisp (needed by core's include_str! in prelude.rs)
    let prelude_path = std::fs::canonicalize("../../lib/prelude.lisp")
        .expect("Cannot find lib/prelude.lisp");
    code.push_str(&format!(
        "pub const PRELUDE_LISP: &str = include_str!(\"{}\");\n",
        prelude_path.display()
    ));
    println!("cargo:rerun-if-changed={}", prelude_path.display());

    std::fs::write(dest, code).unwrap();
}

fn collect_files(base: &Path) -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    walk(base, base, &mut result);
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_str().unwrap();
            // Skip target/, tests/, benches/, etc.
            if name == "target" || name == "tests" || name == "benches" || name == ".git" {
                continue;
            }
            walk(base, &path, out);
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let name = path.file_name().unwrap().to_str().unwrap();
            if ext == "rs" || name == "Cargo.toml" {
                let rel = path.strip_prefix(base).unwrap();
                out.push((rel.to_string_lossy().to_string(), path.clone()));
            }
        }
    }
}
