//! Type inference pass for the lisprint compiler.
//!
//! Analyzes the AST to determine static types where possible.
//! When types are known at compile time, the compiler can emit
//! unboxed operations (e.g., direct iadd instead of tag-dispatched arithmetic).

use std::collections::HashMap;
use lisprint_core::value::Value;

/// Inferred type for a compile-time expression
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LType {
    /// Type is known at compile time
    Int,
    Float,
    Bool,
    Str,
    Nil,
    List,
    Fn,
    /// Numeric: either Int or Float (useful for arithmetic results)
    Num,
    /// Type is unknown / dynamic — must use tagged values
    Any,
}

impl LType {
    pub fn is_known_int(&self) -> bool {
        matches!(self, LType::Int)
    }

    pub fn is_known_numeric(&self) -> bool {
        matches!(self, LType::Int | LType::Float | LType::Num)
    }
}

/// Type environment for a function scope
#[derive(Debug, Clone)]
struct TypeScope {
    vars: HashMap<String, LType>,
}

impl TypeScope {
    fn new() -> Self {
        Self { vars: HashMap::new() }
    }

    fn set(&mut self, name: &str, ty: LType) {
        self.vars.insert(name.to_string(), ty);
    }

    fn get(&self, name: &str) -> LType {
        self.vars.get(name).cloned().unwrap_or(LType::Any)
    }
}

/// Type inference context
pub struct TypeInfer {
    /// Function signatures: name → (param_types, return_type)
    fn_types: HashMap<String, (Vec<LType>, LType)>,
}

impl TypeInfer {
    pub fn new() -> Self {
        Self {
            fn_types: HashMap::new(),
        }
    }

    /// Convert a type annotation string to LType
    fn type_name_to_ltype(name: &str) -> Option<LType> {
        match name {
            "i64" | "int" => Some(LType::Int),
            "f64" | "float" => Some(LType::Float),
            "bool" => Some(LType::Bool),
            "string" | "str" => Some(LType::Str),
            "nil" => Some(LType::Nil),
            "list" => Some(LType::List),
            "fn" => Some(LType::Fn),
            _ => None, // Unknown type (future: UserType)
        }
    }

    /// Infer types for all top-level expressions.
    /// Returns a map from function name → (param_types, return_type).
    pub fn infer_program(&mut self, exprs: &[Value]) -> &HashMap<String, (Vec<LType>, LType)> {
        // Pass 1: collect function signatures with Any types
        for expr in exprs {
            if let Value::List(items) = expr {
                if items.len() >= 4 {
                    if let (Value::Symbol(sym), Value::Symbol(raw_name)) = (&items[0], &items[1]) {
                        if sym.as_str() == "defun" {
                            if let Value::List(params) = &items[2] {
                                let (fn_name, _ret_ann) = Value::parse_type_ann(raw_name);
                                let param_types: Vec<LType> = params.iter().map(|p| {
                                    if let Ok(s) = p.as_symbol() {
                                        let (_, type_ann) = Value::parse_type_ann(s);
                                        type_ann.and_then(Self::type_name_to_ltype).unwrap_or(LType::Any)
                                    } else {
                                        LType::Any
                                    }
                                }).collect();
                                let ret_type = _ret_ann.and_then(Self::type_name_to_ltype).unwrap_or(LType::Any);
                                self.fn_types.insert(fn_name.to_string(), (param_types, ret_type));
                            }
                        }
                    }
                }
            }
        }

        // Pass 2: infer types within each function body
        // We do multiple passes until types stabilize
        for _ in 0..3 {
            let fn_types_snapshot = self.fn_types.clone();
            for expr in exprs {
                if let Value::List(items) = expr {
                    if items.len() >= 4 {
                        if let (Value::Symbol(sym), Value::Symbol(raw_name)) = (&items[0], &items[1]) {
                            if sym.as_str() == "defun" {
                                if let Value::List(params) = &items[2] {
                                    let (fn_name, _) = Value::parse_type_ann(raw_name);
                                    let body = &items[3..];
                                    let (param_types, _) = fn_types_snapshot.get(fn_name)
                                        .cloned()
                                        .unwrap_or((vec![LType::Any; params.len()], LType::Any));

                                    let mut scope = TypeScope::new();
                                    for (i, param) in params.iter().enumerate() {
                                        if let Ok(psym) = param.as_symbol() {
                                            let (pname, _) = Value::parse_type_ann(psym);
                                            scope.set(pname, param_types.get(i).cloned().unwrap_or(LType::Any));
                                        }
                                    }

                                    let ret_type = self.infer_body(body, &mut scope);
                                    self.fn_types.insert(fn_name.to_string(), (param_types, ret_type));
                                }
                            }
                        }
                    }
                }
            }

            // Pass 3: refine parameter types from call sites
            for expr in exprs {
                self.refine_from_calls(expr);
            }
        }

        &self.fn_types
    }

    /// Infer the type of a body (sequence of expressions)
    fn infer_body(&self, exprs: &[Value], scope: &mut TypeScope) -> LType {
        let mut last = LType::Nil;
        for expr in exprs {
            last = self.infer_expr(expr, scope);
        }
        last
    }

    /// Infer the type of a single expression
    fn infer_expr(&self, expr: &Value, scope: &mut TypeScope) -> LType {
        match expr {
            Value::Nil => LType::Nil,
            Value::Bool(_) => LType::Bool,
            Value::Int(_) => LType::Int,
            Value::Float(_) => LType::Float,
            Value::Str(_) => LType::Str,
            Value::Symbol(name) => scope.get(name),
            Value::List(items) => {
                if items.is_empty() {
                    return LType::Nil;
                }
                if let Value::Symbol(sym) = &items[0] {
                    match sym.as_str() {
                        "def" => {
                            if items.len() == 3 {
                                let ty = self.infer_expr(&items[2], scope);
                                if let Ok(name) = items[1].as_symbol() {
                                    scope.set(name, ty.clone());
                                }
                                return ty;
                            }
                            LType::Any
                        }
                        "do" => self.infer_body(&items[1..], scope),
                        "if" => {
                            if items.len() >= 3 {
                                let then_ty = self.infer_expr(&items[2], scope);
                                let else_ty = if items.len() >= 4 {
                                    self.infer_expr(&items[3], scope)
                                } else {
                                    LType::Nil
                                };
                                return Self::unify(&then_ty, &else_ty);
                            }
                            LType::Any
                        }
                        "let" => {
                            if items.len() >= 2 {
                                if let Value::List(bindings) | Value::Vec(bindings) = &items[1] {
                                    for chunk in bindings.chunks(2) {
                                        if chunk.len() == 2 {
                                            let ty = self.infer_expr(&chunk[1], scope);
                                            if let Ok(name) = chunk[0].as_symbol() {
                                                scope.set(name, ty);
                                            }
                                        }
                                    }
                                }
                                return self.infer_body(&items[2..], scope);
                            }
                            LType::Any
                        }
                        "+" | "-" | "*" | "/" | "%" => {
                            if items.len() == 3 {
                                let lhs = self.infer_expr(&items[1], scope);
                                let rhs = self.infer_expr(&items[2], scope);
                                return Self::arith_result(&lhs, &rhs);
                            }
                            LType::Any
                        }
                        "=" | "<" | ">" | "<=" | ">=" | "!=" | "not" => LType::Bool,
                        "nil?" | "number?" | "string?" | "list?" | "fn?" | "empty?" => LType::Bool,
                        "println" | "print" => LType::Nil,
                        "loop" => {
                            // Loop return type is complex; conservatively Any
                            // Could be refined by analyzing the non-recur exit paths
                            if items.len() >= 3 {
                                if let Value::List(bindings) | Value::Vec(bindings) = &items[1] {
                                    for chunk in bindings.chunks(2) {
                                        if chunk.len() == 2 {
                                            let ty = self.infer_expr(&chunk[1], scope);
                                            if let Ok(name) = chunk[0].as_symbol() {
                                                scope.set(name, ty);
                                            }
                                        }
                                    }
                                }
                                return self.infer_loop_body(&items[2..], scope);
                            }
                            LType::Any
                        }
                        "fn" => LType::Fn,
                        "defun" => LType::Nil,
                        "list" | "cons" | "rest" | "concat" => LType::List,
                        "first" | "nth" => LType::Any, // element type unknown
                        "count" => LType::Int,
                        _ => {
                            // Function call — return type from fn_types
                            if let Some((_, ret_type)) = self.fn_types.get(sym.as_str()) {
                                return ret_type.clone();
                            }
                            LType::Any
                        }
                    }
                } else {
                    LType::Any
                }
            }
            _ => LType::Any,
        }
    }

    /// Infer loop body type (the exit value, not recur)
    fn infer_loop_body(&self, exprs: &[Value], scope: &mut TypeScope) -> LType {
        let mut last = LType::Nil;
        for expr in exprs {
            last = self.infer_loop_expr(expr, scope);
        }
        last
    }

    fn infer_loop_expr(&self, expr: &Value, scope: &mut TypeScope) -> LType {
        if let Value::List(items) = expr {
            if let Some(Value::Symbol(sym)) = items.first() {
                match sym.as_str() {
                    "recur" => return LType::Any, // recur doesn't produce a value
                    "if" => {
                        if items.len() >= 3 {
                            let then_ty = self.infer_loop_expr(&items[2], scope);
                            let else_ty = if items.len() >= 4 {
                                self.infer_loop_expr(&items[3], scope)
                            } else {
                                LType::Nil
                            };
                            // If one branch is recur (Any), use the other
                            if then_ty == LType::Any { return else_ty; }
                            if else_ty == LType::Any { return then_ty; }
                            return Self::unify(&then_ty, &else_ty);
                        }
                    }
                    _ => {}
                }
            }
        }
        self.infer_expr(expr, scope)
    }

    /// Refine function parameter types from call sites
    fn refine_from_calls(&mut self, expr: &Value) {
        let mut scope = TypeScope::new();
        self.refine_from_calls_scoped(expr, &mut scope);
    }

    fn refine_from_calls_scoped(&mut self, expr: &Value, scope: &mut TypeScope) {
        if let Value::List(items) = expr {
            if let Some(Value::Symbol(sym)) = items.first() {
                match sym.as_str() {
                    "defun" if items.len() >= 4 => {
                        // Set up scope with function parameters
                        let mut fn_scope = TypeScope::new();
                        if let (Value::Symbol(name), Value::List(params)) = (&items[1], &items[2]) {
                            if let Some((param_types, _)) = self.fn_types.get(name.as_str()).cloned() {
                                for (i, p) in params.iter().enumerate() {
                                    if let Ok(pname) = p.as_symbol() {
                                        fn_scope.set(pname, param_types.get(i).cloned().unwrap_or(LType::Any));
                                    }
                                }
                            }
                        }
                        for item in &items[3..] {
                            self.refine_from_calls_scoped(item, &mut fn_scope);
                        }
                        return;
                    }
                    "def" if items.len() == 3 => {
                        if let Ok(name) = items[1].as_symbol() {
                            let ty = self.infer_expr(&items[2], scope);
                            scope.set(name, ty);
                        }
                    }
                    "let" if items.len() >= 2 => {
                        if let Value::List(bindings) | Value::Vec(bindings) = &items[1] {
                            for chunk in bindings.chunks(2) {
                                if chunk.len() == 2 {
                                    let ty = self.infer_expr(&chunk[1], scope);
                                    if let Ok(name) = chunk[0].as_symbol() {
                                        scope.set(name, ty);
                                    }
                                }
                            }
                        }
                    }
                    "loop" if items.len() >= 3 => {
                        if let Value::List(bindings) | Value::Vec(bindings) = &items[1] {
                            for chunk in bindings.chunks(2) {
                                if chunk.len() == 2 {
                                    let ty = self.infer_expr(&chunk[1], scope);
                                    if let Ok(name) = chunk[0].as_symbol() {
                                        scope.set(name, ty);
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // Function call: refine parameter types
                        if let Some((param_types, ret_type)) = self.fn_types.get(sym.as_str()).cloned() {
                            let args = &items[1..];
                            if args.len() == param_types.len() {
                                let mut new_params = param_types.clone();
                                for (i, arg) in args.iter().enumerate() {
                                    let arg_type = self.infer_expr(arg, scope);
                                    if arg_type != LType::Any {
                                        new_params[i] = Self::unify(&new_params[i], &arg_type);
                                    }
                                }
                                self.fn_types.insert(sym.to_string(), (new_params, ret_type));
                            }
                        }
                    }
                }
            }
            // Recurse into sub-expressions
            for item in items.iter() {
                self.refine_from_calls_scoped(item, scope);
            }
        }
    }

    /// Unify two types: if both are the same, return that; otherwise Any
    fn unify(a: &LType, b: &LType) -> LType {
        if a == b {
            return a.clone();
        }
        match (a, b) {
            (LType::Any, other) | (other, LType::Any) => other.clone(),
            (LType::Int, LType::Float) | (LType::Float, LType::Int) => LType::Num,
            (LType::Num, LType::Int) | (LType::Int, LType::Num) => LType::Num,
            (LType::Num, LType::Float) | (LType::Float, LType::Num) => LType::Num,
            _ => LType::Any,
        }
    }

    /// Result type of arithmetic on two types
    fn arith_result(lhs: &LType, rhs: &LType) -> LType {
        match (lhs, rhs) {
            (LType::Int, LType::Int) => LType::Int,
            (LType::Float, LType::Float) => LType::Float,
            (LType::Int, LType::Float) | (LType::Float, LType::Int) => LType::Float,
            (LType::Num, _) | (_, LType::Num) => LType::Num,
            _ => LType::Any,
        }
    }

    /// Get the inferred type info for a function
    pub fn get_fn_type(&self, name: &str) -> Option<&(Vec<LType>, LType)> {
        self.fn_types.get(name)
    }

    /// Consume self and return the fn_types map
    pub fn into_fn_types(self) -> HashMap<String, (Vec<LType>, LType)> {
        self.fn_types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisprint_core::parser::parse;

    #[test]
    fn test_infer_simple_arith() {
        let exprs = parse("(defun add (a b) (+ a b)) (add 1 2)").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (params, ret) = ti.get_fn_type("add").unwrap();
        assert_eq!(params, &[LType::Int, LType::Int]);
        assert_eq!(*ret, LType::Int);
    }

    #[test]
    fn test_infer_fib() {
        let exprs = parse("(defun fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 10)").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (params, ret) = ti.get_fn_type("fib").unwrap();
        assert_eq!(params, &[LType::Int]);
        assert_eq!(*ret, LType::Int);
    }

    #[test]
    fn test_infer_factorial() {
        let exprs = parse("(defun fact (n) (if (= n 0) 1 (* n (fact (- n 1))))) (fact 5)").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (params, ret) = ti.get_fn_type("fact").unwrap();
        assert_eq!(params, &[LType::Int]);
        assert_eq!(*ret, LType::Int);
    }

    #[test]
    fn test_infer_float_arith() {
        let exprs = parse("(defun fadd (a b) (+ a b)) (fadd 1.0 2.0)").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (params, ret) = ti.get_fn_type("fadd").unwrap();
        assert_eq!(params, &[LType::Float, LType::Float]);
        assert_eq!(*ret, LType::Float);
    }

    #[test]
    fn test_infer_bool_result() {
        let exprs = parse("(defun is-zero (n) (= n 0)) (is-zero 5)").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (_, ret) = ti.get_fn_type("is-zero").unwrap();
        assert_eq!(*ret, LType::Bool);
    }

    #[test]
    fn test_infer_let_binding() {
        let exprs = parse("(defun f (x) (let (y (+ x 1)) y)) (f 10)").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (params, ret) = ti.get_fn_type("f").unwrap();
        assert_eq!(params, &[LType::Int]);
        assert_eq!(*ret, LType::Int);
    }

    #[test]
    fn test_infer_string() {
        let exprs = parse("(defun greet (name) name) (greet \"world\")").unwrap();
        let mut ti = TypeInfer::new();
        ti.infer_program(&exprs);
        let (params, _) = ti.get_fn_type("greet").unwrap();
        assert_eq!(params, &[LType::Str]);
    }
}
