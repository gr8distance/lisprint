use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    reg(env, "math/abs", math_abs);
    reg(env, "math/sqrt", math_sqrt);
    reg(env, "math/pow", math_pow);
    reg(env, "math/sin", math_sin);
    reg(env, "math/cos", math_cos);
    reg(env, "math/tan", math_tan);
    reg(env, "math/floor", math_floor);
    reg(env, "math/ceil", math_ceil);
    reg(env, "math/round", math_round);
    reg(env, "math/min", math_min);
    reg(env, "math/max", math_max);
    reg(env, "math/pi", math_pi);
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

fn math_abs(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/abs requires 1 argument")); }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        Value::Float(n) => Ok(Value::Float(n.abs())),
        _ => Err(LispError::new("math/abs requires a number")),
    }
}

fn math_sqrt(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/sqrt requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.sqrt()))
}

fn math_pow(args: &[Value]) -> LispResult {
    if args.len() != 2 { return Err(LispError::new("math/pow requires 2 arguments")); }
    Ok(Value::Float(args[0].as_float()?.powf(args[1].as_float()?)))
}

fn math_sin(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/sin requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.sin()))
}

fn math_cos(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/cos requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.cos()))
}

fn math_tan(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/tan requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.tan()))
}

fn math_floor(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/floor requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.floor()))
}

fn math_ceil(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/ceil requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.ceil()))
}

fn math_round(args: &[Value]) -> LispResult {
    if args.len() != 1 { return Err(LispError::new("math/round requires 1 argument")); }
    Ok(Value::Float(args[0].as_float()?.round()))
}

fn math_min(args: &[Value]) -> LispResult {
    if args.len() < 1 { return Err(LispError::new("math/min requires at least 1 argument")); }
    let mut min = args[0].as_float()?;
    for arg in &args[1..] {
        let v = arg.as_float()?;
        if v < min { min = v; }
    }
    if args.iter().all(|a| matches!(a, Value::Int(_))) {
        Ok(Value::Int(min as i64))
    } else {
        Ok(Value::Float(min))
    }
}

fn math_max(args: &[Value]) -> LispResult {
    if args.len() < 1 { return Err(LispError::new("math/max requires at least 1 argument")); }
    let mut max = args[0].as_float()?;
    for arg in &args[1..] {
        let v = arg.as_float()?;
        if v > max { max = v; }
    }
    if args.iter().all(|a| matches!(a, Value::Int(_))) {
        Ok(Value::Int(max as i64))
    } else {
        Ok(Value::Float(max))
    }
}

fn math_pi(args: &[Value]) -> LispResult {
    if !args.is_empty() { return Err(LispError::new("math/pi takes no arguments")); }
    Ok(Value::Float(std::f64::consts::PI))
}
