use std::collections::HashMap;
use std::sync::Arc;

use crate::env::Env;
use crate::eval::eval;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    let name = "server/start".to_string();
    env.define(
        "server/start",
        Value::NativeFn(Arc::new(NativeFnData {
            name,
            func: Box::new(|args| server_start(args)),
        })),
    );
}

/// Build a request map from a tiny_http::Request
fn request_to_value(req: &mut tiny_http::Request) -> Value {
    let mut map = HashMap::new();
    map.insert("method".to_string(), Value::str(req.method().as_str()));
    map.insert("url".to_string(), Value::str(req.url()));

    let mut headers_map = HashMap::new();
    for header in req.headers() {
        headers_map.insert(
            header.field.as_str().as_str().to_lowercase(),
            Value::str(header.value.as_str()),
        );
    }
    map.insert("headers".to_string(), Value::Map(Arc::new(headers_map)));

    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    map.insert("body".to_string(), Value::str(body));

    Value::Map(Arc::new(map))
}

/// Call a Lisp function with one argument
fn call_handler(handler: &Value, arg: Value) -> LispResult {
    match handler {
        Value::Fn(f) => {
            let mut fn_env = Env::with_parent(Arc::new(f.env.clone()));
            if let Some(param) = f.params.first() {
                fn_env.define(param, arg);
            }
            let mut result = Value::Nil;
            for expr in &f.body {
                result = eval(expr, &mut fn_env)?;
            }
            Ok(result)
        }
        Value::NativeFn(f) => (f.func)(&[arg]),
        _ => Err(LispError::new("server/start: handler must be a function")),
    }
}

/// Start a blocking HTTP server
/// (server/start port handler-fn)
/// handler-fn receives a request map {:method :url :headers :body}
/// and should return a response map {:status 200 :body "..." :content-type "text/plain"}
/// or a string (treated as 200 text/plain)
fn server_start(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("server/start requires 2 arguments (port handler-fn)"));
    }
    let port = args[0].as_int()? as u16;
    let handler = args[1].clone();

    let addr = format!("0.0.0.0:{}", port);
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| LispError::new(format!("server/start: {}", e)))?;

    eprintln!("lisprint server listening on http://{}", addr);

    for mut request in server.incoming_requests() {
        let req_value = request_to_value(&mut request);

        let response_value = match call_handler(&handler, req_value) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("handler error: {}", e.message);
                let mut map = HashMap::new();
                map.insert("status".to_string(), Value::Int(500));
                map.insert("body".to_string(), Value::str(format!("Internal Server Error: {}", e.message)));
                Value::Map(Arc::new(map))
            }
        };

        // Parse response
        let (status, body, content_type) = match &response_value {
            Value::Map(map) => {
                let status = map.get("status")
                    .and_then(|v| v.as_int().ok())
                    .unwrap_or(200) as i32;
                let body = map.get("body")
                    .and_then(|v| v.as_str().ok())
                    .unwrap_or("")
                    .to_string();
                let ct = map.get("content-type")
                    .and_then(|v| v.as_str().ok())
                    .unwrap_or("text/plain")
                    .to_string();
                (status, body, ct)
            }
            Value::Str(s) => (200, s.to_string(), "text/plain".to_string()),
            _ => (200, response_value.to_string(), "text/plain".to_string()),
        };

        let header = tiny_http::Header::from_bytes(
            b"Content-Type",
            content_type.as_bytes(),
        ).unwrap();

        let response = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(header);

        let _ = request.respond(response);
    }

    Ok(Value::Nil)
}
