use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, LispFn, LispResult, Value};

thread_local! {
    static LOADING_MODULES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

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
            "quasiquote" => return eval_quasiquote(&items[1..], env),
            "defmacro" => return eval_defmacro(&items[1..], env),
            "macroexpand" => return eval_macroexpand(&items[1..], env),
            "loop" => return eval_loop(&items[1..], env),
            "throw" => return eval_throw(&items[1..], env),
            "try" => return eval_try(&items[1..], env),
            "with" => return eval_with(&items[1..], env),
            "ns" => return eval_ns(&items[1..], env),
            "require" => return eval_require(&items[1..], env),
            _ => {}
        }
    }

    // マクロ展開チェック
    let head_val = eval(head, env)?;
    if let Value::Macro(mac) = &head_val {
        let expanded = expand_macro(mac, &items[1..], env)?;
        return eval(&expanded, env);
    }

    // 関数呼び出し
    let args: Result<Vec<Value>, _> = items[1..].iter().map(|v| eval(v, env)).collect();
    let args = args?;

    apply(&head_val, &args)
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

            // 再帰呼び出し対応: 名前付き関数は自分自身を環境に注入
            if let Some(name) = &lisp_fn.name {
                fn_env.define(name.clone(), func.clone());
            }

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

/// (defmacro name (params...) body...)
fn eval_defmacro(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() < 3 {
        return Err(LispError::new("defmacro requires name, params, and body"));
    }

    let name = args[0].as_symbol()?.to_string();
    let params = parse_params(&args[1])?;
    let body = args[2..].to_vec();

    let mac = Value::Macro(Arc::new(LispFn {
        name: Some(name.clone()),
        params,
        body,
        env: env.clone(),
    }));

    env.define(name, mac.clone());
    Ok(mac)
}

/// マクロを展開する (引数は評価せずに渡す)
fn expand_macro(mac: &LispFn, args: &[Value], _env: &mut Env) -> LispResult {
    if args.len() != mac.params.len() {
        return Err(LispError::new(format!(
            "macro {}: expected {} args, got {}",
            mac.name.as_deref().unwrap_or("anonymous"),
            mac.params.len(),
            args.len()
        )));
    }

    let mut macro_env = Env::with_parent(Arc::new(mac.env.clone()));
    for (param, arg) in mac.params.iter().zip(args.iter()) {
        macro_env.define(param.clone(), arg.clone());
    }

    // マクロのbodyを評価して展開結果を返す
    let body = &mac.body;
    if body.is_empty() {
        return Ok(Value::Nil);
    }
    for expr in &body[..body.len() - 1] {
        eval(expr, &mut macro_env)?;
    }
    eval(&body[body.len() - 1], &mut macro_env)
}

/// (macroexpand expr) — マクロを1回展開する (デバッグ用)
fn eval_macroexpand(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("macroexpand requires exactly 1 argument"));
    }
    let expr = eval(&args[0], env)?;
    if let Value::List(items) = &expr {
        if !items.is_empty() {
            if let Ok(head_val) = eval(&items[0], env) {
                if let Value::Macro(mac) = &head_val {
                    return expand_macro(mac, &items[1..], env);
                }
            }
        }
    }
    Ok(expr)
}

/// (quasiquote expr) — ` 記法
fn eval_quasiquote(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("quasiquote requires exactly 1 argument"));
    }
    expand_quasiquote(&args[0], env)
}

fn expand_quasiquote(value: &Value, env: &mut Env) -> LispResult {
    match value {
        Value::List(items) => {
            if items.is_empty() {
                return Ok(Value::list(vec![]));
            }

            // (unquote expr) → eval expr
            if let Value::Symbol(s) = &items[0] {
                if s.as_str() == "unquote" {
                    if items.len() != 2 {
                        return Err(LispError::new("unquote requires exactly 1 argument"));
                    }
                    return eval(&items[1], env);
                }
            }

            // 各要素を展開、splice-unquoteを処理
            let mut result = Vec::new();
            for item in items.iter() {
                if let Value::List(inner) = item {
                    if !inner.is_empty() {
                        if let Value::Symbol(s) = &inner[0] {
                            if s.as_str() == "splice-unquote" {
                                if inner.len() != 2 {
                                    return Err(LispError::new("splice-unquote requires exactly 1 argument"));
                                }
                                let val = eval(&inner[1], env)?;
                                let spliced = val.as_list()?;
                                result.extend_from_slice(spliced);
                                continue;
                            }
                        }
                    }
                }
                result.push(expand_quasiquote(item, env)?);
            }
            Ok(Value::list(result))
        }
        _ => Ok(value.clone()),
    }
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

/// (throw value)
fn eval_throw(args: &[Value], env: &mut Env) -> LispResult {
    if args.len() != 1 {
        return Err(LispError::new("throw requires exactly 1 argument"));
    }
    let val = eval(&args[0], env)?;
    Err(LispError::thrown(val))
}

/// (try body... (catch e handler...))
fn eval_try(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Err(LispError::new("try requires a body"));
    }

    // 最後の引数が (catch e ...) かチェック
    let (body, catch_clause) = match args.last() {
        Some(Value::List(items))
            if !items.is_empty()
                && matches!(&items[0], Value::Symbol(s) if s.as_str() == "catch") =>
        {
            (&args[..args.len() - 1], Some(items.as_slice()))
        }
        _ => (args, None),
    };

    // body を評価
    let mut result = Value::Nil;
    for expr in body {
        match eval(expr, env) {
            Ok(val) => result = val,
            Err(err) => {
                // catch 節があればエラーをハンドル
                if let Some(clause) = catch_clause {
                    if clause.len() < 3 {
                        return Err(LispError::new("catch requires error binding and handler"));
                    }
                    let err_name = clause[1].as_symbol()?;
                    let mut catch_env = Env::with_parent(Arc::new(env.clone()));
                    // thrown value があればそれを、なければ文字列メッセージを束縛
                    let err_val = err.thrown.unwrap_or_else(|| Value::str(err.message));
                    catch_env.define(err_name, err_val);
                    let handler = &clause[2..];
                    let mut handler_result = Value::Nil;
                    for expr in handler {
                        handler_result = eval(expr, &mut catch_env)?;
                    }
                    return Ok(handler_result);
                }
                return Err(err);
            }
        }
    }
    Ok(result)
}

/// (with [name expr ...] body...) — nil短絡
fn eval_with(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Err(LispError::new("with requires bindings"));
    }

    let bindings = args[0].as_vec().or_else(|_| args[0].as_list())?;
    if bindings.len() % 2 != 0 {
        return Err(LispError::new("with bindings must have even number of elements"));
    }

    let mut with_env = Env::with_parent(Arc::new(env.clone()));

    for chunk in bindings.chunks(2) {
        let name = chunk[0].as_symbol()?;
        let val = eval(&chunk[1], &mut with_env)?;
        if val.is_nil() {
            return Ok(Value::Nil);
        }
        with_env.define(name, val);
    }

    let body = &args[1..];
    if body.is_empty() {
        return Ok(Value::Nil);
    }
    for expr in &body[..body.len() - 1] {
        eval(expr, &mut with_env)?;
    }
    eval(&body[body.len() - 1], &mut with_env)
}

/// (ns name (export sym1 sym2 ...))
fn eval_ns(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Err(LispError::new("ns requires a module name"));
    }

    let name = args[0].as_symbol()?;
    env.define("__ns__", Value::symbol(name));

    for clause in &args[1..] {
        if let Value::List(items) = clause {
            if !items.is_empty() {
                if let Value::Symbol(s) = &items[0] {
                    if s.as_str() == "export" {
                        env.define("__exports__", Value::list(items[1..].to_vec()));
                    }
                }
            }
        }
    }

    Ok(Value::Nil)
}

/// (require 'name) / (require 'name :as 'alias) / (require 'name :only '(sym1)) / (require 'name :all)
fn eval_require(args: &[Value], env: &mut Env) -> LispResult {
    if args.is_empty() {
        return Err(LispError::new("require requires a module name"));
    }

    let mod_val = eval(&args[0], env)?;
    let mod_name = match &mod_val {
        Value::Symbol(s) => s.to_string(),
        Value::Str(s) => s.to_string(),
        _ => return Err(LispError::new("require: module name must be a symbol or string")),
    };

    // Parse options
    let mut alias: Option<String> = None;
    let mut only: Option<Vec<String>> = None;
    let mut import_all = false;

    let mut i = 1;
    while i < args.len() {
        let kw = eval(&args[i], env)?;
        match &kw {
            Value::Keyword(k) => match k.as_str() {
                "as" => {
                    i += 1;
                    if i >= args.len() {
                        return Err(LispError::new("require :as expects a symbol"));
                    }
                    let alias_val = eval(&args[i], env)?;
                    alias = Some(match &alias_val {
                        Value::Symbol(s) => s.to_string(),
                        _ => return Err(LispError::new("require :as expects a symbol")),
                    });
                }
                "only" => {
                    i += 1;
                    if i >= args.len() {
                        return Err(LispError::new("require :only expects a list"));
                    }
                    let only_val = eval(&args[i], env)?;
                    let only_list = only_val.as_list()?;
                    only = Some(
                        only_list
                            .iter()
                            .map(|v| match v {
                                Value::Symbol(s) => Ok(s.to_string()),
                                _ => Err(LispError::new("require :only expects symbols")),
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                "all" => {
                    import_all = true;
                }
                other => return Err(LispError::new(format!("require: unknown option :{}", other))),
            },
            _ => return Err(LispError::new("require: options must be keywords")),
        }
        i += 1;
    }

    // Circular dependency check
    LOADING_MODULES.with(|loading| {
        if loading.borrow().contains(&mod_name) {
            return Err(LispError::new(format!(
                "circular dependency detected: '{}'",
                mod_name
            )));
        }
        loading.borrow_mut().insert(mod_name.clone());
        Ok(())
    })?;

    let result = load_and_import_module(&mod_name, alias.as_deref(), only.as_deref(), import_all, env);

    LOADING_MODULES.with(|loading| {
        loading.borrow_mut().remove(&mod_name);
    });

    result
}

fn load_and_import_module(
    mod_name: &str,
    alias: Option<&str>,
    only: Option<&[String]>,
    import_all: bool,
    env: &mut Env,
) -> LispResult {
    let file_path = resolve_module_path(mod_name, env)?;
    let source = std::fs::read_to_string(&file_path)
        .map_err(|e| LispError::new(format!("cannot load module '{}': {}", mod_name, e)))?;

    let exprs = crate::parser::parse(&source)?;
    let mut mod_env = Env::new();
    crate::builtins::register(&mut mod_env);
    crate::prelude::load(&mut mod_env)?;
    for expr in &exprs {
        eval(expr, &mut mod_env)?;
    }

    let exports = get_module_exports(&mod_env);

    if let Some(only_names) = only {
        for name in only_names {
            if !exports.contains(name) {
                return Err(LispError::new(format!(
                    "module '{}' does not export '{}'",
                    mod_name, name
                )));
            }
            if let Ok(val) = mod_env.get(name) {
                env.define(name.clone(), val);
            }
        }
    } else if import_all {
        for name in &exports {
            if let Ok(val) = mod_env.get(name) {
                env.define(name.clone(), val);
            }
        }
    } else {
        let prefix = alias.unwrap_or(mod_name);
        for name in &exports {
            if let Ok(val) = mod_env.get(name) {
                env.define(format!("{}/{}", prefix, name), val);
            }
        }
    }

    Ok(Value::Nil)
}

fn resolve_module_path(name: &str, env: &Env) -> Result<String, LispError> {
    let sep = std::path::MAIN_SEPARATOR;
    let file_name = format!("{}.lisp", name.replace('/', &sep.to_string()));

    // __module_path__ が設定されていればそれを基準にする
    if let Ok(base_val) = env.get("__module_path__") {
        if let Value::Str(base) = &base_val {
            let path = format!("{}{}{}", base, sep, file_name);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        }
    }

    let candidates = vec![
        file_name.clone(),
        format!("src{}{}", sep, file_name),
        format!("lib{}{}", sep, file_name),
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.clone());
        }
    }

    Err(LispError::new(format!(
        "module '{}' not found (tried: {})",
        name,
        candidates.join(", ")
    )))
}

fn get_module_exports(env: &Env) -> Vec<String> {
    if let Ok(exports_val) = env.get("__exports__") {
        if let Value::List(items) = &exports_val {
            return items
                .iter()
                .filter_map(|v| {
                    if let Value::Symbol(s) = v {
                        Some(s.to_string())
                    } else {
                        None
                    }
                })
                .collect();
        }
    }
    vec![]
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
    fn test_eval_quasiquote() {
        assert_eq!(
            eval_str("`(1 2 3)").unwrap(),
            Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn test_eval_unquote() {
        assert_eq!(
            eval_str("(def x 42) `(a ~x b)").unwrap(),
            Value::list(vec![Value::symbol("a"), Value::Int(42), Value::symbol("b")])
        );
    }

    #[test]
    fn test_eval_splice_unquote() {
        assert_eq!(
            eval_str("(def xs '(2 3)) `(1 ~@xs 4)").unwrap(),
            Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)])
        );
    }

    #[test]
    fn test_eval_defmacro() {
        // unless マクロ: (unless cond body) → (if (not cond) body)
        assert_eq!(
            eval_str("(defmacro unless (cond body) `(if (not ~cond) ~body)) (unless false 42)").unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn test_eval_defmacro_when() {
        assert_eq!(
            eval_str("(defmacro when (cond body) `(if ~cond ~body nil)) (when true 99)").unwrap(),
            Value::Int(99)
        );
        assert_eq!(
            eval_str("(defmacro when (cond body) `(if ~cond ~body nil)) (when false 99)").unwrap(),
            Value::Nil
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

    // --- prelude tests ---

    fn eval_with_prelude(input: &str) -> LispResult {
        let mut env = Env::new();
        crate::builtins::register(&mut env);
        crate::prelude::load(&mut env).unwrap();
        let exprs = parse(input).unwrap();
        let mut result = Value::Nil;
        for expr in &exprs {
            result = eval(expr, &mut env)?;
        }
        Ok(result)
    }

    #[test]
    fn test_prelude_map() {
        assert_eq!(
            eval_with_prelude("(map inc '(1 2 3))").unwrap(),
            Value::list(vec![Value::Int(2), Value::Int(3), Value::Int(4)])
        );
    }

    #[test]
    fn test_prelude_filter() {
        assert_eq!(
            eval_with_prelude("(filter even? '(1 2 3 4 5))").unwrap(),
            Value::list(vec![Value::Int(2), Value::Int(4)])
        );
    }

    #[test]
    fn test_prelude_reduce() {
        assert_eq!(
            eval_with_prelude("(reduce + 0 '(1 2 3 4 5))").unwrap(),
            Value::Int(15)
        );
    }

    #[test]
    fn test_prelude_when_unless() {
        assert_eq!(eval_with_prelude("(when true 42)").unwrap(), Value::Int(42));
        assert_eq!(eval_with_prelude("(when false 42)").unwrap(), Value::Nil);
        assert_eq!(eval_with_prelude("(unless false 42)").unwrap(), Value::Int(42));
        assert_eq!(eval_with_prelude("(unless true 42)").unwrap(), Value::Nil);
    }

    #[test]
    fn test_prelude_utilities() {
        assert_eq!(eval_with_prelude("(inc 5)").unwrap(), Value::Int(6));
        assert_eq!(eval_with_prelude("(dec 5)").unwrap(), Value::Int(4));
        assert_eq!(eval_with_prelude("(zero? 0)").unwrap(), Value::Bool(true));
        assert_eq!(eval_with_prelude("(zero? 1)").unwrap(), Value::Bool(false));
        assert_eq!(eval_with_prelude("(even? 4)").unwrap(), Value::Bool(true));
        assert_eq!(eval_with_prelude("(odd? 3)").unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_prelude_range() {
        assert_eq!(
            eval_with_prelude("(range 5)").unwrap(),
            Value::list(vec![Value::Int(0), Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)])
        );
    }

    #[test]
    fn test_prelude_comp() {
        assert_eq!(
            eval_with_prelude("(def inc2 (comp inc inc)) (inc2 5)").unwrap(),
            Value::Int(7)
        );
    }

    #[test]
    fn test_prelude_find() {
        assert_eq!(
            eval_with_prelude("(find even? '(1 3 4 5))").unwrap(),
            Value::Int(4)
        );
        assert_eq!(
            eval_with_prelude("(find even? '(1 3 5))").unwrap(),
            Value::Nil
        );
    }

    #[test]
    fn test_prelude_reject() {
        assert_eq!(
            eval_with_prelude("(reject even? '(1 2 3 4 5))").unwrap(),
            Value::list(vec![Value::Int(1), Value::Int(3), Value::Int(5)])
        );
    }

    #[test]
    fn test_throw_catch() {
        assert_eq!(
            eval_str("(try (throw \"boom\") (catch e e))").unwrap(),
            Value::str("boom")
        );
    }

    #[test]
    fn test_throw_catch_value() {
        assert_eq!(
            eval_str("(try (throw 42) (catch e (+ e 1)))").unwrap(),
            Value::Int(43)
        );
    }

    #[test]
    fn test_try_no_error() {
        assert_eq!(
            eval_str("(try (+ 1 2) (catch e 0))").unwrap(),
            Value::Int(3)
        );
    }

    #[test]
    fn test_try_catches_runtime_error() {
        // undefined symbol is a runtime error
        assert_eq!(
            eval_str("(try (+ undefined 1) (catch e e))").unwrap(),
            Value::str("undefined symbol: undefined")
        );
    }

    #[test]
    fn test_with_all_non_nil() {
        assert_eq!(
            eval_str("(with [a 1 b 2] (+ a b))").unwrap(),
            Value::Int(3)
        );
    }

    #[test]
    fn test_with_short_circuit() {
        assert_eq!(
            eval_str("(with [a nil b 2] (+ a b))").unwrap(),
            Value::Nil
        );
    }

    #[test]
    fn test_with_second_nil() {
        assert_eq!(
            eval_str("(with [a 1 b nil] (+ a b))").unwrap(),
            Value::Nil
        );
    }

    #[test]
    fn test_with_dependent_bindings() {
        assert_eq!(
            eval_str("(with [a 10 b (+ a 5)] b)").unwrap(),
            Value::Int(15)
        );
    }

    #[test]
    fn test_prelude_flatten() {
        assert_eq!(
            eval_with_prelude("(flatten '(1 (2 3) (4 (5))))").unwrap(),
            Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4), Value::Int(5)])
        );
    }

    // --- ns / require tests ---

    #[test]
    fn test_ns_export() {
        // ns + export + defun が動くことを確認
        assert_eq!(
            eval_str("(ns mymod (export add)) (defun add (a b) (+ a b)) (add 1 2)").unwrap(),
            Value::Int(3)
        );
    }

    fn eval_with_module_path(dir: &str, input: &str) -> LispResult {
        let exprs = parse(input).unwrap();
        let mut env = Env::new();
        crate::builtins::register(&mut env);
        env.define("__module_path__", Value::str(dir));
        let mut result = Value::Nil;
        for expr in &exprs {
            result = eval(expr, &mut env)?;
        }
        Ok(result)
    }

    #[test]
    fn test_require_module() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("lisprint_test_require");
        std::fs::create_dir_all(&dir).unwrap();

        let mod_file = dir.join("testmod.lisp");
        let mut f = std::fs::File::create(&mod_file).unwrap();
        writeln!(f, "(ns testmod (export greet))").unwrap();
        writeln!(f, "(defun greet (name) (str \"hello \" name))").unwrap();
        writeln!(f, "(defun internal () 42)").unwrap();

        let result = eval_with_module_path(
            dir.to_str().unwrap(),
            "(require 'testmod) (testmod/greet \"world\")",
        );
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.unwrap(), Value::str("hello world"));
    }

    #[test]
    fn test_require_as() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("lisprint_test_require_as");
        std::fs::create_dir_all(&dir).unwrap();

        let mod_file = dir.join("mymath.lisp");
        let mut f = std::fs::File::create(&mod_file).unwrap();
        writeln!(f, "(ns mymath (export double))").unwrap();
        writeln!(f, "(defun double (n) (* n 2))").unwrap();

        let result = eval_with_module_path(
            dir.to_str().unwrap(),
            "(require 'mymath :as 'm) (m/double 21)",
        );
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.unwrap(), Value::Int(42));
    }

    #[test]
    fn test_require_only() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("lisprint_test_require_only");
        std::fs::create_dir_all(&dir).unwrap();

        let mod_file = dir.join("utils.lisp");
        let mut f = std::fs::File::create(&mod_file).unwrap();
        writeln!(f, "(ns utils (export triple square))").unwrap();
        writeln!(f, "(defun triple (n) (* n 3))").unwrap();
        writeln!(f, "(defun square (n) (* n n))").unwrap();

        let result = eval_with_module_path(
            dir.to_str().unwrap(),
            "(require 'utils :only '(triple)) (triple 4)",
        );
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.unwrap(), Value::Int(12));
    }

    #[test]
    fn test_require_all() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("lisprint_test_require_all");
        std::fs::create_dir_all(&dir).unwrap();

        let mod_file = dir.join("helpers.lisp");
        let mut f = std::fs::File::create(&mod_file).unwrap();
        writeln!(f, "(ns helpers (export add10 add20))").unwrap();
        writeln!(f, "(defun add10 (n) (+ n 10))").unwrap();
        writeln!(f, "(defun add20 (n) (+ n 20))").unwrap();

        let result = eval_with_module_path(
            dir.to_str().unwrap(),
            "(require 'helpers :all) (+ (add10 1) (add20 2))",
        );
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.unwrap(), Value::Int(33));
    }
}
