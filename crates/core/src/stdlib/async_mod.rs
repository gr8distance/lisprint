use std::collections::HashMap;
use std::sync::Arc;

use crate::env::Env;
use crate::eval::eval;
use crate::value::{LispError, LispResult, NativeFnData, Value};

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
/// Sleep for the given number of milliseconds (blocking)
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
/// Spawn a function in a new thread, returns a future handle (map with __thread_handle__)
fn async_spawn(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/spawn requires 1 argument (function)"));
    }
    let func = args[0].clone();

    let handle = std::thread::spawn(move || call_fn(&func));

    // Store the JoinHandle as a boxed pointer in a map
    let handle_ptr = Box::into_raw(Box::new(handle)) as i64;
    let mut map = HashMap::new();
    map.insert("__thread_handle__".to_string(), Value::Int(handle_ptr));
    map.insert("type".to_string(), Value::str("future"));
    Ok(Value::Map(Arc::new(map)))
}

/// (async/await future)
/// Wait for a spawned task to complete and return its result
fn async_await(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/await requires 1 argument (future)"));
    }
    let future = &args[0];
    match future {
        Value::Map(map) => {
            let handle_ptr = map.get("__thread_handle__")
                .ok_or_else(|| LispError::new("async/await: not a valid future"))?
                .as_int()?;

            // Safety: we created this pointer in async_spawn
            let handle = unsafe {
                Box::from_raw(handle_ptr as *mut std::thread::JoinHandle<LispResult>)
            };

            match handle.join() {
                Ok(result) => result,
                Err(_) => Err(LispError::new("async/await: thread panicked")),
            }
        }
        _ => Err(LispError::new("async/await: expected a future map")),
    }
}

/// (async/channel)
/// Create a channel, returns {:sender sender :receiver receiver}
fn async_channel(args: &[Value]) -> LispResult {
    if !args.is_empty() {
        return Err(LispError::new("async/channel takes no arguments"));
    }
    let (tx, rx) = std::sync::mpsc::channel::<Value>();

    let tx_ptr = Box::into_raw(Box::new(tx)) as i64;
    let rx_ptr = Box::into_raw(Box::new(rx)) as i64;

    let mut sender_map = HashMap::new();
    sender_map.insert("__channel_sender__".to_string(), Value::Int(tx_ptr));
    sender_map.insert("type".to_string(), Value::str("sender"));

    let mut receiver_map = HashMap::new();
    receiver_map.insert("__channel_receiver__".to_string(), Value::Int(rx_ptr));
    receiver_map.insert("type".to_string(), Value::str("receiver"));

    let mut map = HashMap::new();
    map.insert("sender".to_string(), Value::Map(Arc::new(sender_map)));
    map.insert("receiver".to_string(), Value::Map(Arc::new(receiver_map)));
    Ok(Value::Map(Arc::new(map)))
}

/// (async/send sender value)
/// Send a value through a channel
fn async_send(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("async/send requires 2 arguments (sender value)"));
    }
    let sender = &args[0];
    match sender {
        Value::Map(map) => {
            let tx_ptr = map.get("__channel_sender__")
                .ok_or_else(|| LispError::new("async/send: not a valid sender"))?
                .as_int()?;

            // Safety: we created this pointer in async_channel
            // Note: we don't consume the sender, so we use a reference
            let tx = unsafe {
                &*(tx_ptr as *const std::sync::mpsc::Sender<Value>)
            };

            tx.send(args[1].clone())
                .map_err(|e| LispError::new(format!("async/send: {}", e)))?;
            Ok(Value::Nil)
        }
        _ => Err(LispError::new("async/send: expected a sender map")),
    }
}

/// (async/recv receiver)
/// Receive a value from a channel (blocking)
fn async_recv(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("async/recv requires 1 argument (receiver)"));
    }
    let receiver = &args[0];
    match receiver {
        Value::Map(map) => {
            let rx_ptr = map.get("__channel_receiver__")
                .ok_or_else(|| LispError::new("async/recv: not a valid receiver"))?
                .as_int()?;

            // Safety: we created this pointer in async_channel
            let rx = unsafe {
                &*(rx_ptr as *const std::sync::mpsc::Receiver<Value>)
            };

            match rx.recv() {
                Ok(val) => Ok(val),
                Err(_) => Ok(Value::Nil), // channel closed
            }
        }
        _ => Err(LispError::new("async/recv: expected a receiver map")),
    }
}
