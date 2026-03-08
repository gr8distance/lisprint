use std::collections::HashMap;

use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, StackSlotData, StackSlotKind, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{default_libcall_names, DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use lisprint_core::value::Value;
use super::typeinfer::{TypeInfer, LType};

/// Runtime value tag constants
pub const TAG_NIL: i64 = 0;
pub const TAG_BOOL: i64 = 1;
pub const TAG_INT: i64 = 2;
pub const TAG_FLOAT: i64 = 3;
pub const TAG_STR: i64 = 4;
pub const TAG_FN: i64 = 5;
pub const TAG_LIST: i64 = 6;
pub const TAG_VEC: i64 = 7;
pub const TAG_MAP: i64 = 8;

/// Tracks local variables within a function being compiled.
/// Each Lisp value is represented as two Cranelift Variables: (tag, payload).
struct FnScope {
    locals: HashMap<String, (Variable, Variable)>,
    next_var: u32,
    /// Inferred types for local variables
    types: HashMap<String, LType>,
}

impl FnScope {
    fn new() -> Self {
        Self {
            locals: HashMap::new(),
            next_var: 0,
            types: HashMap::new(),
        }
    }

    fn declare_var(&mut self, name: &str, builder: &mut FunctionBuilder) -> (Variable, Variable) {
        let tag_var = Variable::from_u32(self.next_var);
        self.next_var += 1;
        let payload_var = Variable::from_u32(self.next_var);
        self.next_var += 1;
        builder.declare_var(tag_var, types::I64);
        builder.declare_var(payload_var, types::I64);
        self.locals.insert(name.to_string(), (tag_var, payload_var));
        (tag_var, payload_var)
    }

    fn get_var(&self, name: &str) -> Option<(Variable, Variable)> {
        self.locals.get(name).copied()
    }
}

/// Cranelift-based compiler for lisprint
pub struct Compiler {
    module: ObjectModule,
    ctx: Context,
    func_ctx: FunctionBuilderContext,
    strings: HashMap<String, DataId>,
    next_str_id: usize,
    /// Declared functions: name → (FuncId, param_count)
    functions: HashMap<String, (FuncId, usize)>,
    next_func_idx: u32,
    /// Bridge (runtime) functions: name → FuncId
    bridges: HashMap<String, FuncId>,
    /// Pending anonymous functions to compile: (generated_name, params, body)
    pending_anons: Vec<(String, Vec<Value>, Vec<Value>)>,
    /// Type inference results: fn_name → (param_types, return_type)
    fn_types: HashMap<String, (Vec<LType>, LType)>,
    /// When true, unknown function calls dispatch through lsp_call_bridge trampoline
    bridge_mode: bool,
}

impl Compiler {
    pub fn new() -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").map_err(|e| e.to_string())?;
        flag_builder.set("opt_level", "speed").map_err(|e| e.to_string())?;

        let isa_builder = cranelift_native::builder()
            .map_err(|e| format!("failed to create ISA builder: {}", e))?;

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| e.to_string())?;

        let object_builder = ObjectBuilder::new(
            isa,
            "lisprint_output",
            default_libcall_names(),
        )
        .map_err(|e| e.to_string())?;

        let module = ObjectModule::new(object_builder);
        let ctx = module.make_context();
        let func_ctx = FunctionBuilderContext::new();

        let mut compiler = Self {
            module,
            ctx,
            func_ctx,
            strings: HashMap::new(),
            next_str_id: 0,
            functions: HashMap::new(),
            next_func_idx: 1,
            bridges: HashMap::new(),
            pending_anons: Vec::new(),
            fn_types: HashMap::new(),
            bridge_mode: false,
        };
        compiler.declare_bridges()?;
        Ok(compiler)
    }

    /// Declare external runtime (bridge) functions
    fn declare_bridges(&mut self) -> Result<(), String> {
        // lsp_println(tag: i64, payload: i64) -> void
        let mut sig_println = self.module.make_signature();
        sig_println.params.push(AbiParam::new(types::I64));
        sig_println.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_println", Linkage::Import, &sig_println)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("println".to_string(), id);

        // lsp_print(tag: i64, payload: i64) -> void
        let mut sig_print = self.module.make_signature();
        sig_print.params.push(AbiParam::new(types::I64));
        sig_print.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_print", Linkage::Import, &sig_print)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("print".to_string(), id);

        // lsp_str_concat(tag1, payload1, tag2, payload2) -> (tag, payload)
        let mut sig_concat = self.module.make_signature();
        for _ in 0..4 {
            sig_concat.params.push(AbiParam::new(types::I64));
        }
        sig_concat.returns.push(AbiParam::new(types::I64));
        sig_concat.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_str_concat", Linkage::Import, &sig_concat)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("str".to_string(), id);

        // lsp_to_string(tag, payload) -> (tag, payload)
        let mut sig_tostr = self.module.make_signature();
        sig_tostr.params.push(AbiParam::new(types::I64));
        sig_tostr.params.push(AbiParam::new(types::I64));
        sig_tostr.returns.push(AbiParam::new(types::I64));
        sig_tostr.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_to_string", Linkage::Import, &sig_tostr)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("to-string".to_string(), id);

        // --- Data structure operations ---
        // Helper: declare a bridge with N tagged params → (tag, payload) return
        let declare_bridge = |module: &mut ObjectModule, name: &str, n_tagged_params: usize|
            -> Result<FuncId, String> {
            let mut sig = module.make_signature();
            for _ in 0..n_tagged_params {
                sig.params.push(AbiParam::new(types::I64)); // tag
                sig.params.push(AbiParam::new(types::I64)); // payload
            }
            sig.returns.push(AbiParam::new(types::I64)); // return tag
            sig.returns.push(AbiParam::new(types::I64)); // return payload
            module.declare_function(name, Linkage::Import, &sig)
                .map_err(|e| e.to_string())
        };

        // lsp_cons(elem_tag, elem_payload, list_tag, list_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_cons", 2)?;
        self.bridges.insert("cons".to_string(), id);

        // lsp_first(coll_tag, coll_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_first", 1)?;
        self.bridges.insert("first".to_string(), id);

        // lsp_rest(coll_tag, coll_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_rest", 1)?;
        self.bridges.insert("rest".to_string(), id);

        // lsp_nth(coll_tag, coll_payload, idx_tag, idx_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_nth", 2)?;
        self.bridges.insert("nth".to_string(), id);

        // lsp_count(coll_tag, coll_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_count", 1)?;
        self.bridges.insert("count".to_string(), id);

        // lsp_empty_q(coll_tag, coll_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_empty_q", 1)?;
        self.bridges.insert("empty?".to_string(), id);

        // lsp_concat: variadic, but we handle 2 collections at a time
        // lsp_concat(a_tag, a_payload, b_tag, b_payload) -> (tag, payload)
        let id = declare_bridge(&mut self.module, "lsp_concat", 2)?;
        self.bridges.insert("concat".to_string(), id);

        Ok(())
    }

    /// Enable bridge mode: unknown function calls dispatch through lsp_call_bridge trampoline
    pub fn set_bridge_mode(&mut self) -> Result<(), String> {
        self.bridge_mode = true;
        // lsp_call_bridge(name_ptr: i64, argc: i64, argv_ptr: i64) -> (tag: i64, payload: i64)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // name_ptr
        sig.params.push(AbiParam::new(types::I64)); // argc
        sig.params.push(AbiParam::new(types::I64)); // argv_ptr
        sig.returns.push(AbiParam::new(types::I64)); // return tag
        sig.returns.push(AbiParam::new(types::I64)); // return payload
        let id = self.module.declare_function("lsp_call_bridge", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("__bridge_call".to_string(), id);
        Ok(())
    }

    fn ensure_string(&mut self, s: &str) -> Result<DataId, String> {
        if let Some(&id) = self.strings.get(s) {
            return Ok(id);
        }

        let name = format!("__str_{}", self.next_str_id);
        self.next_str_id += 1;

        let data_id = self.module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| e.to_string())?;

        let mut desc = DataDescription::new();
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        desc.define(bytes.into_boxed_slice());

        self.module.define_data(data_id, &desc).map_err(|e| e.to_string())?;
        self.strings.insert(s.to_string(), data_id);
        Ok(data_id)
    }

    fn collect_strings(&mut self, exprs: &[Value]) -> Result<(), String> {
        for expr in exprs {
            self.collect_strings_in_expr(expr)?;
        }
        Ok(())
    }

    fn collect_strings_in_expr(&mut self, expr: &Value) -> Result<(), String> {
        match expr {
            Value::Str(s) => { self.ensure_string(s)?; }
            Value::List(items) | Value::Vec(items) => {
                // In bridge mode, collect call-position symbol names as strings
                // (needed for bridge trampoline function name lookup)
                if self.bridge_mode {
                    if let Some(Value::Symbol(sym)) = items.first() {
                        self.ensure_string(sym)?;
                    }
                }
                for item in items.iter() {
                    self.collect_strings_in_expr(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Declare a function signature: N params (each is tag+payload pair) → (tag, payload)
    fn make_fn_sig(&self, param_count: usize) -> cranelift_codegen::ir::Signature {
        let mut sig = self.module.make_signature();
        for _ in 0..param_count {
            sig.params.push(AbiParam::new(types::I64)); // tag
            sig.params.push(AbiParam::new(types::I64)); // payload
        }
        sig.returns.push(AbiParam::new(types::I64)); // return tag
        sig.returns.push(AbiParam::new(types::I64)); // return payload
        sig
    }

    /// Compute a deterministic key for an anonymous fn expression.
    /// Both the pre-declaration pass and emit pass compute the same key
    /// from the expression's structural content.
    fn anon_key(expr: &Value) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let repr = format!("{:?}", expr);
        let mut hasher = DefaultHasher::new();
        repr.hash(&mut hasher);
        format!("__anon_{:016x}", hasher.finish())
    }

    /// Quick type check for an expression using scope type annotations.
    /// Used by emit_arith to skip tag dispatch when types are known.
    fn expr_type(expr: &Value, scope: &FnScope) -> LType {
        match expr {
            Value::Int(_) => LType::Int,
            Value::Float(_) => LType::Float,
            Value::Bool(_) => LType::Bool,
            Value::Str(_) => LType::Str,
            Value::Nil => LType::Nil,
            Value::Symbol(name) => {
                scope.types.get(name.as_str()).cloned().unwrap_or(LType::Any)
            }
            Value::List(items) => {
                if let Some(Value::Symbol(sym)) = items.first() {
                    match sym.as_str() {
                        "+" | "-" | "*" | "/" | "%" if items.len() == 3 => {
                            let lhs = Self::expr_type(&items[1], scope);
                            let rhs = Self::expr_type(&items[2], scope);
                            match (&lhs, &rhs) {
                                (LType::Int, LType::Int) => LType::Int,
                                (LType::Float, LType::Float) => LType::Float,
                                (LType::Int, LType::Float) | (LType::Float, LType::Int) => LType::Float,
                                _ => LType::Any,
                            }
                        }
                        "=" | "<" | ">" | "<=" | ">=" | "!=" | "not" => LType::Bool,
                        "count" => LType::Int,
                        _ => LType::Any,
                    }
                } else {
                    LType::Any
                }
            }
            _ => LType::Any,
        }
    }

    /// Pre-pass: recursively scan all expressions for (fn ...) forms
    /// and declare them in the module.
    fn collect_anon_fns(&mut self, exprs: &[Value]) -> Result<(), String> {
        for expr in exprs {
            self.collect_anon_fns_in_expr(expr)?;
        }
        Ok(())
    }

    fn collect_anon_fns_in_expr(&mut self, expr: &Value) -> Result<(), String> {
        match expr {
            Value::List(items) | Value::Vec(items) => {
                // Check if this is a (fn ...) form
                if let Some(Value::Symbol(sym)) = items.first() {
                    if sym.as_str() == "fn" && items.len() >= 3 {
                        let key = Self::anon_key(expr);
                        if !self.functions.contains_key(&key) {
                            let params = match &items[1] {
                                Value::List(p) | Value::Vec(p) => p.to_vec(),
                                _ => return Err("fn params must be a list".to_string()),
                            };
                            let param_count = params.len();
                            let body = items[2..].to_vec();
                            let sig = self.make_fn_sig(param_count);
                            let func_id = self.module
                                .declare_function(&key, Linkage::Local, &sig)
                                .map_err(|e| e.to_string())?;
                            self.functions.insert(key.clone(), (func_id, param_count));
                            self.pending_anons.push((key, params, body));
                        }
                    }
                }
                // Recurse into children to find nested fn forms
                for item in items.iter() {
                    self.collect_anon_fns_in_expr(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Pre-pass: declare all top-level defun functions in the module
    fn declare_functions(&mut self, exprs: &[Value]) -> Result<(), String> {
        for expr in exprs {
            if let Value::List(items) = expr {
                if items.len() >= 4 {
                    if let Value::Symbol(sym) = &items[0] {
                        if sym.as_str() == "defun" {
                            if let Value::Symbol(name) = &items[1] {
                                // (defun name (params...) body...)
                                if let Value::List(params) = &items[2] {
                                    let param_count = params.len();
                                    let sig = self.make_fn_sig(param_count);
                                    let func_name = format!("_lsp_fn_{}", name);
                                    let func_id = self.module
                                        .declare_function(&func_name, Linkage::Local, &sig)
                                        .map_err(|e| e.to_string())?;
                                    self.functions.insert(name.to_string(), (func_id, param_count));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Compile a defun into a Cranelift function
    fn compile_defun(&mut self, name: &str, params: &[Value], body: &[Value]) -> Result<(), String> {
        let param_names: Vec<String> = params.iter()
            .map(|p| p.as_symbol().map(|s| s.to_string()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let param_count = param_names.len();
        let (func_id, _) = *self.functions.get(name)
            .ok_or_else(|| format!("function {} not declared", name))?;

        let sig = self.make_fn_sig(param_count);
        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, self.next_func_idx);
        self.next_func_idx += 1;

        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.func_ctx);
            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let mut scope = FnScope::new();

            // Set type annotations from type inference
            if let Some((param_types, _)) = self.fn_types.get(name) {
                for (i, param_name) in param_names.iter().enumerate() {
                    if let Some(ty) = param_types.get(i) {
                        scope.types.insert(param_name.clone(), ty.clone());
                    }
                }
            }

            // Bind parameters
            for (i, param_name) in param_names.iter().enumerate() {
                let (tag_var, payload_var) = scope.declare_var(param_name, &mut builder);
                let tag_val = builder.block_params(entry_block)[i * 2];
                let payload_val = builder.block_params(entry_block)[i * 2 + 1];
                builder.def_var(tag_var, tag_val);
                builder.def_var(payload_var, payload_val);
            }

            // Compile body
            let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
            let mut last_payload = builder.ins().iconst(types::I64, 0);

            for expr in body {
                let (tag, payload) = Self::emit_expr(
                    expr,
                    &mut builder,
                    &mut self.module,
                    &self.strings,
                    &self.functions,
                    &mut scope,
                    &self.bridges,
                )?;
                last_tag = tag;
                last_payload = payload;
            }

            builder.ins().return_(&[last_tag, last_payload]);
            builder.finalize();
        }

        self.module.define_function(func_id, &mut self.ctx).map_err(|e| e.to_string())?;
        self.module.clear_context(&mut self.ctx);
        Ok(())
    }

    /// Emit Cranelift IR for an expression, returning (tag, payload)
    fn emit_expr(
        expr: &Value,
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        match expr {
            // Literals
            Value::Nil => {
                let tag = builder.ins().iconst(types::I64, TAG_NIL);
                let payload = builder.ins().iconst(types::I64, 0);
                Ok((tag, payload))
            }
            Value::Bool(b) => {
                let tag = builder.ins().iconst(types::I64, TAG_BOOL);
                let payload = builder.ins().iconst(types::I64, if *b { 1 } else { 0 });
                Ok((tag, payload))
            }
            Value::Int(n) => {
                let tag = builder.ins().iconst(types::I64, TAG_INT);
                let payload = builder.ins().iconst(types::I64, *n);
                Ok((tag, payload))
            }
            Value::Float(f) => {
                let tag = builder.ins().iconst(types::I64, TAG_FLOAT);
                let bits = f.to_bits() as i64;
                let payload = builder.ins().iconst(types::I64, bits);
                Ok((tag, payload))
            }
            Value::Str(s) => {
                let data_id = strings.get(s.as_str())
                    .ok_or_else(|| format!("string not pre-declared: {}", s))?;
                let gv = module.declare_data_in_func(*data_id, builder.func);
                let tag = builder.ins().iconst(types::I64, TAG_STR);
                let payload = builder.ins().global_value(types::I64, gv);
                Ok((tag, payload))
            }

            // Symbol reference (variable lookup)
            Value::Symbol(name) => {
                if let Some((tag_var, payload_var)) = scope.get_var(name) {
                    let tag = builder.use_var(tag_var);
                    let payload = builder.use_var(payload_var);
                    Ok((tag, payload))
                } else {
                    Err(format!("undefined variable: {}", name))
                }
            }

            // List = special form or function call
            Value::List(items) => {
                if items.is_empty() {
                    let tag = builder.ins().iconst(types::I64, TAG_NIL);
                    let payload = builder.ins().iconst(types::I64, 0);
                    return Ok((tag, payload));
                }

                // Check for special forms
                if let Value::Symbol(sym) = &items[0] {
                    // True special forms (cannot be shadowed)
                    match sym.as_str() {
                        "def" => return Self::emit_def(&items[1..], builder, module, strings, functions, scope, bridges),
                        "do" => return Self::emit_do(&items[1..], builder, module, strings, functions, scope, bridges),
                        "if" => return Self::emit_if(&items[1..], builder, module, strings, functions, scope, bridges),
                        "let" => return Self::emit_let(&items[1..], builder, module, strings, functions, scope, bridges),
                        "+" | "-" | "*" | "/" | "%" =>
                            return Self::emit_arith(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges),
                        "=" | "<" | ">" | "<=" | ">=" | "!=" =>
                            return Self::emit_cmp(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges),
                        "not" => return Self::emit_not(&items[1..], builder, module, strings, functions, scope, bridges),
                        "loop" => return Self::emit_loop(&items[1..], builder, module, strings, functions, scope, bridges),
                        "fn" => {
                            // (fn (params...) body...)
                            if items.len() < 3 {
                                return Err("fn requires parameters and body".to_string());
                            }
                            let params = match &items[1] {
                                Value::List(p) | Value::Vec(p) => p.to_vec(),
                                _ => return Err("fn parameters must be a list".to_string()),
                            };
                            let param_count = params.len();

                            // Look up the pre-declared anonymous function by content hash
                            let key = Self::anon_key(expr);
                            let (func_id, _) = *functions.get(&key)
                                .ok_or_else(|| format!("anonymous function not pre-declared: {}", key))?;

                            // Return (TAG_FN, func_pointer | arity << 48)
                            let func_ref = module.declare_func_in_func(func_id, builder.func);
                            let func_addr = builder.ins().func_addr(types::I64, func_ref);
                            // Pack arity into high bits (max 65535 params, plenty)
                            let arity_val = builder.ins().iconst(types::I64, (param_count as i64) << 48);
                            let packed = builder.ins().bor(func_addr, arity_val);
                            let tag = builder.ins().iconst(types::I64, TAG_FN);
                            return Ok((tag, packed));
                        }
                        "defun" => {
                            let tag = builder.ins().iconst(types::I64, TAG_NIL);
                            let payload = builder.ins().iconst(types::I64, 0);
                            return Ok((tag, payload));
                        }
                        _ => {}
                    }

                    // Function call
                    if let Some(&(func_id, param_count)) = functions.get(sym.as_str()) {
                        let args_exprs = &items[1..];
                        if args_exprs.len() != param_count {
                            return Err(format!(
                                "{}: expected {} arguments, got {}",
                                sym, param_count, args_exprs.len()
                            ));
                        }

                        // Evaluate arguments
                        let mut call_args = Vec::new();
                        for arg_expr in args_exprs {
                            let (tag, payload) = Self::emit_expr(
                                arg_expr, builder, module, strings, functions, scope, bridges,
                            )?;
                            call_args.push(tag);
                            call_args.push(payload);
                        }

                        let local_func = module.declare_func_in_func(func_id, builder.func);
                        let call = builder.ins().call(local_func, &call_args);
                        let results = builder.inst_results(call);
                        let ret_tag = results[0];
                        let ret_payload = results[1];
                        return Ok((ret_tag, ret_payload));
                    }

                    // Built-in operations (can be shadowed by user functions)
                    match sym.as_str() {
                        "println" | "print" => return Self::emit_bridge_io(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges),
                        // Data structure operations
                        "list" => return Self::emit_list(&items[1..], builder, module, strings, functions, scope, bridges),
                        "cons" => return Self::emit_bridge_call("cons", &items[1..], 2, builder, module, strings, functions, scope, bridges),
                        "first" => return Self::emit_bridge_call("first", &items[1..], 1, builder, module, strings, functions, scope, bridges),
                        "rest" => return Self::emit_bridge_call("rest", &items[1..], 1, builder, module, strings, functions, scope, bridges),
                        "nth" => return Self::emit_bridge_call("nth", &items[1..], 2, builder, module, strings, functions, scope, bridges),
                        "count" => return Self::emit_bridge_call("count", &items[1..], 1, builder, module, strings, functions, scope, bridges),
                        "empty?" => return Self::emit_bridge_call("empty?", &items[1..], 1, builder, module, strings, functions, scope, bridges),
                        "concat" => return Self::emit_concat(&items[1..], builder, module, strings, functions, scope, bridges),
                        // Type predicates (inline tag checks)
                        "nil?" => return Self::emit_type_pred(TAG_NIL, &items[1..], builder, module, strings, functions, scope, bridges),
                        "number?" => return Self::emit_number_pred(&items[1..], builder, module, strings, functions, scope, bridges),
                        "string?" => return Self::emit_type_pred(TAG_STR, &items[1..], builder, module, strings, functions, scope, bridges),
                        "list?" => return Self::emit_type_pred(TAG_LIST, &items[1..], builder, module, strings, functions, scope, bridges),
                        "fn?" => return Self::emit_type_pred(TAG_FN, &items[1..], builder, module, strings, functions, scope, bridges),
                        _ => {}
                    }

                    // Variable-based function call (call_indirect)
                    if let Some((_, payload_var)) = scope.get_var(sym) {
                        let args_exprs = &items[1..];
                        let mut call_args = Vec::new();
                        for arg_expr in args_exprs {
                            let (tag, payload) = Self::emit_expr(
                                arg_expr, builder, module, strings, functions, scope, bridges,
                            )?;
                            call_args.push(tag);
                            call_args.push(payload);
                        }

                        let fn_payload = builder.use_var(payload_var);
                        // Extract function address (low 48 bits)
                        let addr_mask = builder.ins().iconst(types::I64, 0x0000_FFFF_FFFF_FFFFu64 as i64);
                        let func_addr = builder.ins().band(fn_payload, addr_mask);

                        // Create signature for indirect call
                        let mut sig = module.make_signature();
                        for _ in 0..args_exprs.len() {
                            sig.params.push(AbiParam::new(types::I64)); // tag
                            sig.params.push(AbiParam::new(types::I64)); // payload
                        }
                        sig.returns.push(AbiParam::new(types::I64)); // return tag
                        sig.returns.push(AbiParam::new(types::I64)); // return payload
                        let sig_ref = builder.import_signature(sig);

                        let call = builder.ins().call_indirect(sig_ref, func_addr, &call_args);
                        let results = builder.inst_results(call);
                        let ret_tag = results[0];
                        let ret_payload = results[1];
                        return Ok((ret_tag, ret_payload));
                    }

                    // Bridge trampoline for unknown function calls (bridge mode only)
                    if let Some(&bridge_call_id) = bridges.get("__bridge_call") {
                        let args_exprs = &items[1..];
                        let argc = args_exprs.len();

                        // Evaluate all arguments
                        let mut arg_vals = Vec::new();
                        for arg_expr in args_exprs {
                            let (tag, payload) = Self::emit_expr(
                                arg_expr, builder, module, strings, functions, scope, bridges,
                            )?;
                            arg_vals.push((tag, payload));
                        }

                        // Get function name as string constant
                        let name_data = strings.get(sym.as_str())
                            .ok_or_else(|| format!("bridge name string not found: {}", sym))?;
                        let name_gv = module.declare_data_in_func(*name_data, builder.func);
                        let name_ptr = builder.ins().global_value(types::I64, name_gv);

                        let argc_val = builder.ins().iconst(types::I64, argc as i64);
                        let argv_ptr = if argc > 0 {
                            let slot_size = (argc * 16) as u32;
                            let slot = builder.create_sized_stack_slot(
                                StackSlotData::new(StackSlotKind::ExplicitSlot, slot_size, 3)
                            );
                            for (i, (tag, payload)) in arg_vals.iter().enumerate() {
                                builder.ins().stack_store(*tag, slot, (i * 16) as i32);
                                builder.ins().stack_store(*payload, slot, (i * 16 + 8) as i32);
                            }
                            builder.ins().stack_addr(types::I64, slot, 0)
                        } else {
                            builder.ins().iconst(types::I64, 0)
                        };

                        let local_func = module.declare_func_in_func(bridge_call_id, builder.func);
                        let call = builder.ins().call(local_func, &[name_ptr, argc_val, argv_ptr]);
                        let results = builder.inst_results(call);
                        return Ok((results[0], results[1]));
                    }
                }

                Err(format!("cannot compile call: {}", items[0]))
            }

            _ => Err(format!("cannot compile: {}", expr.type_name())),
        }
    }

    /// (def name value)
    fn emit_def(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err("def requires 2 arguments (name value)".to_string());
        }
        let name = args[0].as_symbol().map_err(|e| e.to_string())?;
        let (tag, payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges)?;
        let (tag_var, payload_var) = scope.declare_var(name, builder);
        builder.def_var(tag_var, tag);
        builder.def_var(payload_var, payload);
        Ok((tag, payload))
    }

    /// (if cond then else?)
    fn emit_if(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() < 2 || args.len() > 3 {
            return Err("if requires 2 or 3 arguments".to_string());
        }

        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;

        // Truthy: not nil (tag!=0) and not false (tag==1 && payload==0)
        // Falsy: nil (tag==0) OR (tag==1 AND payload==0)
        let is_nil = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_tag, TAG_NIL);
        let is_bool = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_tag, TAG_BOOL);
        let is_false_val = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_payload, 0);
        let is_false_bool = builder.ins().band(is_bool, is_false_val);
        let is_falsy = builder.ins().bor(is_nil, is_false_bool);

        let then_block = builder.create_block();
        let else_block = builder.create_block();
        let merge_block = builder.create_block();

        builder.append_block_param(merge_block, types::I64); // result tag
        builder.append_block_param(merge_block, types::I64); // result payload

        builder.ins().brif(is_falsy, else_block, &[], then_block, &[]);

        // Then branch
        builder.switch_to_block(then_block);
        builder.seal_block(then_block);
        let (then_tag, then_payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges)?;
        builder.ins().jump(merge_block, &[then_tag, then_payload]);

        // Else branch
        builder.switch_to_block(else_block);
        builder.seal_block(else_block);
        let (else_tag, else_payload) = if args.len() == 3 {
            Self::emit_expr(&args[2], builder, module, strings, functions, scope, bridges)?
        } else {
            let t = builder.ins().iconst(types::I64, TAG_NIL);
            let p = builder.ins().iconst(types::I64, 0);
            (t, p)
        };
        builder.ins().jump(merge_block, &[else_tag, else_payload]);

        // Merge
        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        let result_tag = builder.block_params(merge_block)[0];
        let result_payload = builder.block_params(merge_block)[1];
        Ok((result_tag, result_payload))
    }

    /// (let (bindings...) body...)
    fn emit_let(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.is_empty() {
            return Err("let requires bindings and body".to_string());
        }

        let bindings = match &args[0] {
            Value::List(items) | Value::Vec(items) => items.to_vec(),
            _ => return Err("let bindings must be a list or vector".to_string()),
        };

        if bindings.len() % 2 != 0 {
            return Err("let bindings must have even number of elements".to_string());
        }

        for chunk in bindings.chunks(2) {
            let name = chunk[0].as_symbol().map_err(|e| e.to_string())?;
            let (tag, payload) = Self::emit_expr(&chunk[1], builder, module, strings, functions, scope, bridges)?;
            let (tag_var, payload_var) = scope.declare_var(name, builder);
            builder.def_var(tag_var, tag);
            builder.def_var(payload_var, payload);
        }

        let body = &args[1..];
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in body {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges)?;
            last_tag = tag;
            last_payload = payload;
        }
        Ok((last_tag, last_payload))
    }

    /// Arithmetic: +, -, *, /, %
    /// Handles both Int and Float operands with type coercion.
    /// When type inference determines both operands are Int, skips tag dispatch entirely.
    fn emit_arith(
        op: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err(format!("{} requires 2 arguments", op));
        }

        // Type-inference fast path: if both operands are known-Int, skip tag dispatch
        let lhs_type = Self::expr_type(&args[0], scope);
        let rhs_type = Self::expr_type(&args[1], scope);

        if lhs_type.is_known_int() && rhs_type.is_known_int() {
            let (_, lhs_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;
            let (_, rhs_payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges)?;
            let result = match op {
                "+" => builder.ins().iadd(lhs_payload, rhs_payload),
                "-" => builder.ins().isub(lhs_payload, rhs_payload),
                "*" => builder.ins().imul(lhs_payload, rhs_payload),
                "/" => builder.ins().sdiv(lhs_payload, rhs_payload),
                "%" => builder.ins().srem(lhs_payload, rhs_payload),
                _ => unreachable!(),
            };
            let tag = builder.ins().iconst(types::I64, TAG_INT);
            return Ok((tag, result));
        }

        let (lhs_tag, lhs_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;
        let (rhs_tag, rhs_payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges)?;

        // Check if either operand is float
        let lhs_is_float = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, lhs_tag, TAG_FLOAT);
        let rhs_is_float = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, rhs_tag, TAG_FLOAT);
        let any_float = builder.ins().bor(lhs_is_float, rhs_is_float);

        let int_block = builder.create_block();
        let float_block = builder.create_block();
        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64); // result tag
        builder.append_block_param(merge_block, types::I64); // result payload

        builder.ins().brif(any_float, float_block, &[], int_block, &[]);

        // Integer path
        builder.switch_to_block(int_block);
        builder.seal_block(int_block);
        let int_result = match op {
            "+" => builder.ins().iadd(lhs_payload, rhs_payload),
            "-" => builder.ins().isub(lhs_payload, rhs_payload),
            "*" => builder.ins().imul(lhs_payload, rhs_payload),
            "/" => builder.ins().sdiv(lhs_payload, rhs_payload),
            "%" => builder.ins().srem(lhs_payload, rhs_payload),
            _ => unreachable!(),
        };
        let int_tag = builder.ins().iconst(types::I64, TAG_INT);
        builder.ins().jump(merge_block, &[int_tag, int_result]);

        // Float path: convert both to f64, operate, convert back to bits
        builder.switch_to_block(float_block);
        builder.seal_block(float_block);

        // Convert lhs: if int, convert to f64; if float, bitcast
        let lhs_f = Self::emit_to_f64(lhs_tag, lhs_payload, builder);
        let rhs_f = Self::emit_to_f64(rhs_tag, rhs_payload, builder);

        let float_result = match op {
            "+" => builder.ins().fadd(lhs_f, rhs_f),
            "-" => builder.ins().fsub(lhs_f, rhs_f),
            "*" => builder.ins().fmul(lhs_f, rhs_f),
            "/" => builder.ins().fdiv(lhs_f, rhs_f),
            "%" => {
                // f64 modulo: a - floor(a/b) * b
                let div = builder.ins().fdiv(lhs_f, rhs_f);
                let floored = builder.ins().floor(div);
                let prod = builder.ins().fmul(floored, rhs_f);
                builder.ins().fsub(lhs_f, prod)
            }
            _ => unreachable!(),
        };
        let float_bits = builder.ins().bitcast(types::I64, cranelift_codegen::ir::MemFlags::new(), float_result);
        let float_tag = builder.ins().iconst(types::I64, TAG_FLOAT);
        builder.ins().jump(merge_block, &[float_tag, float_bits]);

        // Merge
        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        let result_tag = builder.block_params(merge_block)[0];
        let result_payload = builder.block_params(merge_block)[1];
        Ok((result_tag, result_payload))
    }

    /// Convert a tagged value to f64: if int, fcvt_from_sint; if already float, bitcast
    fn emit_to_f64(
        tag: cranelift_codegen::ir::Value,
        payload: cranelift_codegen::ir::Value,
        builder: &mut FunctionBuilder,
    ) -> cranelift_codegen::ir::Value {
        let is_int = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, tag, TAG_INT);

        let int_block = builder.create_block();
        let float_block = builder.create_block();
        let merge = builder.create_block();
        builder.append_block_param(merge, types::F64);

        builder.ins().brif(is_int, int_block, &[], float_block, &[]);

        builder.switch_to_block(int_block);
        builder.seal_block(int_block);
        let f_from_int = builder.ins().fcvt_from_sint(types::F64, payload);
        builder.ins().jump(merge, &[f_from_int]);

        builder.switch_to_block(float_block);
        builder.seal_block(float_block);
        let f_from_bits = builder.ins().bitcast(types::F64, cranelift_codegen::ir::MemFlags::new(), payload);
        builder.ins().jump(merge, &[f_from_bits]);

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        builder.block_params(merge)[0]
    }

    /// Comparison: =, <, >, <=, >=, !=
    fn emit_cmp(
        op: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err(format!("{} requires 2 arguments", op));
        }
        let (_, lhs) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;
        let (_, rhs) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges)?;

        use cranelift_codegen::ir::condcodes::IntCC;
        let cc = match op {
            "=" => IntCC::Equal,
            "!=" => IntCC::NotEqual,
            "<" => IntCC::SignedLessThan,
            ">" => IntCC::SignedGreaterThan,
            "<=" => IntCC::SignedLessThanOrEqual,
            ">=" => IntCC::SignedGreaterThanOrEqual,
            _ => unreachable!(),
        };

        let cmp_result = builder.ins().icmp(cc, lhs, rhs);
        let tag = builder.ins().iconst(types::I64, TAG_BOOL);
        let payload = builder.ins().uextend(types::I64, cmp_result);
        Ok((tag, payload))
    }

    /// (not expr)
    fn emit_not(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err("not requires 1 argument".to_string());
        }
        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;

        // Falsy = nil or false
        let is_nil = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_tag, TAG_NIL);
        let is_bool = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_tag, TAG_BOOL);
        let is_false_val = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_payload, 0);
        let is_false_bool = builder.ins().band(is_bool, is_false_val);
        let is_falsy = builder.ins().bor(is_nil, is_false_bool);
        let tag = builder.ins().iconst(types::I64, TAG_BOOL);
        let payload = builder.ins().uextend(types::I64, is_falsy);
        Ok((tag, payload))
    }

    /// (println expr) / (print expr) — bridge call to runtime
    fn emit_bridge_io(
        name: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err(format!("{} requires 1 argument", name));
        }
        let (tag, payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;
        let bridge_id = bridges.get(name)
            .ok_or_else(|| format!("bridge function {} not found", name))?;
        let local_func = module.declare_func_in_func(*bridge_id, builder.func);
        builder.ins().call(local_func, &[tag, payload]);

        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let nil_payload = builder.ins().iconst(types::I64, 0);
        Ok((nil_tag, nil_payload))
    }

    /// Generic bridge call: evaluate N args, call bridge function, return (tag, payload)
    fn emit_bridge_call(
        bridge_name: &str,
        args: &[Value],
        expected_args: usize,
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != expected_args {
            return Err(format!("{} requires {} argument(s)", bridge_name, expected_args));
        }
        let mut call_args = Vec::new();
        for arg in args {
            let (tag, payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges)?;
            call_args.push(tag);
            call_args.push(payload);
        }
        let bridge_id = bridges.get(bridge_name)
            .ok_or_else(|| format!("bridge function {} not found", bridge_name))?;
        let local_func = module.declare_func_in_func(*bridge_id, builder.func);
        let call = builder.ins().call(local_func, &call_args);
        let results = builder.inst_results(call);
        Ok((results[0], results[1]))
    }

    /// (list a b c ...) → chain of cons calls: cons(a, cons(b, cons(c, nil)))
    fn emit_list(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        // Evaluate all arguments first
        let mut evaluated = Vec::new();
        for arg in args {
            let (tag, payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges)?;
            evaluated.push((tag, payload));
        }

        // Build list from right to left: cons(last, nil), cons(second-to-last, ...), etc.
        let mut result_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut result_payload = builder.ins().iconst(types::I64, 0);

        let cons_id = bridges.get("cons")
            .ok_or_else(|| "bridge function cons not found".to_string())?;

        for (elem_tag, elem_payload) in evaluated.into_iter().rev() {
            let local_func = module.declare_func_in_func(*cons_id, builder.func);
            let call = builder.ins().call(local_func, &[elem_tag, elem_payload, result_tag, result_payload]);
            let results = builder.inst_results(call);
            result_tag = results[0];
            result_payload = results[1];
        }

        Ok((result_tag, result_payload))
    }

    /// (concat a b c ...) → pairwise concat calls
    fn emit_concat(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.is_empty() {
            let tag = builder.ins().iconst(types::I64, TAG_NIL);
            let payload = builder.ins().iconst(types::I64, 0);
            return Ok((tag, payload));
        }

        let (mut result_tag, mut result_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;

        let concat_id = bridges.get("concat")
            .ok_or_else(|| "bridge function concat not found".to_string())?;

        for arg in &args[1..] {
            let (b_tag, b_payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges)?;
            let local_func = module.declare_func_in_func(*concat_id, builder.func);
            let call = builder.ins().call(local_func, &[result_tag, result_payload, b_tag, b_payload]);
            let results = builder.inst_results(call);
            result_tag = results[0];
            result_payload = results[1];
        }

        Ok((result_tag, result_payload))
    }

    /// Type predicate: check if tag matches a specific constant
    fn emit_type_pred(
        expected_tag: i64,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err("type predicate requires 1 argument".to_string());
        }
        let (val_tag, _val_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;
        let matches = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, val_tag, expected_tag);
        let tag = builder.ins().iconst(types::I64, TAG_BOOL);
        let payload = builder.ins().uextend(types::I64, matches);
        Ok((tag, payload))
    }

    /// (number? x) — true if Int or Float
    fn emit_number_pred(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err("number? requires 1 argument".to_string());
        }
        let (val_tag, _val_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;
        let is_int = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, val_tag, TAG_INT);
        let is_float = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, val_tag, TAG_FLOAT);
        let is_number = builder.ins().bor(is_int, is_float);
        let tag = builder.ins().iconst(types::I64, TAG_BOOL);
        let payload = builder.ins().uextend(types::I64, is_number);
        Ok((tag, payload))
    }

    /// (loop [bindings...] body)
    /// Bindings: [name1 init1 name2 init2 ...]
    /// body may contain (recur new1 new2 ...) to jump back
    fn emit_loop(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() < 2 {
            return Err("loop requires bindings and body".to_string());
        }

        let bindings = match &args[0] {
            Value::List(items) | Value::Vec(items) => items.to_vec(),
            _ => return Err("loop bindings must be a vector".to_string()),
        };
        if bindings.len() % 2 != 0 {
            return Err("loop bindings must have even number of elements".to_string());
        }

        // Collect binding names and evaluate initial values
        let mut bind_names = Vec::new();
        let mut init_tags = Vec::new();
        let mut init_payloads = Vec::new();
        for chunk in bindings.chunks(2) {
            let name = chunk[0].as_symbol().map_err(|e| e.to_string())?;
            bind_names.push(name.to_string());
            let (tag, payload) = Self::emit_expr(&chunk[1], builder, module, strings, functions, scope, bridges)?;
            init_tags.push(tag);
            init_payloads.push(payload);
        }

        // Create loop header block with params for each binding (tag + payload)
        let loop_block = builder.create_block();
        for _ in 0..bind_names.len() {
            builder.append_block_param(loop_block, types::I64); // tag
            builder.append_block_param(loop_block, types::I64); // payload
        }

        // Create exit block with result params
        let exit_block = builder.create_block();
        builder.append_block_param(exit_block, types::I64); // result tag
        builder.append_block_param(exit_block, types::I64); // result payload

        // Jump to loop header with initial values
        let mut jump_args = Vec::new();
        for i in 0..bind_names.len() {
            jump_args.push(init_tags[i]);
            jump_args.push(init_payloads[i]);
        }
        builder.ins().jump(loop_block, &jump_args);

        // Switch to loop block
        builder.switch_to_block(loop_block);

        // Bind loop params to scope variables
        let block_params: Vec<cranelift_codegen::ir::Value> = builder.block_params(loop_block).to_vec();
        for (i, name) in bind_names.iter().enumerate() {
            let (tag_var, payload_var) = scope.declare_var(name, builder);
            builder.def_var(tag_var, block_params[i * 2]);
            builder.def_var(payload_var, block_params[i * 2 + 1]);
        }

        let body = &args[1..];
        let (result_tag, result_payload, terminated) = Self::emit_loop_body(
            body, builder, module, strings, functions, scope, bridges,
            loop_block, exit_block, &bind_names,
        )?;

        // If body didn't end with recur, jump to exit
        if !terminated {
            builder.ins().jump(exit_block, &[result_tag, result_payload]);
        }

        // Seal loop block now that all jumps to it are known
        builder.seal_block(loop_block);

        // Switch to exit block
        builder.switch_to_block(exit_block);
        builder.seal_block(exit_block);

        let exit_tag = builder.block_params(exit_block)[0];
        let exit_payload = builder.block_params(exit_block)[1];
        Ok((exit_tag, exit_payload))
    }

    /// Compile loop body, handling recur specially
    /// Returns (tag, payload, terminated) where terminated=true means recur was emitted
    fn emit_loop_body(
        exprs: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        loop_block: cranelift_codegen::ir::Block,
        exit_block: cranelift_codegen::ir::Block,
        bind_names: &[String],
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value, bool), String> {
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);

        for expr in exprs {
            let (tag, payload, terminated) = Self::emit_loop_expr(
                expr, builder, module, strings, functions, scope, bridges,
                loop_block, exit_block, bind_names,
            )?;
            last_tag = tag;
            last_payload = payload;
            if terminated {
                return Ok((last_tag, last_payload, true));
            }
        }
        Ok((last_tag, last_payload, false))
    }

    /// Compile a single expression inside a loop, with recur and if-recur support
    /// Returns (tag, payload, terminated)
    fn emit_loop_expr(
        expr: &Value,
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        loop_block: cranelift_codegen::ir::Block,
        exit_block: cranelift_codegen::ir::Block,
        bind_names: &[String],
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value, bool), String> {
        if let Value::List(items) = expr {
            if let Some(Value::Symbol(sym)) = items.first() {
                match sym.as_str() {
                    "recur" => {
                        let recur_args = &items[1..];
                        if recur_args.len() != bind_names.len() {
                            return Err(format!(
                                "recur: expected {} arguments, got {}",
                                bind_names.len(),
                                recur_args.len()
                            ));
                        }
                        let mut jump_args = Vec::new();
                        for arg in recur_args {
                            let (tag, payload) = Self::emit_expr(
                                arg, builder, module, strings, functions, scope, bridges,
                            )?;
                            jump_args.push(tag);
                            jump_args.push(payload);
                        }
                        builder.ins().jump(loop_block, &jump_args);
                        // Block is terminated
                        return Ok((jump_args[0], jump_args[1], true));
                    }
                    "if" => {
                        // Special if handling inside loop to support recur in branches
                        return Self::emit_loop_if(
                            &items[1..], builder, module, strings, functions, scope, bridges,
                            loop_block, exit_block, bind_names,
                        );
                    }
                    _ => {}
                }
            }
        }
        // Not recur or special if — delegate to normal emit_expr
        let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges)?;
        Ok((tag, payload, false))
    }

    /// (if cond then else) inside a loop — branches may contain recur
    /// Returns (tag, payload, terminated) — terminated only if BOTH branches recur
    fn emit_loop_if(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        loop_block: cranelift_codegen::ir::Block,
        exit_block: cranelift_codegen::ir::Block,
        bind_names: &[String],
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value, bool), String> {
        if args.len() < 2 || args.len() > 3 {
            return Err("if requires 2 or 3 arguments".to_string());
        }

        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges)?;

        let is_nil = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_tag, TAG_NIL);
        let is_bool = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_tag, TAG_BOOL);
        let is_false_val = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond_payload, 0);
        let is_false_bool = builder.ins().band(is_bool, is_false_val);
        let is_falsy = builder.ins().bor(is_nil, is_false_bool);

        let then_block = builder.create_block();
        let else_block = builder.create_block();
        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64);
        builder.append_block_param(merge_block, types::I64);

        builder.ins().brif(is_falsy, else_block, &[], then_block, &[]);

        // Then branch (may contain recur)
        builder.switch_to_block(then_block);
        builder.seal_block(then_block);
        let (then_tag, then_payload, then_terminated) = Self::emit_loop_expr(
            &args[1], builder, module, strings, functions, scope, bridges,
            loop_block, exit_block, bind_names,
        )?;
        if !then_terminated {
            builder.ins().jump(merge_block, &[then_tag, then_payload]);
        }

        // Else branch (may contain recur)
        builder.switch_to_block(else_block);
        builder.seal_block(else_block);
        let (else_tag, else_payload, else_terminated) = if args.len() == 3 {
            Self::emit_loop_expr(
                &args[2], builder, module, strings, functions, scope, bridges,
                loop_block, exit_block, bind_names,
            )?
        } else {
            let t = builder.ins().iconst(types::I64, TAG_NIL);
            let p = builder.ins().iconst(types::I64, 0);
            (t, p, false)
        };
        if !else_terminated {
            builder.ins().jump(merge_block, &[else_tag, else_payload]);
        }

        // If both branches terminated (both recur'd), merge block is unreachable
        // but we still need to switch to it for Cranelift
        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);

        if then_terminated && else_terminated {
            // Both branches jumped back to loop — this merge is unreachable
            // but we need valid values. The block will be eliminated by optimizer.
            let t = builder.ins().iconst(types::I64, TAG_NIL);
            let p = builder.ins().iconst(types::I64, 0);
            builder.ins().jump(exit_block, &[t, p]);
            Ok((t, p, true))
        } else {
            let result_tag = builder.block_params(merge_block)[0];
            let result_payload = builder.block_params(merge_block)[1];
            Ok((result_tag, result_payload, false))
        }
    }

    /// (do expr1 expr2 ...)
    fn emit_do(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in args {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges)?;
            last_tag = tag;
            last_payload = payload;
        }
        Ok((last_tag, last_payload))
    }

    /// Compile all expressions into an object file.
    pub fn compile_exprs(mut self, exprs: &[Value]) -> Result<Vec<u8>, String> {
        if exprs.is_empty() {
            return Err("nothing to compile".to_string());
        }

        // Pass 1: collect strings
        self.collect_strings(exprs)?;

        // Pass 2: declare all top-level functions
        self.declare_functions(exprs)?;

        // Pass 2.5: scan for anonymous fn expressions and declare them
        self.collect_anon_fns(exprs)?;

        // Pass 2.75: type inference
        let mut type_infer = TypeInfer::new();
        type_infer.infer_program(exprs);
        self.fn_types = type_infer.into_fn_types();

        // Pass 3: compile each defun
        // Collect defun info first to avoid borrow issues
        let defuns: Vec<(String, Vec<Value>, Vec<Value>)> = exprs.iter().filter_map(|expr| {
            if let Value::List(items) = expr {
                if items.len() >= 4 {
                    if let (Value::Symbol(sym), Value::Symbol(name)) = (&items[0], &items[1]) {
                        if sym.as_str() == "defun" {
                            if let Value::List(params) = &items[2] {
                                let body = items[3..].to_vec();
                                return Some((name.to_string(), params.to_vec(), body));
                            }
                        }
                    }
                }
            }
            None
        }).collect();

        for (name, params, body) in &defuns {
            self.compile_defun(name, params, body)?;
        }

        // Pass 3.5: compile pending anonymous functions
        let pending = std::mem::take(&mut self.pending_anons);
        for (name, params, body) in &pending {
            self.compile_defun(&name, params, body)?;
        }

        // Pass 4: compile _lsp_main (non-defun top-level expressions)
        let main_exprs: Vec<&Value> = exprs.iter().filter(|expr| {
            if let Value::List(items) = expr {
                if let Some(Value::Symbol(sym)) = items.first() {
                    return sym.as_str() != "defun";
                }
            }
            true
        }).collect();

        let sig = self.make_fn_sig(0);
        let func_id = self.module
            .declare_function("_lsp_main", Linkage::Export, &sig)
            .map_err(|e| e.to_string())?;

        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, 0);

        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.func_ctx);
            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let mut scope = FnScope::new();
            let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
            let mut last_payload = builder.ins().iconst(types::I64, 0);

            for expr in &main_exprs {
                let (tag, payload) = Self::emit_expr(
                    expr,
                    &mut builder,
                    &mut self.module,
                    &self.strings,
                    &self.functions,
                    &mut scope,
                    &self.bridges,
                )?;
                last_tag = tag;
                last_payload = payload;
            }

            builder.ins().return_(&[last_tag, last_payload]);
            builder.finalize();
        }

        self.module.define_function(func_id, &mut self.ctx).map_err(|e| e.to_string())?;
        self.module.clear_context(&mut self.ctx);

        let product = self.module.finish();
        let bytes = product.emit().map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisprint_core::parser::parse;

    #[test]
    fn test_compiler_creation() {
        assert!(Compiler::new().is_ok());
    }

    #[test]
    fn test_compile_int_literal() {
        let exprs = parse("42").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_float_literal() {
        let exprs = parse("3.14").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_bool_literal() {
        let exprs = parse("true").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_nil_literal() {
        let exprs = parse("nil").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_string_literal() {
        let exprs = parse("\"hello world\"").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_multiple_exprs() {
        let exprs = parse("1 2 3").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_def() {
        let exprs = parse("(def x 42) x").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_defun_and_call() {
        let exprs = parse("(defun answer () 42) (answer)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_defun_with_params() {
        let exprs = parse("(defun identity (x) x) (identity 99)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_defun_multi_param() {
        let exprs = parse("(defun first (a b) a) (first 1 2)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_do() {
        let exprs = parse("(do 1 2 3)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_defun_recursive() {
        let exprs = parse("(defun f (x) x) (f 5)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_if() {
        let exprs = parse("(if true 1 2)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_if_no_else() {
        let exprs = parse("(if true 42)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_if_nil() {
        let exprs = parse("(if nil 1 2)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_let() {
        let exprs = parse("(let (x 10 y 20) (+ x y))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_arithmetic() {
        let exprs = parse("(+ 1 2)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());

        let exprs = parse("(- 10 3)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());

        let exprs = parse("(* 4 5)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());

        let exprs = parse("(/ 10 2)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_comparison() {
        let exprs = parse("(= 1 1)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());

        let exprs = parse("(< 1 2)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());

        let exprs = parse("(> 3 1)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_not() {
        let exprs = parse("(not true)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_factorial() {
        // Recursive factorial
        let exprs = parse("(defun fact (n) (if (= n 0) 1 (* n (fact (- n 1))))) (fact 5)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_println() {
        let exprs = parse("(println 42)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_println_string() {
        let exprs = parse("(println \"hello\")").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_fibonacci() {
        let exprs = parse("(defun fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 10)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_loop_recur() {
        // Factorial via loop/recur
        let exprs = parse("(loop [i 5 acc 1] (if (= i 0) acc (recur (- i 1) (* acc i))))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_loop_fib_fast() {
        // Fibonacci via loop/recur
        let exprs = parse("(defun fib-fast (n) (loop [i n a 0 b 1] (if (= i 0) a (recur (- i 1) b (+ a b))))) (fib-fast 10)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_float_arith() {
        let exprs = parse("(+ 1.5 2.5)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_mixed_arith() {
        // Int + Float should promote to Float
        let exprs = parse("(+ 1 2.5)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_anon_fn() {
        // Anonymous function as a value
        let exprs = parse("(def f (fn (x) x))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_anon_fn_multi_param() {
        let exprs = parse("(def add (fn (a b) (+ a b)))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_anon_fn_call_indirect() {
        // Higher-order function: pass fn as argument, call via variable
        let exprs = parse("(defun apply1 (f x) (f x)) (apply1 (fn (x) (+ x 1)) 10)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_anon_fn_call_two_args() {
        let exprs = parse("(defun apply2 (f a b) (f a b)) (apply2 (fn (a b) (+ a b)) 3 4)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_anon_fn_in_let() {
        // fn bound in let, then called
        let exprs = parse("(let (f (fn (x) (* x 2))) (f 5))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    // --- Data structure tests ---

    #[test]
    fn test_compile_list_empty() {
        let exprs = parse("(list)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_list_with_elements() {
        let exprs = parse("(list 1 2 3)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_cons() {
        let exprs = parse("(cons 1 (list 2 3))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_first() {
        let exprs = parse("(first (list 1 2 3))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_rest() {
        let exprs = parse("(rest (list 1 2 3))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_nth() {
        let exprs = parse("(nth (list 10 20 30) 1)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_count() {
        let exprs = parse("(count (list 1 2 3))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_empty_pred() {
        let exprs = parse("(empty? (list))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_concat() {
        let exprs = parse("(concat (list 1 2) (list 3 4))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_concat_multi() {
        let exprs = parse("(concat (list 1) (list 2) (list 3))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    // --- Type predicate tests ---

    #[test]
    fn test_compile_nil_pred() {
        let exprs = parse("(nil? nil)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_number_pred() {
        let exprs = parse("(number? 42)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_string_pred() {
        let exprs = parse("(string? \"hello\")").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_list_pred() {
        let exprs = parse("(list? (list 1 2))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_fn_pred() {
        let exprs = parse("(fn? (fn (x) x))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_list_with_higher_order() {
        // Build a list, pass to a function that uses first/rest
        let exprs = parse("(defun head (lst) (first lst)) (head (list 10 20 30))").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }
}
