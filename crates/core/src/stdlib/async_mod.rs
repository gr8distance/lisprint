use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::env::Env;
use crate::eval::eval;
use crate::value::{LispError, LispResult, NativeFnData, Value};

// Global handle registries with type-safe storage
static NEXT_HANDLE_ID: AtomicU64 = AtomicU64::new(1);

enum Handle {
    Thread(std::thread::JoinHandle<LispResult>),
    Sender(std::sync::mpsc::Sender<Value>),
    Receiver(std::sync::mpsc::Receiver<Value>),
}

fn registry() -> &'static Mutex<HashMap<u64, Handle>> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Mutex<HashMap<u64, Handle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn store_handle(handle: Handle) -> u64 {
    let id = NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
    registry().lock().unwrap().insert(id, handle);
    id
}

fn store_handle_with_id(id: u64, handle: Handle) {
    registry().lock().unwrap().insert(id, handle);
}

fn take_handle(id: u64) -> Result<Handle, LispError> {
    registry()
        .lock()
        .unwrap()
        .remove(&id)
        .ok_or_else(|| LispError::new("invalid or already consumed handle"))
}

pub fn register(env: &mut Env) {
    reg(env, "async/sleep", async_sleep);
    reg(env, "async/spawn", async_spawn);
    reg(env, "async/await", async_await);
    reg(env, "async/channel", async_channel);
    reg(env, "async/send", async_send);
    reg(env, "async/recv", async_recv);
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

/// (async/sleep ms)
fn async_sleep(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/sleep requires 1 argument (milliseconds)"));
    }
    let ms = args[0].as_int()? as u64;
    std::thread::sleep(std::time::Duration::from_millis(ms));
    Ok(Value::Nil)
}

/// Call a Lisp function with no arguments
fn call_fn(f: &Value) -> LispResult {
    match f {
        Value::Fn(lisp_fn) => {
            let mut fn_env = Env::with_parent(Arc::new(lisp_fn.env.clone()));
            let mut result = Value::Nil;
            for expr in &lisp_fn.body {
                result = eval(expr, &mut fn_env)?;
            }
            Ok(result)
        }
        Value::NativeFn(native) => (native.func)(&[]),
        _ => Err(LispError::new("expected a function")),
    }
}

/// (async/spawn f)
fn async_spawn(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/spawn requires 1 argument (function)"));
    }
    let func = args[0].clone();
    let handle = std::thread::spawn(move || call_fn(&func));
    let id = store_handle(Handle::Thread(handle));

    let mut map = HashMap::new();
    map.insert("__handle_id__".to_string(), Value::Int(id as i64));
    map.insert("type".to_string(), Value::str("future"));
    Ok(Value::Map(Arc::new(map)))
}

/// (async/await future)
fn async_await(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/await requires 1 argument (future)"));
    }
    match &args[0] {
        Value::Map(map) => {
            let id = map
                .get("__handle_id__")
                .ok_or_else(|| LispError::new("async/await: not a valid future"))?
                .as_int()? as u64;

            let handle = take_handle(id)?;
            match handle {
                Handle::Thread(jh) => match jh.join() {
                    Ok(result) => result,
                    Err(_) => Err(LispError::new("async/await: thread panicked")),
                },
                _ => Err(LispError::new("async/await: handle is not a thread")),
            }
        }
        _ => Err(LispError::new("async/await: expected a future map")),
    }
}

/// (async/channel)
fn async_channel(args: &[Value]) -> LispResult {
    if !args.is_empty() {
        return Err(LispError::new("async/channel takes no arguments"));
    }
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let tx_id = store_handle(Handle::Sender(tx));
    let rx_id = store_handle(Handle::Receiver(rx));

    let mut sender_map = HashMap::new();
    sender_map.insert("__handle_id__".to_string(), Value::Int(tx_id as i64));
    sender_map.insert("type".to_string(), Value::str("sender"));

    let mut receiver_map = HashMap::new();
    receiver_map.insert("__handle_id__".to_string(), Value::Int(rx_id as i64));
    receiver_map.insert("type".to_string(), Value::str("receiver"));

    let mut map = HashMap::new();
    map.insert("sender".to_string(), Value::Map(Arc::new(sender_map)));
    map.insert("receiver".to_string(), Value::Map(Arc::new(receiver_map)));
    Ok(Value::Map(Arc::new(map)))
}

/// (async/send sender value)
/// Clone the Sender outside the lock to avoid deadlock with recv
fn async_send(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("async/send requires 2 arguments (sender value)"));
    }
    match &args[0] {
        Value::Map(map) => {
            let id = map
                .get("__handle_id__")
                .ok_or_else(|| LispError::new("async/send: not a valid sender"))?
                .as_int()? as u64;

            let tx_clone = {
                let reg = registry().lock().unwrap();
                match reg.get(&id) {
                    Some(Handle::Sender(tx)) => tx.clone(),
                    Some(_) => return Err(LispError::new("async/send: handle is not a sender")),
                    None => return Err(LispError::new("async/send: invalid handle")),
                }
            };
            tx_clone
                .send(args[1].clone())
                .map_err(|e| LispError::new(format!("async/send: {}", e)))?;
            Ok(Value::Nil)
        }
        _ => Err(LispError::new("async/send: expected a sender map")),
    }
}

/// (async/recv receiver)
/// Take the Receiver out of the registry to avoid holding the lock during blocking recv
fn async_recv(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/recv requires 1 argument (receiver)"));
    }
    match &args[0] {
        Value::Map(map) => {
            let id = map
                .get("__handle_id__")
                .ok_or_else(|| LispError::new("async/recv: not a valid receiver"))?
                .as_int()? as u64;

            let handle = take_handle(id)?;
            match handle {
                Handle::Receiver(rx) => {
                    let result = match rx.recv() {
                        Ok(val) => Ok(val),
                        Err(_) => Ok(Value::Nil),
                    };
                    // Put it back for potential reuse
                    store_handle_with_id(id, Handle::Receiver(rx));
                    result
                }
                other => {
                    // Put it back
                    store_handle_with_id(id, other);
                    Err(LispError::new("async/recv: handle is not a receiver"))
                }
            }
        }
        _ => Err(LispError::new("async/recv: expected a receiver map")),
    }
}
