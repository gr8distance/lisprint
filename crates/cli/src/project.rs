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

    // 1. Compile Lisp source with Cranelift (bridge mode) → .o
    let lisp_path = proj.entry_file().canonicalize()
        .map_err(|e| format!("Entry file not found: {}", e))?;
    let source = std::fs::read_to_string(&lisp_path)
        .map_err(|e| format!("Failed to read {}: {}", lisp_path.display(), e))?;
    let exprs = lisprint_core::parser::parse(&source)
        .map_err(|e| format!("Parse error: {}", e))?;

    eprintln!("Compiling Lisp source...");
    let mut compiler = lisprint_compiler::Compiler::new()
        .map_err(|e| format!("Compiler init error: {}", e))?;
    compiler.set_bridge_mode()
        .map_err(|e| format!("Failed to set bridge mode: {}", e))?;
    let obj_bytes = compiler.compile_exprs(&exprs)
        .map_err(|e| format!("Compile error: {}", e))?;

    // 2. Write .o file and create static library (.a)
    let obj_path = build_dir.join("lspcode.o");
    std::fs::write(&obj_path, &obj_bytes)
        .map_err(|e| format!("Failed to write object file: {}", e))?;

    let ar_status = std::process::Command::new("ar")
        .args(["rcs", "liblspcode.a", "lspcode.o"])
        .current_dir(&build_dir)
        .status()
        .map_err(|e| format!("Failed to run ar: {}", e))?;
    if !ar_status.success() {
        return Err("Failed to create static library".to_string());
    }

    // 3. Extract embedded lisprint-core source
    let core_path = extract_embedded_core(&build_dir)?;

    // 4. Generate Cargo.toml
    let bridge_modules = proj.bridge_modules();
    generate_cargo_toml(proj, &core_path, &build_dir)?;

    // 5. Copy bridge files
    for module in &bridge_modules {
        let src = proj.bridge_dir().join(format!("{}.rs", module));
        let dst = bridges_dir.join(format!("{}.rs", module));
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("Failed to copy bridge {}: {}", module, e))?;
    }

    // 6. Generate bridges/mod.rs
    generate_bridges_mod(&bridge_modules, &bridges_dir)?;

    // 7. Generate build.rs (link the compiled .o static lib)
    generate_build_rs_bridge(&build_dir)?;

    // 8. Generate main.rs (runtime + bridge dispatch + extern _lsp_main)
    generate_main_rs_compiled(&src_dir)?;

    // 9. Write .gitignore
    let _ = std::fs::write(proj.root.join(".lisprint").join(".gitignore"), "*\n");

    // 10. cargo build
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

fn generate_build_rs_bridge(build_dir: &Path) -> Result<(), String> {
    let content = r#"fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search=native={}", manifest_dir);
    println!("cargo:rustc-link-lib=static=lspcode");
}
"#;
    std::fs::write(build_dir.join("build.rs"), content)
        .map_err(|e| format!("Failed to write build.rs: {}", e))
}

fn generate_main_rs_compiled(src_dir: &Path) -> Result<(), String> {
    let content = r##"#![allow(private_interfaces)]

use lisprint_core::builtins;
use lisprint_core::env::Env;
use lisprint_core::prelude;
use lisprint_core::value::Value;

mod bridges;

// === Runtime value tags ===
const TAG_NIL: i64 = 0;
const TAG_BOOL: i64 = 1;
const TAG_INT: i64 = 2;
const TAG_FLOAT: i64 = 3;
const TAG_STR: i64 = 4;
#[allow(dead_code)]
const TAG_FN: i64 = 5;
const TAG_LIST: i64 = 6;

// === FFI types ===
#[repr(C)]
pub struct TaggedValue {
    tag: i64,
    payload: i64,
}

#[repr(C)]
struct RuntimeList {
    len: usize,
    data: *const (i64, i64),
}

// === String helpers ===
fn read_str(payload: i64) -> &'static str {
    let ptr = payload as *const u8;
    if ptr.is_null() { return ""; }
    unsafe {
        let mut len = 0;
        while *ptr.add(len) != 0 { len += 1; }
        let slice = std::slice::from_raw_parts(ptr, len);
        std::str::from_utf8_unchecked(slice)
    }
}

fn leak_string(s: String) -> *const u8 {
    let mut bytes = s.into_bytes();
    bytes.push(0);
    let ptr = bytes.as_ptr();
    std::mem::forget(bytes);
    ptr
}

// === List helpers ===
fn make_list(items: Vec<(i64, i64)>) -> TaggedValue {
    let len = items.len();
    let boxed = items.into_boxed_slice();
    let data = Box::into_raw(boxed) as *const (i64, i64);
    let rl = Box::new(RuntimeList { len, data });
    let ptr = Box::into_raw(rl);
    TaggedValue { tag: TAG_LIST, payload: ptr as i64 }
}

fn get_list(payload: i64) -> &'static [(i64, i64)] {
    let ptr = payload as *const RuntimeList;
    if ptr.is_null() { return &[]; }
    unsafe {
        let rl = &*ptr;
        std::slice::from_raw_parts(rl.data, rl.len)
    }
}

// === Runtime functions (linked with compiled .o) ===

#[no_mangle]
pub extern "C" fn lsp_println(tag: i64, payload: i64) {
    lsp_print(tag, payload);
    println!();
}

#[no_mangle]
pub extern "C" fn lsp_print(tag: i64, payload: i64) {
    match tag {
        TAG_NIL => print!("nil"),
        TAG_BOOL => print!("{}", if payload != 0 { "true" } else { "false" }),
        TAG_INT => print!("{}", payload),
        TAG_FLOAT => {
            let f = f64::from_bits(payload as u64);
            print!("{}", f);
        }
        TAG_STR => print!("{}", read_str(payload)),
        TAG_LIST => {
            let items = get_list(payload);
            print!("(");
            for (i, (t, p)) in items.iter().enumerate() {
                if i > 0 { print!(" "); }
                lsp_print(*t, *p);
            }
            print!(")");
        }
        _ => print!("<unknown:{}>", tag),
    }
}

#[no_mangle]
pub extern "C" fn lsp_str_concat(_tag1: i64, payload1: i64, _tag2: i64, payload2: i64) -> TaggedValue {
    let s1 = read_str(payload1);
    let s2 = read_str(payload2);
    let result = format!("{}{}", s1, s2);
    let ptr = leak_string(result);
    TaggedValue { tag: TAG_STR, payload: ptr as i64 }
}

#[no_mangle]
pub extern "C" fn lsp_to_string(tag: i64, payload: i64) -> TaggedValue {
    let s = match tag {
        TAG_NIL => "nil".to_string(),
        TAG_BOOL => if payload != 0 { "true" } else { "false" }.to_string(),
        TAG_INT => payload.to_string(),
        TAG_FLOAT => f64::from_bits(payload as u64).to_string(),
        TAG_STR => return TaggedValue { tag: TAG_STR, payload },
        TAG_LIST => {
            let items = get_list(payload);
            let parts: Vec<String> = items.iter().map(|(t, p)| {
                let tv = lsp_to_string(*t, *p);
                read_str(tv.payload).to_string()
            }).collect();
            format!("({})", parts.join(" "))
        }
        _ => format!("<unknown:{}>", tag),
    };
    let ptr = leak_string(s);
    TaggedValue { tag: TAG_STR, payload: ptr as i64 }
}

// === Data structure operations ===

#[no_mangle]
pub extern "C" fn lsp_cons(elem_tag: i64, elem_payload: i64, list_tag: i64, list_payload: i64) -> TaggedValue {
    let mut items = vec![(elem_tag, elem_payload)];
    if list_tag == TAG_LIST {
        items.extend_from_slice(get_list(list_payload));
    } else if list_tag != TAG_NIL {
        items.push((list_tag, list_payload));
    }
    make_list(items)
}

#[no_mangle]
pub extern "C" fn lsp_first(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    if coll_tag == TAG_LIST {
        let items = get_list(coll_payload);
        if let Some(&(tag, payload)) = items.first() {
            return TaggedValue { tag, payload };
        }
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

#[no_mangle]
pub extern "C" fn lsp_rest(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    if coll_tag == TAG_LIST {
        let items = get_list(coll_payload);
        if items.len() > 1 {
            return make_list(items[1..].to_vec());
        }
    }
    make_list(vec![])
}

#[no_mangle]
pub extern "C" fn lsp_nth(coll_tag: i64, coll_payload: i64, _idx_tag: i64, idx_payload: i64) -> TaggedValue {
    if coll_tag == TAG_LIST {
        let items = get_list(coll_payload);
        let idx = idx_payload as usize;
        if idx < items.len() {
            let (tag, payload) = items[idx];
            return TaggedValue { tag, payload };
        }
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

#[no_mangle]
pub extern "C" fn lsp_count(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    let len = if coll_tag == TAG_LIST {
        get_list(coll_payload).len()
    } else if coll_tag == TAG_NIL {
        0
    } else if coll_tag == TAG_STR {
        read_str(coll_payload).len()
    } else {
        0
    };
    TaggedValue { tag: TAG_INT, payload: len as i64 }
}

#[no_mangle]
pub extern "C" fn lsp_empty_q(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    let empty = if coll_tag == TAG_LIST {
        get_list(coll_payload).is_empty()
    } else if coll_tag == TAG_NIL {
        true
    } else {
        false
    };
    TaggedValue { tag: TAG_BOOL, payload: if empty { 1 } else { 0 } }
}

#[no_mangle]
pub extern "C" fn lsp_concat(a_tag: i64, a_payload: i64, b_tag: i64, b_payload: i64) -> TaggedValue {
    let mut items = Vec::new();
    if a_tag == TAG_LIST {
        items.extend_from_slice(get_list(a_payload));
    } else if a_tag != TAG_NIL {
        items.push((a_tag, a_payload));
    }
    if b_tag == TAG_LIST {
        items.extend_from_slice(get_list(b_payload));
    } else if b_tag != TAG_NIL {
        items.push((b_tag, b_payload));
    }
    make_list(items)
}

// === Bridge dispatch ===

static mut BRIDGE_ENV: *mut Env = std::ptr::null_mut();

fn tagged_to_value(tag: i64, payload: i64) -> Value {
    match tag {
        TAG_NIL => Value::Nil,
        TAG_BOOL => Value::Bool(payload != 0),
        TAG_INT => Value::Int(payload),
        TAG_FLOAT => Value::Float(f64::from_bits(payload as u64)),
        TAG_STR => Value::str(read_str(payload).to_string()),
        TAG_LIST => {
            let items = get_list(payload);
            let values: Vec<Value> = items.iter()
                .map(|(t, p)| tagged_to_value(*t, *p))
                .collect();
            Value::list(values)
        }
        _ => Value::Nil,
    }
}

fn value_to_tagged(val: &Value) -> TaggedValue {
    match val {
        Value::Nil => TaggedValue { tag: TAG_NIL, payload: 0 },
        Value::Bool(b) => TaggedValue { tag: TAG_BOOL, payload: if *b { 1 } else { 0 } },
        Value::Int(n) => TaggedValue { tag: TAG_INT, payload: *n },
        Value::Float(f) => TaggedValue { tag: TAG_FLOAT, payload: f.to_bits() as i64 },
        Value::Str(s) => {
            let ptr = leak_string(s.to_string());
            TaggedValue { tag: TAG_STR, payload: ptr as i64 }
        }
        Value::List(items) => {
            let pairs: Vec<(i64, i64)> = items.iter()
                .map(|v| {
                    let tv = value_to_tagged(v);
                    (tv.tag, tv.payload)
                })
                .collect();
            make_list(pairs)
        }
        _ => TaggedValue { tag: TAG_NIL, payload: 0 },
    }
}

#[no_mangle]
pub extern "C" fn lsp_call_bridge(name_ptr: *const u8, argc: i64, argv: *const i64) -> TaggedValue {
    let name = unsafe {
        let mut len = 0;
        while *name_ptr.add(len) != 0 { len += 1; }
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, len))
    };

    let argc = argc as usize;
    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        let tag = unsafe { *argv.add(i * 2) };
        let payload = unsafe { *argv.add(i * 2 + 1) };
        args.push(tagged_to_value(tag, payload));
    }

    let env = unsafe { &*BRIDGE_ENV };
    match env.get(name) {
        Ok(func_val) => {
            match &func_val {
                Value::NativeFn(nf) => {
                    match (nf.func)(&args) {
                        Ok(result) => value_to_tagged(&result),
                        Err(e) => {
                            eprintln!("Bridge error in {}: {}", name, e);
                            TaggedValue { tag: TAG_NIL, payload: 0 }
                        }
                    }
                }
                _ => {
                    eprintln!("Bridge {} is not a function", name);
                    TaggedValue { tag: TAG_NIL, payload: 0 }
                }
            }
        }
        Err(_) => {
            eprintln!("Bridge function not found: {}", name);
            TaggedValue { tag: TAG_NIL, payload: 0 }
        }
    }
}

// === Entry point ===

#[repr(C)]
struct LspResult {
    tag: i64,
    payload: i64,
}

extern "C" {
    fn _lsp_main() -> LspResult;
}

fn main() {
    // Initialize bridge environment with builtins + prelude + bridge functions
    let mut env = Env::new();
    builtins::register(&mut env);
    prelude::load(&mut env).expect("failed to load prelude");
    bridges::register_all(&mut env);
    unsafe { BRIDGE_ENV = Box::into_raw(Box::new(env)); }

    // Run compiled Lisp code
    unsafe { _lsp_main(); }
}
"##;

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
