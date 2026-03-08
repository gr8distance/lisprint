use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Project {
    pub name: String,
    pub version: String,
    pub dependencies: BTreeMap<String, String>,
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
                let version_str = match val {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("*")
                        .to_string(),
                    _ => "*".to_string(),
                };
                dependencies.insert(key.clone(), version_str);
            }
        }

        let root = path.parent().unwrap().to_path_buf();
        Ok(Self {
            name,
            version,
            dependencies,
            root,
        })
    }

    pub fn entry_file(&self) -> PathBuf {
        self.root.join("src").join("main.lisp")
    }

    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("[project]\n");
        out.push_str(&format!("name = \"{}\"\n", self.name));
        out.push_str(&format!("version = \"{}\"\n", self.version));

        if !self.dependencies.is_empty() {
            out.push_str("\n[dependencies]\n");
            for (name, version) in &self.dependencies {
                out.push_str(&format!("{} = \"{}\"\n", name, version));
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

    println!("Added '{}' to dependencies", pkg);
    Ok(())
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
