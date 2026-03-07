use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispResult, NativeFnData, Value};

pub fn register(env: &mut Env) {
    // 算術
    register_fn(env, "+", builtin_add);
    register_fn(env, "-", builtin_sub);
    register_fn(env, "*", builtin_mul);
    register_fn(env, "/", builtin_div);
    register_fn(env, "mod", builtin_mod);

    // 比較
    register_fn(env, "=", builtin_eq);
    register_fn(env, "<", builtin_lt);
    register_fn(env, ">", builtin_gt);
    register_fn(env, "<=", builtin_lte);
    register_fn(env, ">=", builtin_gte);

    // 論理
    register_fn(env, "not", builtin_not);

    // リスト操作
    register_fn(env, "list", builtin_list);
    register_fn(env, "cons", builtin_cons);
    register_fn(env, "first", builtin_first);
    register_fn(env, "rest", builtin_rest);
    register_fn(env, "nth", builtin_nth);
    register_fn(env, "count", builtin_count);
    register_fn(env, "empty?", builtin_empty);
    register_fn(env, "concat", builtin_concat);

    // 型判定
    register_fn(env, "nil?", builtin_nil_q);
    register_fn(env, "number?", builtin_number_q);
    register_fn(env, "string?", builtin_string_q);
    register_fn(env, "list?", builtin_list_q);
    register_fn(env, "fn?", builtin_fn_q);

    // 文字列
    register_fn(env, "str", builtin_str);

    // IO
    register_fn(env, "println", builtin_println);
    register_fn(env, "print", builtin_print);

    // その他
    register_fn(env, "apply", builtin_apply);
    register_fn(env, "identity", builtin_identity);

    // テスト用アサーション
    register_fn(env, "assert=", builtin_assert_eq);
    register_fn(env, "assert-true", builtin_assert_true);
    register_fn(env, "assert-nil", builtin_assert_nil);
}

fn register_fn(
    env: &mut Env,
    name: &str,
    func: fn(&[Value]) -> LispResult,
) {
    let name = name.to_string();
    env.define(
        name.clone(),
        Value::NativeFn(Arc::new(NativeFnData {
            name: name.clone(),
            func: Box::new(move |args| func(args)),
        })),
    );
}

// --- 算術 ---

fn builtin_add(args: &[Value]) -> LispResult {
    numeric_fold(args, 0, |a, b| a + b, 0.0, |a, b| a + b)
}

fn builtin_sub(args: &[Value]) -> LispResult {
    if args.len() == 1 {
        return match &args[0] {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(n) => Ok(Value::Float(-n)),
            _ => Err(LispError::new("- requires numbers")),
        };
    }
    numeric_reduce(args, |a, b| a - b, |a, b| a - b)
}

fn builtin_mul(args: &[Value]) -> LispResult {
    numeric_fold(args, 1, |a, b| a * b, 1.0, |a, b| a * b)
}

fn builtin_div(args: &[Value]) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::new("/ requires at least 2 arguments"));
    }
    numeric_reduce(args, |a, b| a / b, |a, b| a / b)
}

fn builtin_mod(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("mod requires exactly 2 arguments"));
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
        _ => Err(LispError::new("mod requires integers")),
    }
}

fn numeric_fold(
    args: &[Value],
    int_init: i64,
    int_op: fn(i64, i64) -> i64,
    float_init: f64,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let mut has_float = false;
    for arg in args {
        if matches!(arg, Value::Float(_)) {
            has_float = true;
            break;
        }
    }

    if has_float {
        let mut acc = float_init;
        for arg in args {
            acc = float_op(acc, arg.as_float()?);
        }
        Ok(Value::Float(acc))
    } else {
        let mut acc = int_init;
        for arg in args {
            acc = int_op(acc, arg.as_int()?);
        }
        Ok(Value::Int(acc))
    }
}

fn numeric_reduce(
    args: &[Value],
    int_op: fn(i64, i64) -> i64,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let has_float = args.iter().any(|a| matches!(a, Value::Float(_)));

    if has_float {
        let mut acc = args[0].as_float()?;
        for arg in &args[1..] {
            acc = float_op(acc, arg.as_float()?);
        }
        Ok(Value::Float(acc))
    } else {
        let mut acc = args[0].as_int()?;
        for arg in &args[1..] {
            acc = int_op(acc, arg.as_int()?);
        }
        Ok(Value::Int(acc))
    }
}

// --- 比較 ---

fn builtin_eq(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("= requires exactly 2 arguments"));
    }
    Ok(Value::Bool(args[0] == args[1]))
}

fn builtin_lt(args: &[Value]) -> LispResult {
    compare_numbers(args, |a, b| a < b, |a, b| a < b)
}

fn builtin_gt(args: &[Value]) -> LispResult {
    compare_numbers(args, |a, b| a > b, |a, b| a > b)
}

fn builtin_lte(args: &[Value]) -> LispResult {
    compare_numbers(args, |a, b| a <= b, |a, b| a <= b)
}

fn builtin_gte(args: &[Value]) -> LispResult {
    compare_numbers(args, |a, b| a >= b, |a, b| a >= b)
}

fn compare_numbers(
    args: &[Value],
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("comparison requires exactly 2 arguments"));
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(int_cmp(*a, *b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(*a, *b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(*a as f64, *b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(float_cmp(*a, *b as f64))),
        _ => Err(LispError::new("comparison requires numbers")),
    }
}

// --- 論理 ---

fn builtin_not(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("not requires exactly 1 argument"));
    }
    Ok(Value::Bool(!args[0].is_truthy()))
}

// --- リスト操作 ---

fn builtin_list(args: &[Value]) -> LispResult {
    Ok(Value::list(args.to_vec()))
}

fn builtin_cons(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("cons requires exactly 2 arguments"));
    }
    let tail = args[1].as_list()?;
    let mut new_list = vec![args[0].clone()];
    new_list.extend_from_slice(tail);
    Ok(Value::list(new_list))
}

fn builtin_first(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("first requires exactly 1 argument"));
    }
    let items = args[0].as_list().or_else(|_| args[0].as_vec())?;
    Ok(items.first().cloned().unwrap_or(Value::Nil))
}

fn builtin_rest(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("rest requires exactly 1 argument"));
    }
    let items = args[0].as_list().or_else(|_| args[0].as_vec())?;
    if items.is_empty() {
        Ok(Value::list(vec![]))
    } else {
        Ok(Value::list(items[1..].to_vec()))
    }
}

fn builtin_nth(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("nth requires exactly 2 arguments"));
    }
    let items = args[0].as_list().or_else(|_| args[0].as_vec())?;
    let idx = args[1].as_int()? as usize;
    Ok(items.get(idx).cloned().unwrap_or(Value::Nil))
}

fn builtin_count(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("count requires exactly 1 argument"));
    }
    match &args[0] {
        Value::List(v) | Value::Vec(v) => Ok(Value::Int(v.len() as i64)),
        Value::Str(s) => Ok(Value::Int(s.len() as i64)),
        Value::Nil => Ok(Value::Int(0)),
        _ => Err(LispError::new("count requires a collection")),
    }
}

fn builtin_empty(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("empty? requires exactly 1 argument"));
    }
    match &args[0] {
        Value::List(v) | Value::Vec(v) => Ok(Value::Bool(v.is_empty())),
        Value::Nil => Ok(Value::Bool(true)),
        _ => Err(LispError::new("empty? requires a collection")),
    }
}

fn builtin_concat(args: &[Value]) -> LispResult {
    let mut result = Vec::new();
    for arg in args {
        match arg {
            Value::List(v) | Value::Vec(v) => result.extend_from_slice(v),
            Value::Nil => {}
            _ => return Err(LispError::new("concat requires lists")),
        }
    }
    Ok(Value::list(result))
}

// --- 型判定 ---

fn builtin_nil_q(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("nil? requires exactly 1 argument"));
    }
    Ok(Value::Bool(args[0].is_nil()))
}

fn builtin_number_q(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("number? requires exactly 1 argument"));
    }
    Ok(Value::Bool(matches!(args[0], Value::Int(_) | Value::Float(_))))
}

fn builtin_string_q(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("string? requires exactly 1 argument"));
    }
    Ok(Value::Bool(matches!(args[0], Value::Str(_))))
}

fn builtin_list_q(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("list? requires exactly 1 argument"));
    }
    Ok(Value::Bool(matches!(args[0], Value::List(_))))
}

fn builtin_fn_q(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("fn? requires exactly 1 argument"));
    }
    Ok(Value::Bool(matches!(args[0], Value::Fn(_) | Value::NativeFn(_))))
}

// --- 文字列 ---

fn builtin_str(args: &[Value]) -> LispResult {
    let mut s = String::new();
    for arg in args {
        match arg {
            Value::Str(v) => s.push_str(v),
            other => s.push_str(&format!("{}", other)),
        }
    }
    Ok(Value::str(s))
}

// --- IO ---

fn builtin_println(args: &[Value]) -> LispResult {
    let parts: Vec<String> = args.iter().map(|a| match a {
        Value::Str(s) => s.to_string(),
        other => format!("{}", other),
    }).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Nil)
}

fn builtin_print(args: &[Value]) -> LispResult {
    let parts: Vec<String> = args.iter().map(|a| match a {
        Value::Str(s) => s.to_string(),
        other => format!("{}", other),
    }).collect();
    print!("{}", parts.join(" "));
    Ok(Value::Nil)
}

// --- その他 ---

fn builtin_apply(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("apply requires exactly 2 arguments"));
    }
    let func = &args[0];
    let list_args = args[1].as_list()?;
    crate::eval::apply(func, list_args)
}

fn builtin_identity(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("identity requires exactly 1 argument"));
    }
    Ok(args[0].clone())
}

// --- テスト用アサーション ---

fn builtin_assert_eq(args: &[Value]) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("assert= requires exactly 2 arguments"));
    }
    if args[0] == args[1] {
        Ok(Value::Bool(true))
    } else {
        Err(LispError::new(format!(
            "assertion failed: {} != {}",
            args[0], args[1]
        )))
    }
}

fn builtin_assert_true(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("assert-true requires exactly 1 argument"));
    }
    if args[0].is_truthy() {
        Ok(Value::Bool(true))
    } else {
        Err(LispError::new(format!(
            "assertion failed: expected truthy, got {}",
            args[0]
        )))
    }
}

fn builtin_assert_nil(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("assert-nil requires exactly 1 argument"));
    }
    if args[0].is_nil() {
        Ok(Value::Bool(true))
    } else {
        Err(LispError::new(format!(
            "assertion failed: expected nil, got {}",
            args[0]
        )))
    }
}
