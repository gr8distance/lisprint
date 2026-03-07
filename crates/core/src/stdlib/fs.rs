use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "fs/read", fs_read);
    reg(env, "fs/write", fs_write);
    reg(env, "fs/append", fs_append);
    reg(env, "fs/exists?", fs_exists);
    reg(env, "fs/delete", fs_delete);
    reg(env, "fs/copy", fs_copy);
    reg(env, "fs/rename", fs_rename);
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

fn fs_read(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("fs/read requires 1 argument")); }
    let path = args[0].as_str()?;
    std::fs::read_to_string(path)
        .map(Value::str)
        .map_err(|e| LispError::new(format!("fs/read: {}", e)))
}

fn fs_write(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("fs/write requires 2 arguments")); }
    let path = args[0].as_str()?;
    let content = args[1].as_str()?;
    std::fs::write(path, content)
        .map(|_| Value::Nil)
        .map_err(|e| LispError::new(format!("fs/write: {}", e)))
}

fn fs_append(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("fs/append requires 2 arguments")); }
    let path = args[0].as_str()?;
    let content = args[1].as_str()?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| LispError::new(format!("fs/append: {}", e)))?;
    file.write_all(content.as_bytes())
        .map(|_| Value::Nil)
        .map_err(|e| LispError::new(format!("fs/append: {}", e)))
}

fn fs_exists(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("fs/exists? requires 1 argument")); }
    let path = args[0].as_str()?;
    Ok(Value::Bool(std::path::Path::new(path).exists()))
}

fn fs_delete(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("fs/delete requires 1 argument")); }
    let path = args[0].as_str()?;
    let p = std::path::Path::new(path);
    if p.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map(|_| Value::Nil)
    .map_err(|e| LispError::new(format!("fs/delete: {}", e)))
}

fn fs_copy(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("fs/copy requires 2 arguments")); }
    let src = args[0].as_str()?;
    let dst = args[1].as_str()?;
    std::fs::copy(src, dst)
        .map(|_| Value::Nil)
        .map_err(|e| LispError::new(format!("fs/copy: {}", e)))
}

fn fs_rename(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("fs/rename requires 2 arguments")); }
    let src = args[0].as_str()?;
    let dst = args[1].as_str()?;
    std::fs::rename(src, dst)
        .map(|_| Value::Nil)
        .map_err(|e| LispError::new(format!("fs/rename: {}", e)))
}
