use std::collections::HashMap;
use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "http/get", http_get);
    reg(env, "http/post", http_post);
    reg(env, "http/put", http_put);
    reg(env, "http/delete", http_delete);
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

fn build_response(resp: reqwest::blocking::Response) -> LispResult {
    let status = resp.status().as_u16() as i64;
    let mut headers_map = HashMap::new();
    for (k, v) in resp.headers() {
        if let Ok(val) = v.to_str() {
            headers_map.insert(k.as_str().to_string(), Value::str(val));
        }
    }
    let body = resp.text()
        .map_err(|e| LispError::new(format!("http: failed to read body: {}", e)))?;

    let mut map = HashMap::new();
    map.insert("status".to_string(), Value::Int(status));
    map.insert("body".to_string(), Value::str(body));
    map.insert("headers".to_string(), Value::Map(Arc::new(headers_map)));
    Ok(Value::Map(Arc::new(map)))
}

fn parse_options(args: &[Value]) -> Result<(HashMap<String, String>, Option<String>), LispError> {
    let mut headers = HashMap::new();
    let mut body = None;
    if let Some(opts) = args.first() {
        if let Value::Map(map) = opts {
            if let Some(Value::Map(h)) = map.get("headers") {
                for (k, v) in h.iter() {
                    headers.insert(k.clone(), v.as_str()?.to_string());
                }
            }
            if let Some(b) = map.get("body") {
                body = Some(b.as_str()?.to_string());
            }
        }
    }
    Ok((headers, body))
}

fn http_get(args: &[Value]) -> LispResult {
    if args.is_empty() { return Err(LispError::new("http/get requires at least 1 argument (url)")); }
    let url = args[0].as_str()?;
    let (headers, _) = parse_options(&args[1..])?;

    let client = reqwest::blocking::Client::new();
    let mut req = client.get(url);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send()
        .map_err(|e| LispError::new(format!("http/get: {}", e)))?;
    build_response(resp)
}

fn http_post(args: &[Value]) -> LispResult {
    if args.is_empty() { return Err(LispError::new("http/post requires at least 1 argument (url)")); }
    let url = args[0].as_str()?;
    let (headers, body) = parse_options(&args[1..])?;

    let client = reqwest::blocking::Client::new();
    let mut req = client.post(url);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(b) = body {
        req = req.body(b);
    }
    let resp = req.send()
        .map_err(|e| LispError::new(format!("http/post: {}", e)))?;
    build_response(resp)
}

fn http_put(args: &[Value]) -> LispResult {
    if args.is_empty() { return Err(LispError::new("http/put requires at least 1 argument (url)")); }
    let url = args[0].as_str()?;
    let (headers, body) = parse_options(&args[1..])?;

    let client = reqwest::blocking::Client::new();
    let mut req = client.put(url);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(b) = body {
        req = req.body(b);
    }
    let resp = req.send()
        .map_err(|e| LispError::new(format!("http/put: {}", e)))?;
    build_response(resp)
}

fn http_delete(args: &[Value]) -> LispResult {
    if args.is_empty() { return Err(LispError::new("http/delete requires at least 1 argument (url)")); }
    let url = args[0].as_str()?;
    let (headers, _) = parse_options(&args[1..])?;

    let client = reqwest::blocking::Client::new();
    let mut req = client.delete(url);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send()
        .map_err(|e| LispError::new(format!("http/delete: {}", e)))?;
    build_response(resp)
}
