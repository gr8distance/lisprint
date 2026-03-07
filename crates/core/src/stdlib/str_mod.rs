use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "str/upper", str_upper);
    reg(env, "str/lower", str_lower);
    reg(env, "str/trim", str_trim);
    reg(env, "str/split", str_split);
    reg(env, "str/join", str_join);
    reg(env, "str/contains?", str_contains);
    reg(env, "str/starts-with?", str_starts_with);
    reg(env, "str/ends-with?", str_ends_with);
    reg(env, "str/replace", str_replace);
    reg(env, "str/len", str_len);
    reg(env, "str/substr", str_substr);
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

fn str_upper(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("str/upper requires 1 argument")); }
    Ok(Value::str(args[0].as_str()?.to_uppercase()))
}

fn str_lower(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("str/lower requires 1 argument")); }
    Ok(Value::str(args[0].as_str()?.to_lowercase()))
}

fn str_trim(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("str/trim requires 1 argument")); }
    Ok(Value::str(args[0].as_str()?.trim()))
}

fn str_split(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("str/split requires 2 arguments")); }
    let s = args[0].as_str()?;
    let sep = args[1].as_str()?;
    let parts: Vec<Value> = s.split(sep).map(|p| Value::str(p)).collect();
    Ok(Value::list(parts))
}

fn str_join(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("str/join requires 2 arguments")); }
    let sep = args[0].as_str()?;
    let items = args[1].as_list()?;
    let strs: Result<Vec<String>, _> = items.iter().map(|v| {
        match v {
            Value::Str(s) => Ok(s.to_string()),
            other => Ok(format!("{}", other)),
        }
    }).collect();
    Ok(Value::str(strs?.join(sep)))
}

fn str_contains(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("str/contains? requires 2 arguments")); }
    Ok(Value::Bool(args[0].as_str()?.contains(args[1].as_str()?)))
}

fn str_starts_with(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("str/starts-with? requires 2 arguments")); }
    Ok(Value::Bool(args[0].as_str()?.starts_with(args[1].as_str()?)))
}

fn str_ends_with(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("str/ends-with? requires 2 arguments")); }
    Ok(Value::Bool(args[0].as_str()?.ends_with(args[1].as_str()?)))
}

fn str_replace(args: &[Value]) -> LispResult {
    if args.len() != 3 { return Err(LispError::new("str/replace requires 3 arguments")); }
    Ok(Value::str(args[0].as_str()?.replace(args[1].as_str()?, args[2].as_str()?)))
}

fn str_len(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("str/len requires 1 argument")); }
    Ok(Value::Int(args[0].as_str()?.len() as i64))
}

fn str_substr(args: &[Value]) -> LispResult {
    if args.len() < 2 || args.len() > 3 {
        return Err(LispError::new("str/substr requires 2-3 arguments"));
    }
    let s = args[0].as_str()?;
    let start = args[1].as_int()? as usize;
    let end = if args.len() == 3 {
        args[2].as_int()? as usize
    } else {
        s.len()
    };
    if start > s.len() || end > s.len() || start > end {
        return Err(LispError::new("str/substr: index out of bounds"));
    }
    Ok(Value::str(&s[start..end]))
}
