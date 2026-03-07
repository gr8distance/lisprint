use std::collections::HashMap;
use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "json/parse", json_parse);
    reg(env, "json/encode", json_encode);
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

fn json_parse(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("json/parse requires 1 argument")); }
    let s = args[0].as_str()?;
    let v: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| LispError::new(format!("json/parse: {}", e)))?;
    Ok(json_to_value(&v))
}

fn json_encode(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("json/encode requires 1 argument")); }
    let json = value_to_json(&args[0])?;
    Ok(Value::str(json.to_string()))
}

fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::str(s.as_str()),
        serde_json::Value::Array(arr) => {
            Value::list(arr.iter().map(json_to_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut map = HashMap::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_value(v));
            }
            Value::Map(Arc::new(map))
        }
    }
}

fn value_to_json(val: &Value) -> Result<serde_json::Value, LispError> {
    match val {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
        Value::Int(n) => Ok(serde_json::Value::Number((*n).into())),
        Value::Float(n) => {
            serde_json::Number::from_f64(*n)
                .map(serde_json::Value::Number)
                .ok_or_else(|| LispError::new("json/encode: invalid float"))
        }
        Value::Str(s) => Ok(serde_json::Value::String(s.to_string())),
        Value::List(items) | Value::Vec(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_json).collect();
            Ok(serde_json::Value::Array(arr?))
        }
        Value::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(k.clone(), value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        Value::Keyword(k) => Ok(serde_json::Value::String(k.to_string())),
        _ => Err(LispError::new(format!("json/encode: cannot encode {}", val.type_name()))),
    }
}
