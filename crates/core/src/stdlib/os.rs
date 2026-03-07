use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "os/exec", os_exec);
    reg(env, "os/exit", os_exit);
    reg(env, "os/pid", os_pid);
    reg(env, "os/arch", os_arch);
    reg(env, "os/name", os_name);

    // env サブモジュール
    reg(env, "env/get", env_get);
    reg(env, "env/set", env_set);
    reg(env, "env/all", env_all);

    // dir サブモジュール
    reg(env, "dir/list", dir_list);
    reg(env, "dir/create", dir_create);
    reg(env, "dir/remove", dir_remove);
    reg(env, "dir/cwd", dir_cwd);

    // path サブモジュール
    reg(env, "path/join", path_join);
    reg(env, "path/basename", path_basename);
    reg(env, "path/dirname", path_dirname);
    reg(env, "path/ext", path_ext);
    reg(env, "path/absolute", path_absolute);
}

fn reg(env: &mut Env, name: &str, func: fn(&[Value]) -> LispResult) {
    let n = name.to_string();
    env.define(
        name,
        Value::NativeFn(Arc::new(NativeFnData {
            name: n,
            func: Box::new(move |args| func(args)),
        })),
    );
}

// --- os ---

fn os_exec(args: &[Value]) -> LispResult {
    if args.is_empty() { return Err(LispError::new("os/exec requires at least 1 argument")); }
    let cmd = args[0].as_str()?;
    let cmd_args: Result<Vec<String>, _> = args[1..].iter().map(|a| {
        match a {
            Value::Str(s) => Ok(s.to_string()),
            other => Ok(format!("{}", other)),
        }
    }).collect();
    let output = std::process::Command::new(cmd)
        .args(cmd_args?)
        .output()
        .map_err(|e| LispError::new(format!("os/exec: {}", e)))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(Value::str(stdout))
}

fn os_exit(args: &[Value]) -> LispResult {
    let code = if args.is_empty() { 0 } else { args[0].as_int()? as i32 };
    std::process::exit(code);
}

fn os_pid(_args: &[Value]) -> LispResult {
    Ok(Value::Int(std::process::id() as i64))
}

fn os_arch(_args: &[Value]) -> LispResult {
    Ok(Value::str(std::env::consts::ARCH))
}

fn os_name(_args: &[Value]) -> LispResult {
    Ok(Value::str(std::env::consts::OS))
}

// --- env ---

fn env_get(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("env/get requires 1 argument")); }
    let key = args[0].as_str()?;
    match std::env::var(key) {
        Ok(val) => Ok(Value::str(val)),
        Err(_) => Ok(Value::Nil),
    }
}

fn env_set(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("env/set requires 2 arguments")); }
    let key = args[0].as_str()?;
    let val = args[1].as_str()?;
    std::env::set_var(key, val);
    Ok(Value::Nil)
}

fn env_all(_args: &[Value]) -> LispResult {
    let mut map = std::collections::HashMap::new();
    for (key, val) in std::env::vars() {
        map.insert(key, Value::str(val));
    }
    Ok(Value::Map(Arc::new(map)))
}

// --- dir ---

fn dir_list(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("dir/list requires 1 argument")); }
    let path = args[0].as_str()?;
    let entries = std::fs::read_dir(path)
        .map_err(|e| LispError::new(format!("dir/list: {}", e)))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| LispError::new(format!("dir/list: {}", e)))?;
        files.push(Value::str(entry.file_name().to_string_lossy().to_string()));
    }
    Ok(Value::list(files))
}

fn dir_create(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("dir/create requires 1 argument")); }
    let path = args[0].as_str()?;
    std::fs::create_dir_all(path)
        .map(|_| Value::Nil)
        .map_err(|e| LispError::new(format!("dir/create: {}", e)))
}

fn dir_remove(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("dir/remove requires 1 argument")); }
    let path = args[0].as_str()?;
    std::fs::remove_dir_all(path)
        .map(|_| Value::Nil)
        .map_err(|e| LispError::new(format!("dir/remove: {}", e)))
}

fn dir_cwd(_args: &[Value]) -> LispResult {
    std::env::current_dir()
        .map(|p| Value::str(p.to_string_lossy().to_string()))
        .map_err(|e| LispError::new(format!("dir/cwd: {}", e)))
}

// --- path ---

fn path_join(args: &[Value]) -> LispResult {
    if args.len() < 2 { return Err(LispError::new("path/join requires at least 2 arguments")); }
    let mut path = std::path::PathBuf::from(args[0].as_str()?);
    for arg in &args[1..] {
        path.push(arg.as_str()?);
    }
    Ok(Value::str(path.to_string_lossy().to_string()))
}

fn path_basename(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("path/basename requires 1 argument")); }
    let path = std::path::Path::new(args[0].as_str()?);
    Ok(Value::str(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
}

fn path_dirname(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("path/dirname requires 1 argument")); }
    let path = std::path::Path::new(args[0].as_str()?);
    Ok(Value::str(path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()))
}

fn path_ext(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("path/ext requires 1 argument")); }
    let path = std::path::Path::new(args[0].as_str()?);
    Ok(match path.extension() {
        Some(ext) => Value::str(ext.to_string_lossy().to_string()),
        None => Value::Nil,
    })
}

fn path_absolute(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("path/absolute requires 1 argument")); }
    let path = args[0].as_str()?;
    std::fs::canonicalize(path)
        .map(|p| Value::str(p.to_string_lossy().to_string()))
        .map_err(|e| LispError::new(format!("path/absolute: {}", e)))
}
