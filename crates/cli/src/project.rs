use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Crate dependency specification (Cargo.toml compatible)
#[derive(Debug, Clone)]
pub struct CrateDep {
    pub version: String,
    pub features: Vec<String>,
    pub path: Option<String>,
}

impl CrateDep {
    pub fn simple(version: &str) -> Self {
        Self {
            version: version.to_string(),
            features: Vec::new(),
            path: None,
        }
    }

    /// Convert to Cargo.toml dependency value
    pub fn to_cargo_toml_value(&self) -> String {
        if self.features.is_empty() && self.path.is_none() {
            format!("\"{}\"", self.version)
        } else {
            let mut parts = vec![format!("version = \"{}\"", self.version)];
            if !self.features.is_empty() {
                let feats: Vec<String> = self.features.iter().map(|f| format!("\"{}\"", f)).collect();
                parts.push(format!("features = [{}]", feats.join(", ")));
            }
            if let Some(p) = &self.path {
                parts.push(format!("path = \"{}\"", p));
            }
            format!("{{ {} }}", parts.join(", "))
        }
    }

    /// Convert to lisp.toml value
    fn to_toml_value(&self) -> toml::Value {
        if self.features.is_empty() && self.path.is_none() {
            toml::Value::String(self.version.clone())
        } else {
            let mut t = toml::Table::new();
            t.insert("version".to_string(), toml::Value::String(self.version.clone()));
            if !self.features.is_empty() {
                let arr: Vec<toml::Value> = self.features.iter().map(|f| toml::Value::String(f.clone())).collect();
                t.insert("features".to_string(), toml::Value::Array(arr));
            }
            if let Some(p) = &self.path {
                t.insert("path".to_string(), toml::Value::String(p.clone()));
            }
            toml::Value::Table(t)
        }
    }

    fn from_toml_value(val: &toml::Value) -> Self {
        match val {
            toml::Value::String(s) => Self::simple(s),
            toml::Value::Table(t) => {
                let version = t.get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .to_string();
                let features = t.get("features")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let path = t.get("path").and_then(|v| v.as_str()).map(String::from);
                Self { version, features, path }
            }
            _ => Self::simple("*"),
        }
    }
}

#[derive(Debug)]
pub struct Project {
    pub name: String,
    pub version: String,
    pub dependencies: BTreeMap<String, CrateDep>,
    pub root: PathBuf,
}

impl Project {
    /// Find lisp.toml by walking up from current directory
    pub fn find() -> Option<Self> {
        let mut dir = std::env::current_dir().ok()?;
        loop {
            let toml_path = dir.join("lisp.toml");
            if toml_path.exists() {
                return Self::load(&toml_path).ok();
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let table: toml::Table = content
            .parse()
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

        let project = table
            .get("project")
            .and_then(|v| v.as_table())
            .ok_or("Missing [project] section in lisp.toml")?;

        let name = project
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("Missing project.name")?
            .to_string();

        let version = project
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0")
            .to_string();

        let mut dependencies = BTreeMap::new();
        if let Some(deps) = table.get("dependencies").and_then(|v| v.as_table()) {
            for (key, val) in deps {
                dependencies.insert(key.clone(), CrateDep::from_toml_value(val));
            }
        }

        let root = path.parent().unwrap().to_path_buf();
        Ok(Self { name, version, dependencies, root })
    }

    pub fn entry_file(&self) -> PathBuf {
        self.root.join("src").join("main.lisp")
    }

    pub fn bridge_dir(&self) -> PathBuf {
        self.root.join("bridge")
    }

    pub fn has_bridges(&self) -> bool {
        let dir = self.bridge_dir();
        dir.exists() && std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().any(|e| {
                e.path().extension().map(|ext| ext == "rs").unwrap_or(false)
            }))
            .unwrap_or(false)
    }

    /// List bridge module names (filename without .rs)
    pub fn bridge_modules(&self) -> Vec<String> {
        let dir = self.bridge_dir();
        if !dir.exists() {
            return Vec::new();
        }
        let mut modules = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        modules.push(stem.to_string());
                    }
                }
            }
        }
        modules.sort();
        modules
    }

    pub fn build_dir(&self) -> PathBuf {
        self.root.join(".lisprint").join("build")
    }

    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("[project]\n");
        out.push_str(&format!("name = \"{}\"\n", self.name));
        out.push_str(&format!("version = \"{}\"\n", self.version));

        if !self.dependencies.is_empty() {
            out.push_str("\n[dependencies]\n");
            for (name, dep) in &self.dependencies {
                if dep.features.is_empty() && dep.path.is_none() {
                    out.push_str(&format!("{} = \"{}\"\n", name, dep.version));
                } else {
                    // Use toml serialization for complex deps
                    let mut t = toml::Table::new();
                    t.insert(name.clone(), dep.to_toml_value());
                    let s = toml::to_string_pretty(&t).unwrap_or_default();
                    out.push_str(&s);
                }
            }
        }

        out
    }
}

pub fn new_project(name: &str) -> Result<(), String> {
    let dir = Path::new(name);
    if dir.exists() {
        return Err(format!("Directory '{}' already exists", name));
    }

    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("Failed to create directory: {}", e))?;

    let project = Project {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        dependencies: BTreeMap::new(),
        root: dir.to_path_buf(),
    };

    std::fs::write(dir.join("lisp.toml"), project.to_toml())
        .map_err(|e| format!("Failed to write lisp.toml: {}", e))?;

    let main_lisp = format!(
        r#";; {name}

(defun main ()
  (println "Hello from {name}!"))

(main)
"#
    );
    std::fs::write(dir.join("src").join("main.lisp"), main_lisp)
        .map_err(|e| format!("Failed to write src/main.lisp: {}", e))?;

    println!("Created project '{}'", name);
    println!("  {}/lisp.toml", name);
    println!("  {}/src/main.lisp", name);
    Ok(())
}

pub fn init_project() -> Result<(), String> {
    let toml_path = Path::new("lisp.toml");
    if toml_path.exists() {
        return Err("lisp.toml already exists in this directory".to_string());
    }

    let name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-project".to_string());

    std::fs::create_dir_all("src")
        .map_err(|e| format!("Failed to create src/: {}", e))?;

    let project = Project {
        name: name.clone(),
        version: "0.1.0".to_string(),
        dependencies: BTreeMap::new(),
        root: PathBuf::from("."),
    };

    std::fs::write(toml_path, project.to_toml())
        .map_err(|e| format!("Failed to write lisp.toml: {}", e))?;

    let main_path = Path::new("src/main.lisp");
    if !main_path.exists() {
        let main_lisp = format!(
            r#";; {name}

(defun main ()
  (println "Hello from {name}!"))

(main)
"#
        );
        std::fs::write(main_path, main_lisp)
            .map_err(|e| format!("Failed to write src/main.lisp: {}", e))?;
    }

    println!("Initialized project '{}' in current directory", name);
    Ok(())
}

pub fn add_dependency(pkg: &str) -> Result<(), String> {
    let toml_path = find_toml()?;
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("Failed to read lisp.toml: {}", e))?;
    let mut table: toml::Table = content
        .parse()
        .map_err(|e| format!("Failed to parse lisp.toml: {}", e))?;

    let deps = table
        .entry("dependencies")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or("dependencies is not a table")?;

    if deps.contains_key(pkg) {
        println!("'{}' is already in dependencies", pkg);
        return Ok(());
    }

    deps.insert(pkg.to_string(), toml::Value::String("*".to_string()));

    let out = toml::to_string_pretty(&table)
        .map_err(|e| format!("Failed to serialize lisp.toml: {}", e))?;
    std::fs::write(&toml_path, out)
        .map_err(|e| format!("Failed to write lisp.toml: {}", e))?;

    // Generate bridge template
    let toml_dir = toml_path.parent().unwrap();
    let bridge_dir = toml_dir.join("bridge");
    let safe_name = pkg.replace('-', "_");
    let bridge_file = bridge_dir.join(format!("{}.rs", safe_name));
    if !bridge_file.exists() {
        std::fs::create_dir_all(&bridge_dir)
            .map_err(|e| format!("Failed to create bridge/: {}", e))?;
        let template = format!(
r#"use lisprint_core::env::Env;
use lisprint_core::value::{{Value, LispResult, LispError, NativeFnData}};
use std::sync::Arc;

pub fn register(env: &mut Env) {{
    let name = "{pkg}/hello".to_string();
    env.define(
        "{pkg}/hello",
        Value::NativeFn(Arc::new(NativeFnData {{
            name,
            func: Box::new(|_args| {{
                // TODO: implement
                Ok(Value::str("hello from {pkg}"))
            }}),
        }})),
    );
}}
"#
        );
        std::fs::write(&bridge_file, template)
            .map_err(|e| format!("Failed to write bridge template: {}", e))?;
        println!("Added '{}' to dependencies", pkg);
        println!("  bridge/{}.rs created (edit to implement)", safe_name);
    } else {
        println!("Added '{}' to dependencies", pkg);
    }

    Ok(())
}

// Embedded lisprint-core source (generated by build.rs)
mod core_embed {
    include!(concat!(env!("OUT_DIR"), "/core_embed.rs"));
}

/// Extract embedded lisprint-core source to the build directory
fn extract_embedded_core(build_dir: &Path) -> Result<PathBuf, String> {
    let core_dir = build_dir.join("lisprint-core");
    // Always overwrite to ensure version consistency
    for (rel_path, content) in core_embed::CORE_FILES {
        let dest = core_dir.join(rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir: {}", e))?;
        }
        std::fs::write(&dest, content)
            .map_err(|e| format!("Failed to write {}: {}", rel_path, e))?;
    }

    // Extract prelude.lisp to the path expected by include_str!("../../../lib/prelude.lisp")
    // From .lisprint/build/lisprint-core/src/prelude.rs → ../../../lib/ = .lisprint/lib/
    let lisprint_dir = build_dir.parent()
        .ok_or("Invalid build directory")?;
    let prelude_dir = lisprint_dir.join("lib");
    std::fs::create_dir_all(&prelude_dir)
        .map_err(|e| format!("Failed to create lib dir: {}", e))?;
    std::fs::write(prelude_dir.join("prelude.lisp"), core_embed::PRELUDE_LISP)
        .map_err(|e| format!("Failed to write prelude.lisp: {}", e))?;

    Ok(core_dir)
}

/// Build the bridge Cargo project, returns path to compiled binary
pub fn build_bridge_project(proj: &Project) -> Result<PathBuf, String> {
    let build_dir = proj.build_dir();
    let src_dir = build_dir.join("src");
    let bridges_dir = src_dir.join("bridges");

    std::fs::create_dir_all(&bridges_dir)
        .map_err(|e| format!("Failed to create build dir: {}", e))?;

    // Extract embedded lisprint-core source
    let core_path = extract_embedded_core(&build_dir)?;

    // 1. Generate Cargo.toml
    let bridge_modules = proj.bridge_modules();
    generate_cargo_toml(proj, &core_path, &build_dir)?;

    // 2. Copy bridge files
    for module in &bridge_modules {
        let src = proj.bridge_dir().join(format!("{}.rs", module));
        let dst = bridges_dir.join(format!("{}.rs", module));
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("Failed to copy bridge {}: {}", module, e))?;
    }

    // 3. Generate bridges/mod.rs
    generate_bridges_mod(&bridge_modules, &bridges_dir)?;

    // 4. Generate main.rs
    let lisp_path = proj.entry_file().canonicalize()
        .map_err(|e| format!("Entry file not found: {}", e))?;
    generate_main_rs(&lisp_path, &src_dir)?;

    // 5. Write .gitignore
    let _ = std::fs::write(proj.root.join(".lisprint").join(".gitignore"), "*\n");

    // 6. cargo build
    eprintln!("Compiling bridges...");
    let status = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&build_dir)
        .status()
        .map_err(|e| format!("Failed to run cargo: {}", e))?;

    if !status.success() {
        return Err("Bridge compilation failed".to_string());
    }

    let binary = build_dir.join("target").join("release").join(&proj.name);
    if !binary.exists() {
        // Try the crate name with hyphens replaced
        let alt = build_dir.join("target").join("release").join(proj.name.replace('-', "_"));
        if alt.exists() {
            return Ok(alt);
        }
        return Err(format!("Binary not found at {}", binary.display()));
    }
    Ok(binary)
}

fn generate_cargo_toml(proj: &Project, core_path: &Path, build_dir: &Path) -> Result<(), String> {
    let mut cargo = String::new();
    cargo.push_str(&format!(
        r#"[package]
name = "{}"
version = "{}"
edition = "2021"

[[bin]]
name = "{}"
path = "src/main.rs"

[dependencies]
lisprint-core = {{ path = "{}" }}
"#,
        proj.name,
        proj.version,
        proj.name,
        core_path.display(),
    ));

    for (name, dep) in &proj.dependencies {
        cargo.push_str(&format!("{} = {}\n", name, dep.to_cargo_toml_value()));
    }

    std::fs::write(build_dir.join("Cargo.toml"), cargo)
        .map_err(|e| format!("Failed to write Cargo.toml: {}", e))
}

fn generate_bridges_mod(modules: &[String], bridges_dir: &Path) -> Result<(), String> {
    let mut content = String::new();
    for m in modules {
        content.push_str(&format!("pub mod {};\n", m));
    }
    content.push_str("\nuse lisprint_core::env::Env;\n\n");
    content.push_str("pub fn register_all(env: &mut Env) {\n");
    for m in modules {
        content.push_str(&format!("    {}::register(env);\n", m));
    }
    content.push_str("}\n");

    std::fs::write(bridges_dir.join("mod.rs"), content)
        .map_err(|e| format!("Failed to write bridges/mod.rs: {}", e))
}

fn generate_main_rs(lisp_path: &Path, src_dir: &Path) -> Result<(), String> {
    let content = format!(
        r#"use lisprint_core::builtins;
use lisprint_core::env::Env;
use lisprint_core::eval;
use lisprint_core::parser;
use lisprint_core::prelude;

mod bridges;

fn main() {{
    let mut env = Env::new();
    builtins::register(&mut env);
    prelude::load(&mut env).expect("failed to load prelude");
    bridges::register_all(&mut env);

    let source = std::fs::read_to_string("{lisp_path}")
        .expect("failed to read lisp source");

    match parser::parse(&source) {{
        Ok(exprs) => {{
            for expr in &exprs {{
                if let Err(e) = eval::eval(expr, &mut env) {{
                    eprintln!("Error: {{}}", e);
                    std::process::exit(1);
                }}
            }}
        }}
        Err(e) => {{
            eprintln!("Parse error: {{}}", e);
            std::process::exit(1);
        }}
    }}
}}
"#,
        lisp_path = lisp_path.display(),
    );

    std::fs::write(src_dir.join("main.rs"), content)
        .map_err(|e| format!("Failed to write main.rs: {}", e))
}

fn find_toml() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| format!("Cannot get cwd: {}", e))?;
    loop {
        let path = dir.join("lisp.toml");
        if path.exists() {
            return Ok(path);
        }
        if !dir.pop() {
            return Err("No lisp.toml found (run 'lisprint init' first)".to_string());
        }
    }
}
