use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "time/now", time_now);
    reg(env, "time/millis", time_millis);
    reg(env, "time/sleep", time_sleep);
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

fn time_now(_args: &[Value]) -> LispResult {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| LispError::new(format!("time/now: {}", e)))?;
    Ok(Value::Int(dur.as_secs() as i64))
}

fn time_millis(_args: &[Value]) -> LispResult {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| LispError::new(format!("time/millis: {}", e)))?;
    Ok(Value::Int(dur.as_millis() as i64))
}

fn time_sleep(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("time/sleep requires 1 argument (milliseconds)")); }
    let ms = args[0].as_int()? as u64;
    std::thread::sleep(std::time::Duration::from_millis(ms));
    Ok(Value::Nil)
}
