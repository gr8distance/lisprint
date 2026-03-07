use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "re/match?", re_match);
    reg(env, "re/find", re_find);
    reg(env, "re/find-all", re_find_all);
    reg(env, "re/replace", re_replace);
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

fn re_match(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("re/match? requires 2 arguments")); }
    let pattern = args[0].as_str()?;
    let text = args[1].as_str()?;
    let re = regex::Regex::new(pattern)
        .map_err(|e| LispError::new(format!("re/match?: {}", e)))?;
    Ok(Value::Bool(re.is_match(text)))
}

fn re_find(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("re/find requires 2 arguments")); }
    let pattern = args[0].as_str()?;
    let text = args[1].as_str()?;
    let re = regex::Regex::new(pattern)
        .map_err(|e| LispError::new(format!("re/find: {}", e)))?;
    match re.find(text) {
        Some(m) => Ok(Value::str(m.as_str())),
        None => Ok(Value::Nil),
    }
}

fn re_find_all(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("re/find-all requires 2 arguments")); }
    let pattern = args[0].as_str()?;
    let text = args[1].as_str()?;
    let re = regex::Regex::new(pattern)
        .map_err(|e| LispError::new(format!("re/find-all: {}", e)))?;
    let matches: Vec<Value> = re.find_iter(text).map(|m| Value::str(m.as_str())).collect();
    Ok(Value::list(matches))
}

fn re_replace(args: &[Value]) -> LispResult {
    if args.len() != 3 { return Err(LispError::new("re/replace requires 3 arguments")); }
    let pattern = args[0].as_str()?;
    let replacement = args[1].as_str()?;
    let text = args[2].as_str()?;
    let re = regex::Regex::new(pattern)
        .map_err(|e| LispError::new(format!("re/replace: {}", e)))?;
    Ok(Value::str(re.replace_all(text, replacement).to_string()))
}
