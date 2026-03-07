use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispFn, LispResult, Value};

/// 式を評価する
pub fn eval(value: &Value, env: &mut Env) -> LispResult {
    match value {
        Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
            Ok(value.clone())
        }

        Value::Keyword(_) => Ok(value.clone()),

        Value::Symbol(name) => env.get(name),

        Value::Vec(items) => {
            let evaluated: Result<Vec<Value>, _> = items.iter().map(|v| eval(v, env)).collect();
            Ok(Value::vec(evaluated?))
        }

        Value::List(items) => {
            if items.is_empty() {
                return Ok(Value::list(vec![]));
            }
            eval_list(items, env)
        }

        _ => Err(LispError::new(format!("cannot evaluate: {}", value))),
    }
}

fn eval_list(items: &[Value], env: &mut Env) -> LispResult {
    let head = &items[0];

    // 特殊形式チェック
    if let Value::Symbol(name) = head {
        match name.as_str() {
            "def" => return eval_def(&items[1..], env),
            "defun" => return eval_defun(&items[1..], env),
            "fn" => return eval_fn(&items[1..], env),
            "if" => return eval_if(&items[1..], env),
            "let" => return eval_let(&items[1..], env),
            "do" => return eval_do(&items[1..], env),
            "quote" => return eval_quote(&items[1..]),
            "loop" => return eval_loop(&items[1..], env),
            _ => {}
        }
    }

    // 関数呼び出し
    let func = eval(head, env)?;
    let args: Result<Vec<Value>, _> = items[1..].iter().map(|v| eval(v, env)).collect();
    let args = args?;

    apply(&func, &args)
}

/// 関数を適用する (TCO対応)
pub fn apply(func: &Value, args: &[Value]) -> LispResult {
    match func {
        Value::NativeFn(native) => (native.func)(args),
        Value::Fn(lisp_fn) => {
            if args.len() != lisp_fn.params.len() {
                return Err(LispError::new(format!(
                    "{}: expected {} args, got {}",
                    lisp_fn.name.as_deref().unwrap_or("fn"),
                    lisp_fn.params.len(),
                    args.len()
                )));
            }

            let mut fn_env = Env::with_parent(Arc::new(lisp_fn.env.clone()));
            for (param, arg) in lisp_fn.params.iter().zip(args.iter()) {
                fn_env.define(param.clone(), arg.clone());
            }

            // TCO: 最後の式だけ再帰的に評価
            let body = &lisp_fn.body;
            if body.is_empty() {
                return Ok(Value::Nil);
            }

            for expr in &body[..body.len() - 1] {
                eval(expr, &mut fn_env)?;
            }

            eval(&body[body.len() - 1], &mut fn_env)
        }
        _ => Err(LispError::new(format!("{} is not a function", func))),
    }
}

// --- 特殊形式 ---

/// (def name value)
fn eval_def(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::new("def requires exactly 2 arguments"));
    }
    let name = args[0].as_symbol()?;
    let val = eval(&args[1], env)?;
    env.define(name, val.clone());
    Ok(val)
}

/// (defun name (params...) body...)
fn eval_defun(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() < 3 {
        return Err(LispError::new("defun requires name, params, and body"));
    }

    let name = args[0].as_symbol()?.to_string();
    let params = parse_params(&args[1])?;
    let body = args[2..].to_vec();

    let func = Value::Fn(Arc::new(LispFn {
        name: Some(name.clone()),
        params,
        body,
        env: env.clone(),
    }));

    env.define(name, func.clone());
    Ok(func)
}

/// (fn (params...) body...)
fn eval_fn(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::new("fn requires params and body"));
    }

    let params = parse_params(&args[0])?;
    let body = args[1..].to_vec();

    Ok(Value::Fn(Arc::new(LispFn {
        name: None,
        params,
        body,
        env: env.clone(),
    })))
}

/// (if cond then else?)
fn eval_if(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() < 2 || args.len() > 3 {
        return Err(LispError::new("if requires 2 or 3 arguments"));
    }

    let cond = eval(&args[0], env)?;
    if cond.is_truthy() {
        eval(&args[1], env)
    } else if args.len() == 3 {
        eval(&args[2], env)
    } else {
        Ok(Value::Nil)
    }
}

/// (let [bindings...] body...)
fn eval_let(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Err(LispError::new("let requires bindings"));
    }

    let bindings = args[0].as_vec().or_else(|_| args[0].as_list())?;
    if bindings.len() % 2 != 0 {
        return Err(LispError::new("let bindings must have even number of elements"));
    }

    let mut let_env = Env::with_parent(Arc::new(env.clone()));

    for chunk in bindings.chunks(2) {
        let name = chunk[0].as_symbol()?;
        let val = eval(&chunk[1], &mut let_env)?;
        let_env.define(name, val);
    }

    // evaluate body
    let body = &args[1..];
    if body.is_empty() {
        return Ok(Value::Nil);
    }
    for expr in &body[..body.len() - 1] {
        eval(expr, &mut let_env)?;
    }
    eval(&body[body.len() - 1], &mut let_env)
}

/// (do exprs...)
fn eval_do(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Ok(Value::Nil);
    }
    for expr in &args[..args.len() - 1] {
        eval(expr, env)?;
    }
    eval(&args[args.len() - 1], env)
}

/// (quote expr)
fn eval_quote(args: &[Value]) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("quote requires exactly 1 argument"));
    }
    Ok(args[0].clone())
}

/// (loop [bindings...] body...)
fn eval_loop(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Err(LispError::new("loop requires bindings"));
    }

    let bindings = args[0].as_vec().or_else(|_| args[0].as_list())?;
    if bindings.len() % 2 != 0 {
        return Err(LispError::new("loop bindings must have even number of elements"));
    }

    let body = &args[1..];
    let mut names = Vec::new();
    let mut values = Vec::new();

    for chunk in bindings.chunks(2) {
        names.push(chunk[0].as_symbol()?.to_string());
        values.push(chunk[1].clone());
    }

    loop {
        let mut loop_env = Env::with_parent(Arc::new(env.clone()));
        for (name, val) in names.iter().zip(values.iter()) {
            let evaluated = eval(val, &mut loop_env)?;
            loop_env.define(name.clone(), evaluated);
        }

        match eval_body_with_recur(body, &mut loop_env)? {
            LoopResult::Return(val) => return Ok(val),
            LoopResult::Recur(new_values) => {
                if new_values.len() != names.len() {
                    return Err(LispError::new(format!(
                        "recur: expected {} args, got {}",
                        names.len(),
                        new_values.len()
                    )));
                }
                values = new_values.into_iter().map(|v| quote_value(v)).collect();
            }
        }
    }
}

enum LoopResult {
    Return(Value),
    Recur(Vec<Value>),
}

fn eval_with_recur(expr: &Value, env: &mut Env) -> Result<LoopResult, LispError> {
    // recur チェック
    if let Value::List(items) = expr {
        if !items.is_empty() {
            if let Value::Symbol(name) = &items[0] {
                match name.as_str() {
                    "recur" => {
                        let args: Result<Vec<Value>, _> =
                            items[1..].iter().map(|v| eval(v, env)).collect();
                        return Ok(LoopResult::Recur(args?));
                    }
                    "if" => {
                        return eval_if_with_recur(&items[1..], env);
                    }
                    "do" => {
                        if items.len() <= 1 {
                            return Ok(LoopResult::Return(Value::Nil));
                        }
                        for e in &items[1..items.len() - 1] {
                            eval(e, env)?;
                        }
                        return eval_with_recur(&items[items.len() - 1], env);
                    }
                    "let" => {
                        let bindings = items[1].as_vec().or_else(|_| items[1].as_list())?;
                        if bindings.len() % 2 != 0 {
                            return Err(LispError::new("let bindings must have even number of elements"));
                        }
                        let mut let_env = Env::with_parent(Arc::new(env.clone()));
                        for chunk in bindings.chunks(2) {
                            let bname = chunk[0].as_symbol()?;
                            let val = eval(&chunk[1], &mut let_env)?;
                            let_env.define(bname, val);
                        }
                        let body = &items[2..];
                        if body.is_empty() {
                            return Ok(LoopResult::Return(Value::Nil));
                        }
                        for e in &body[..body.len() - 1] {
                            eval(e, &mut let_env)?;
                        }
                        return eval_with_recur(&body[body.len() - 1], &mut let_env);
                    }
                    _ => {}
                }
            }
        }
    }

    let val = eval(expr, env)?;
    Ok(LoopResult::Return(val))
}

fn eval_if_with_recur(args: &[Value], env: &mut Env) -> Result<LoopResult, LispError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(LispError::new("if requires 2 or 3 arguments"));
    }
    let cond = eval(&args[0], env)?;
    if cond.is_truthy() {
        eval_with_recur(&args[1], env)
    } else if args.len() == 3 {
        eval_with_recur(&args[2], env)
    } else {
        Ok(LoopResult::Return(Value::Nil))
    }
}

fn eval_body_with_recur(body: &[Value], env: &mut Env) -> Result<LoopResult, LispError> {
    if body.is_empty() {
        return Ok(LoopResult::Return(Value::Nil));
    }

    for expr in &body[..body.len() - 1] {
        eval(expr, env)?;
    }

    eval_with_recur(&body[body.len() - 1], env)
}

/// 評価済みの値をquoteでラップ (loop再突入時用)
fn quote_value(val: Value) -> Value {
    Value::list(vec![Value::symbol("quote"), val])
}

// --- ヘルパー ---

fn parse_params(value: &Value) -> Result<Vec<String>, LispError> {
    let items = value.as_vec().or_else(|_| value.as_list())?;
    items
        .iter()
        .map(|v| {
            v.as_symbol()
                .map(|s| s.to_string())
                .map_err(|_| LispError::new("function parameters must be symbols"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn eval_str(input: &str) -> LispResult {
        let exprs = parse(input).unwrap();
        let mut env = Env::new();
        crate::builtins::register(&mut env);
        let mut result = Value::Nil;
        for expr in &exprs {
            result = eval(expr, &mut env)?;
        }
        Ok(result)
    }

    #[test]
    fn test_eval_int() {
        assert_eq!(eval_str("42").unwrap(), Value::Int(42));
    }

    #[test]
    fn test_eval_string() {
        assert_eq!(eval_str("\"hello\"").unwrap(), Value::str("hello"));
    }

    #[test]
    fn test_eval_arithmetic() {
        assert_eq!(eval_str("(+ 1 2)").unwrap(), Value::Int(3));
        assert_eq!(eval_str("(- 10 3)").unwrap(), Value::Int(7));
        assert_eq!(eval_str("(* 3 4)").unwrap(), Value::Int(12));
        assert_eq!(eval_str("(/ 10 3)").unwrap(), Value::Int(3));
    }

    #[test]
    fn test_eval_nested() {
        assert_eq!(eval_str("(+ 1 (* 2 3))").unwrap(), Value::Int(7));
    }

    #[test]
    fn test_eval_def() {
        assert_eq!(eval_str("(def x 42) x").unwrap(), Value::Int(42));
    }

    #[test]
    fn test_eval_if_true() {
        assert_eq!(eval_str("(if true 1 2)").unwrap(), Value::Int(1));
    }

    #[test]
    fn test_eval_if_false() {
        assert_eq!(eval_str("(if false 1 2)").unwrap(), Value::Int(2));
    }

    #[test]
    fn test_eval_if_nil() {
        assert_eq!(eval_str("(if nil 1 2)").unwrap(), Value::Int(2));
    }

    #[test]
    fn test_eval_let() {
        assert_eq!(eval_str("(let [x 10 y 20] (+ x y))").unwrap(), Value::Int(30));
    }

    #[test]
    fn test_eval_defun() {
        assert_eq!(
            eval_str("(defun add (a b) (+ a b)) (add 3 4)").unwrap(),
            Value::Int(7)
        );
    }

    #[test]
    fn test_eval_fn() {
        assert_eq!(
            eval_str("(def add (fn (a b) (+ a b))) (add 3 4)").unwrap(),
            Value::Int(7)
        );
    }

    #[test]
    fn test_eval_do() {
        assert_eq!(eval_str("(do 1 2 3)").unwrap(), Value::Int(3));
    }

    #[test]
    fn test_eval_quote() {
        let result = eval_str("'(1 2 3)").unwrap();
        assert_eq!(result, Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
    }

    #[test]
    fn test_eval_comparison() {
        assert_eq!(eval_str("(= 1 1)").unwrap(), Value::Bool(true));
        assert_eq!(eval_str("(< 1 2)").unwrap(), Value::Bool(true));
        assert_eq!(eval_str("(> 2 1)").unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_eval_loop_recur() {
        assert_eq!(
            eval_str("(loop [i 0 acc 0] (if (= i 5) acc (recur (+ i 1) (+ acc i))))").unwrap(),
            Value::Int(10) // 0+1+2+3+4
        );
    }

    #[test]
    fn test_eval_closure() {
        assert_eq!(
            eval_str("(defun make-adder (x) (fn (y) (+ x y))) (def add5 (make-adder 5)) (add5 3)").unwrap(),
            Value::Int(8)
        );
    }

    #[test]
    fn test_eval_closure_lexical_scope() {
        assert_eq!(
            eval_str("(def x 10) (defun get-x () x) (let [x 20] (get-x))").unwrap(),
            Value::Int(10) // lexical scope: get-x captures x=10
        );
    }
}
