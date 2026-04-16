use std::collections::HashMap;

use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{default_libcall_names, DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use lisprint_core::builtins;
use lisprint_core::env::Env;
use lisprint_core::eval;
use lisprint_core::prelude;
use lisprint_core::value::Value;

/// Runtime value tag constants
pub const TAG_NIL: i64 = 0;
pub const TAG_BOOL: i64 = 1;
pub const TAG_INT: i64 = 2;
pub const TAG_FLOAT: i64 = 3;
pub const TAG_STR: i64 = 4;
pub const TAG_LIST: i64 = 5;
pub const TAG_FN: i64 = 6;

use cranelift_codegen::ir::Block;

/// Tracks local variables within a function being compiled.
/// Each Lisp value is represented as two Cranelift Variables: (tag, payload).
struct FnScope {
    locals: HashMap<String, (Variable, Variable)>,
    next_var: u32,
    /// Active loop context for recur: (header_block, binding_names)
    loop_ctx: Option<(Block, Vec<String>)>,
}

impl FnScope {
    fn new() -> Self {
        Self {
            locals: HashMap::new(),
            next_var: 0,
            loop_ctx: None,
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

/// Info about a lambda function collected during the pre-pass.
struct LambdaInfo {
    /// Unique name for this lambda
    name: String,
    /// Parameter names
    params: Vec<String>,
    /// Body expressions
    body: Vec<Value>,
    /// Captured variable names (free variables from enclosing scope)
    captures: Vec<String>,
}

/// Cranelift-based compiler for lisprint
pub struct Compiler {
    module: ObjectModule,
    ctx: Context,
    func_ctx: FunctionBuilderContext,
    strings: HashMap<String, DataId>,
    next_str_id: usize,
    /// Declared functions: name → list of (FuncId, param_count) for multi-arity support
    functions: HashMap<String, Vec<(FuncId, usize)>>,
    next_func_idx: u32,
    /// Bridge (runtime) functions: name → FuncId
    bridges: HashMap<String, FuncId>,
    /// Lambda functions: name → (FuncId, param_count, capture_count)
    lambdas: HashMap<String, (FuncId, usize, usize)>,
    #[allow(dead_code)]
    next_lambda_id: usize,
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
            lambdas: HashMap::new(),
            next_lambda_id: 0,
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

        // --- Error handling ---
        // lsp_throw(tag, payload) -> void (sets global error)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_throw", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("throw".to_string(), id);

        // lsp_has_error() -> i64 (1 if error, 0 if not)
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_has_error", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("has_error".to_string(), id);

        // lsp_get_error() -> (tag, payload)
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_get_error", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("get_error".to_string(), id);

        // lsp_clear_error() -> void
        let sig = self.module.make_signature();
        let id = self.module.declare_function("lsp_clear_error", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("clear_error".to_string(), id);

        // --- List operations ---
        // All list bridge functions: (tag, payload)* -> (tag, payload)

        // lsp_list_new(count: i64, elements: *const i64) -> (tag, payload)
        // elements is a pointer to array of [tag, payload, tag, payload, ...]
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // count
        sig.params.push(AbiParam::new(types::I64)); // elements ptr
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_list_new", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("list_new".to_string(), id);

        // lsp_cons(tag, payload, list_tag, list_payload) -> (tag, payload)
        let mut sig = self.module.make_signature();
        for _ in 0..4 { sig.params.push(AbiParam::new(types::I64)); }
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_cons", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("cons".to_string(), id);

        // lsp_first(tag, payload) -> (tag, payload)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_first", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("first".to_string(), id);

        // lsp_rest(tag, payload) -> (tag, payload)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_rest", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("rest".to_string(), id);

        // lsp_count(tag, payload) -> (tag, payload)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_count", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("count".to_string(), id);

        // lsp_nth(list_tag, list_payload, idx_tag, idx_payload) -> (tag, payload)
        let mut sig = self.module.make_signature();
        for _ in 0..4 { sig.params.push(AbiParam::new(types::I64)); }
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_nth", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("nth".to_string(), id);

        // lsp_empty(tag, payload) -> (tag, payload)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_empty", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("empty?".to_string(), id);

        // lsp_concat(tag1, payload1, tag2, payload2) -> (tag, payload)
        let mut sig = self.module.make_signature();
        for _ in 0..4 { sig.params.push(AbiParam::new(types::I64)); }
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("lsp_concat", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.bridges.insert("concat".to_string(), id);

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

    /// Pre-pass: declare all top-level defun functions in the module
    fn declare_functions(&mut self, exprs: &[Value]) -> Result<(), String> {
        for expr in exprs {
            if let Value::List(items) = expr {
                if items.len() >= 3 {
                    if let Value::Symbol(sym) = &items[0] {
                        if sym.as_str() == "defun" {
                            if let Value::Symbol(name) = &items[1] {
                                // Check if multi-arity: (defun name ((params1) body1) ((params2) body2) ...)
                                let is_multi = if let Value::List(first_clause) = &items[2] {
                                    !first_clause.is_empty() && matches!(&first_clause[0], Value::List(_) | Value::Vec(_))
                                } else {
                                    false
                                };

                                if is_multi {
                                    for (arity_idx, clause) in items[2..].iter().enumerate() {
                                        if let Value::List(clause_items) = clause {
                                            if let Some(Value::List(params)) = clause_items.first() {
                                                let param_count = params.len();
                                                let sig = self.make_fn_sig(param_count);
                                                let func_name = format!("_lsp_fn_{}_{}", name, param_count);
                                                let func_id = self.module
                                                    .declare_function(&func_name, Linkage::Local, &sig)
                                                    .map_err(|e| e.to_string())?;
                                                self.functions.entry(name.to_string())
                                                    .or_default()
                                                    .push((func_id, param_count));
                                                let _ = arity_idx;
                                            }
                                        }
                                    }
                                } else if let Value::List(params) = &items[2] {
                                    // Single arity: (defun name (params...) body...)
                                    let param_count = params.len();
                                    let sig = self.make_fn_sig(param_count);
                                    let func_name = format!("_lsp_fn_{}", name);
                                    let func_id = self.module
                                        .declare_function(&func_name, Linkage::Local, &sig)
                                        .map_err(|e| e.to_string())?;
                                    self.functions.insert(name.to_string(), vec![(func_id, param_count)]);
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
        let arities = self.functions.get(name)
            .ok_or_else(|| format!("function {} not declared", name))?;
        let func_id = arities.iter()
            .find(|(_, pc)| *pc == param_count)
            .map(|(fid, _)| *fid)
            .ok_or_else(|| format!("function {}: no arity for {} params", name, param_count))?;

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
                    &self.lambdas,
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

    /// Compile a lambda function. Signature: (env_ptr, params...) → (tag, payload)
    fn compile_lambda(&mut self, info: &LambdaInfo) -> Result<(), String> {
        let (func_id, _, _) = *self.lambdas.get(&info.name)
            .ok_or_else(|| format!("lambda {} not declared", info.name))?;

        let param_count = info.params.len();
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // env_ptr
        for _ in 0..param_count {
            sig.params.push(AbiParam::new(types::I64)); // tag
            sig.params.push(AbiParam::new(types::I64)); // payload
        }
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));

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

            // First param is env_ptr (closure captures)
            let env_ptr = builder.block_params(entry_block)[0];

            // Load captured variables from env_ptr
            // Layout: [cap0_tag, cap0_payload, cap1_tag, cap1_payload, ...]
            for (i, cap_name) in info.captures.iter().enumerate() {
                let (tag_var, payload_var) = scope.declare_var(cap_name, &mut builder);
                let offset_tag = (i * 2 * 8) as i32;
                let offset_payload = (i * 2 * 8 + 8) as i32;
                let tag_val = builder.ins().load(types::I64, cranelift_codegen::ir::MemFlags::new(), env_ptr, offset_tag);
                let payload_val = builder.ins().load(types::I64, cranelift_codegen::ir::MemFlags::new(), env_ptr, offset_payload);
                builder.def_var(tag_var, tag_val);
                builder.def_var(payload_var, payload_val);
            }

            // Bind regular parameters (offset by 1 for env_ptr)
            for (i, param_name) in info.params.iter().enumerate() {
                let (tag_var, payload_var) = scope.declare_var(param_name, &mut builder);
                let tag_val = builder.block_params(entry_block)[1 + i * 2];
                let payload_val = builder.block_params(entry_block)[1 + i * 2 + 1];
                builder.def_var(tag_var, tag_val);
                builder.def_var(payload_var, payload_val);
            }

            // Compile body
            let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
            let mut last_payload = builder.ins().iconst(types::I64, 0);
            for expr in &info.body {
                let (tag, payload) = Self::emit_expr(
                    expr,
                    &mut builder,
                    &mut self.module,
                    &self.strings,
                    &self.functions,
                    &mut scope,
                    &self.bridges,
                    &self.lambdas,
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
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
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
                    match sym.as_str() {
                        "def" => return Self::emit_def(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "do" => return Self::emit_do(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "if" => return Self::emit_if(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "let" => return Self::emit_let(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "+" | "-" | "*" | "/" | "%" =>
                            return Self::emit_arith(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "=" | "<" | ">" | "<=" | ">=" | "!=" =>
                            return Self::emit_cmp(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "not" => return Self::emit_not(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "loop" => return Self::emit_loop(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "recur" => return Self::emit_recur(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "list" => return Self::emit_list_literal(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "match" => return Self::emit_match(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "throw" => return Self::emit_throw(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "try" => return Self::emit_try(&items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "println" | "print" => return Self::emit_bridge_io(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges, lambdas),
                        "defun" => {
                            let tag = builder.ins().iconst(types::I64, TAG_NIL);
                            let payload = builder.ins().iconst(types::I64, 0);
                            return Ok((tag, payload));
                        }
                        "__make_closure" => {
                            return Self::emit_make_closure(&items[1..], builder, module, scope, lambdas);
                        }
                        _ => {}
                    }

                    // Function call — find matching arity
                    if let Some(arities) = functions.get(sym.as_str()) {
                        let args_exprs = &items[1..];
                        let arg_count = args_exprs.len();
                        let func_id = arities.iter()
                            .find(|(_, pc)| *pc == arg_count)
                            .map(|(fid, _)| *fid)
                            .ok_or_else(|| format!(
                                "{}: no matching arity for {} arguments",
                                sym, arg_count
                            ))?;

                        // Evaluate arguments
                        let mut call_args = Vec::new();
                        for arg_expr in args_exprs {
                            let (tag, payload) = Self::emit_expr(
                                arg_expr, builder, module, strings, functions, scope, bridges, lambdas,
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

                    // Bridge call fallback (builtins: cons, first, rest, count, nth, empty?, concat, str)
                    if bridges.contains_key(sym.as_str()) {
                        return Self::emit_bridge_call(sym.as_str(), &items[1..], builder, module, strings, functions, scope, bridges, lambdas);
                    }

                    // Closure call: variable holding a function value
                    if scope.get_var(sym).is_some() {
                        return Self::emit_closure_call(sym, &items[1..], builder, module, strings, functions, scope, bridges, lambdas);
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
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err("def requires 2 arguments (name value)".to_string());
        }
        let name = args[0].as_symbol().map_err(|e| e.to_string())?;
        let (tag, payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges, lambdas)?;
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
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() < 2 || args.len() > 3 {
            return Err("if requires 2 or 3 arguments".to_string());
        }

        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;

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
        let (then_tag, then_payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges, lambdas)?;
        builder.ins().jump(merge_block, &[then_tag, then_payload]);

        // Else branch
        builder.switch_to_block(else_block);
        builder.seal_block(else_block);
        let (else_tag, else_payload) = if args.len() == 3 {
            Self::emit_expr(&args[2], builder, module, strings, functions, scope, bridges, lambdas)?
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
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
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
            let (tag, payload) = Self::emit_expr(&chunk[1], builder, module, strings, functions, scope, bridges, lambdas)?;
            Self::emit_destructure_bind(&chunk[0], tag, payload, builder, module, scope, bridges, lambdas)?;
        }

        let body = &args[1..];
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in body {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges, lambdas)?;
            last_tag = tag;
            last_payload = payload;
        }
        Ok((last_tag, last_payload))
    }

    /// Bind a destructuring pattern to a compiled value (tag, payload).
    fn emit_destructure_bind(
        pattern: &Value,
        val_tag: cranelift_codegen::ir::Value,
        val_payload: cranelift_codegen::ir::Value,
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        #[allow(unused)] lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(), String> {
        match pattern {
            // Simple symbol binding
            Value::Symbol(name) => {
                let (tag_var, payload_var) = scope.declare_var(name, builder);
                builder.def_var(tag_var, val_tag);
                builder.def_var(payload_var, val_payload);
                Ok(())
            }
            // Vector destructuring: [a b c]
            Value::Vec(items) => {
                let nth_id = bridges.get("nth")
                    .ok_or_else(|| "bridge function nth not found".to_string())?;
                for (i, item) in items.iter().enumerate() {
                    let idx_tag = builder.ins().iconst(types::I64, TAG_INT);
                    let idx_payload = builder.ins().iconst(types::I64, i as i64);
                    let local_func = module.declare_func_in_func(*nth_id, builder.func);
                    let call = builder.ins().call(local_func, &[val_tag, val_payload, idx_tag, idx_payload]);
                    let results = builder.inst_results(call);
                    let elem_tag = results[0];
                    let elem_payload = results[1];
                    // Recursively bind (supports nested destructuring)
                    Self::emit_destructure_bind(item, elem_tag, elem_payload, builder, module, scope, bridges, lambdas)?;
                }
                Ok(())
            }
            // Map destructuring: {:keys [x y]} — keys style
            Value::Map(map) => {
                // For now, support {:keys [sym1 sym2 ...]} pattern
                // Keys are looked up by name from the map
                if let Some(Value::Vec(keys)) = map.get("keys") {
                    // :keys pattern — each key name is used to look up from the map
                    // We use the same nth approach but this needs map bridge support
                    // For now, just bind each key as a variable with nil (placeholder)
                    for key in keys.iter() {
                        if let Value::Symbol(name) = key {
                            let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
                            let nil_payload = builder.ins().iconst(types::I64, 0);
                            let (tag_var, payload_var) = scope.declare_var(name, builder);
                            builder.def_var(tag_var, nil_tag);
                            builder.def_var(payload_var, nil_payload);
                        }
                    }
                }
                // TODO: full map destructuring with runtime bridge
                Ok(())
            }
            _ => Err(format!("invalid destructuring pattern: {}", pattern)),
        }
    }

    /// Arithmetic: +, -, *, /, % — supports 2+ arguments via left fold
    fn emit_arith(
        op: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() < 2 {
            return Err(format!("{} requires at least 2 arguments", op));
        }
        let (_, mut acc) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;

        for arg in &args[1..] {
            let (_, rhs) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges, lambdas)?;
            acc = match op {
                "+" => builder.ins().iadd(acc, rhs),
                "-" => builder.ins().isub(acc, rhs),
                "*" => builder.ins().imul(acc, rhs),
                "/" => builder.ins().sdiv(acc, rhs),
                "%" => builder.ins().srem(acc, rhs),
                _ => unreachable!(),
            };
        }

        let tag = builder.ins().iconst(types::I64, TAG_INT);
        Ok((tag, acc))
    }

    /// Comparison: =, <, >, <=, >=, !=
    fn emit_cmp(
        op: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err(format!("{} requires 2 arguments", op));
        }
        let (_, lhs) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;
        let (_, rhs) = Self::emit_expr(&args[1], builder, module, strings, functions, scope, bridges, lambdas)?;

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
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err("not requires 1 argument".to_string());
        }
        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;

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
    /// (throw value) — set global error state
    fn emit_throw(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.is_empty() {
            return Err("throw requires 1 argument".to_string());
        }
        let (tag, payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;
        let throw_id = bridges.get("throw")
            .ok_or_else(|| "bridge function throw not found".to_string())?;
        let local_func = module.declare_func_in_func(*throw_id, builder.func);
        builder.ins().call(local_func, &[tag, payload]);

        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let nil_payload = builder.ins().iconst(types::I64, 0);
        Ok((nil_tag, nil_payload))
    }

    /// (try body (catch e handler))
    fn emit_try(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        // (try body (catch e handler))
        // Find the catch clause
        let mut body_exprs = Vec::new();
        let mut catch_var = None;
        let mut catch_body = None;

        for arg in args {
            if let Value::List(items) = arg {
                if items.len() >= 3 {
                    if let Value::Symbol(sym) = &items[0] {
                        if sym.as_str() == "catch" {
                            if let Value::Symbol(var_name) = &items[1] {
                                catch_var = Some(var_name.to_string());
                                catch_body = Some(items[2..].to_vec());
                                continue;
                            }
                        }
                    }
                }
            }
            body_exprs.push(arg);
        }

        let catch_var = catch_var.ok_or_else(|| "try requires a catch clause".to_string())?;
        let catch_body = catch_body.ok_or_else(|| "try requires a catch body".to_string())?;

        // Clear any existing error
        let clear_id = bridges.get("clear_error")
            .ok_or_else(|| "bridge function clear_error not found".to_string())?;
        let clear_func = module.declare_func_in_func(*clear_id, builder.func);
        builder.ins().call(clear_func, &[]);

        // Compile body
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in &body_exprs {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges, lambdas)?;
            last_tag = tag;
            last_payload = payload;
        }

        // Check if error occurred
        let has_err_id = bridges.get("has_error")
            .ok_or_else(|| "bridge function has_error not found".to_string())?;
        let has_err_func = module.declare_func_in_func(*has_err_id, builder.func);
        let has_err_call = builder.ins().call(has_err_func, &[]);
        let has_err = builder.inst_results(has_err_call)[0];
        let is_err = builder.ins().icmp_imm(
            cranelift_codegen::ir::condcodes::IntCC::NotEqual,
            has_err, 0,
        );

        let catch_block = builder.create_block();
        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64);
        builder.append_block_param(merge_block, types::I64);

        builder.ins().brif(is_err, catch_block, &[], merge_block, &[last_tag, last_payload]);

        // Catch block
        builder.switch_to_block(catch_block);
        builder.seal_block(catch_block);

        // Get error value and bind to catch variable
        let get_err_id = bridges.get("get_error")
            .ok_or_else(|| "bridge function get_error not found".to_string())?;
        let get_err_func = module.declare_func_in_func(*get_err_id, builder.func);
        let get_err_call = builder.ins().call(get_err_func, &[]);
        let err_results = builder.inst_results(get_err_call);
        let err_tag = err_results[0];
        let err_payload = err_results[1];

        let (err_tag_var, err_payload_var) = scope.declare_var(&catch_var, builder);
        builder.def_var(err_tag_var, err_tag);
        builder.def_var(err_payload_var, err_payload);

        // Clear error
        let clear_func2 = module.declare_func_in_func(*clear_id, builder.func);
        builder.ins().call(clear_func2, &[]);

        // Compile catch body
        let mut catch_last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut catch_last_payload = builder.ins().iconst(types::I64, 0);
        for expr in &catch_body {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges, lambdas)?;
            catch_last_tag = tag;
            catch_last_payload = payload;
        }
        builder.ins().jump(merge_block, &[catch_last_tag, catch_last_payload]);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        let result_tag = builder.block_params(merge_block)[0];
        let result_payload = builder.block_params(merge_block)[1];
        Ok((result_tag, result_payload))
    }

    /// (match value pattern1 expr1 pattern2 expr2 ...)
    /// Compiles to a chain of if-else blocks checking each pattern.
    fn emit_match(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() < 3 || args.len() % 2 != 1 {
            return Err("match requires a value and pattern/expr pairs".to_string());
        }

        // Evaluate target
        let (target_tag, target_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;

        // Create merge block for the final result
        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64);
        builder.append_block_param(merge_block, types::I64);

        let pairs = &args[1..];
        let num_pairs = pairs.len() / 2;

        for (i, chunk) in pairs.chunks(2).enumerate() {
            let pattern = &chunk[0];
            let body = &chunk[1];
            let is_last = i == num_pairs - 1;

            match pattern {
                // Wildcard _ or symbol binding — always matches
                Value::Symbol(s) if s.as_str() == "_" => {
                    // Wildcard: just compile the body
                    let (tag, payload) = Self::emit_expr(body, builder, module, strings, functions, scope, bridges, lambdas)?;
                    builder.ins().jump(merge_block, &[tag, payload]);
                    // No more patterns after wildcard
                    break;
                }
                Value::Symbol(s) => {
                    // Binding: bind target to variable, compile body
                    let (tag_var, payload_var) = scope.declare_var(s, builder);
                    builder.def_var(tag_var, target_tag);
                    builder.def_var(payload_var, target_payload);
                    let (tag, payload) = Self::emit_expr(body, builder, module, strings, functions, scope, bridges, lambdas)?;
                    builder.ins().jump(merge_block, &[tag, payload]);
                    break;
                }

                // Literal patterns: nil, bool, int
                Value::Nil => {
                    let is_nil = builder.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        target_tag, TAG_NIL,
                    );
                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    builder.ins().brif(is_nil, then_block, &[], else_block, &[]);

                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    let (tag, payload) = Self::emit_expr(body, builder, module, strings, functions, scope, bridges, lambdas)?;
                    builder.ins().jump(merge_block, &[tag, payload]);

                    builder.switch_to_block(else_block);
                    builder.seal_block(else_block);
                    if is_last {
                        // No match — return nil
                        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
                        let nil_payload = builder.ins().iconst(types::I64, 0);
                        builder.ins().jump(merge_block, &[nil_tag, nil_payload]);
                    }
                }
                Value::Bool(b) => {
                    let is_bool = builder.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        target_tag, TAG_BOOL,
                    );
                    let expected = if *b { 1i64 } else { 0i64 };
                    let is_val = builder.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        target_payload, expected,
                    );
                    let is_match = builder.ins().band(is_bool, is_val);

                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    builder.ins().brif(is_match, then_block, &[], else_block, &[]);

                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    let (tag, payload) = Self::emit_expr(body, builder, module, strings, functions, scope, bridges, lambdas)?;
                    builder.ins().jump(merge_block, &[tag, payload]);

                    builder.switch_to_block(else_block);
                    builder.seal_block(else_block);
                    if is_last {
                        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
                        let nil_payload = builder.ins().iconst(types::I64, 0);
                        builder.ins().jump(merge_block, &[nil_tag, nil_payload]);
                    }
                }
                Value::Int(n) => {
                    let is_int = builder.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        target_tag, TAG_INT,
                    );
                    let is_val = builder.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        target_payload, *n,
                    );
                    let is_match = builder.ins().band(is_int, is_val);

                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    builder.ins().brif(is_match, then_block, &[], else_block, &[]);

                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    let (tag, payload) = Self::emit_expr(body, builder, module, strings, functions, scope, bridges, lambdas)?;
                    builder.ins().jump(merge_block, &[tag, payload]);

                    builder.switch_to_block(else_block);
                    builder.seal_block(else_block);
                    if is_last {
                        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
                        let nil_payload = builder.ins().iconst(types::I64, 0);
                        builder.ins().jump(merge_block, &[nil_tag, nil_payload]);
                    }
                }

                _ => {
                    // Unsupported pattern — skip (treat as no match for now)
                    if is_last {
                        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
                        let nil_payload = builder.ins().iconst(types::I64, 0);
                        builder.ins().jump(merge_block, &[nil_tag, nil_payload]);
                    }
                }
            }
        }

        builder.seal_block(merge_block);
        builder.switch_to_block(merge_block);
        let result_tag = builder.block_params(merge_block)[0];
        let result_payload = builder.block_params(merge_block)[1];
        Ok((result_tag, result_payload))
    }

    /// (__make_closure "lambda_name" cap1 cap2 ...) — create a closure struct on heap
    /// Closure layout: [func_ptr: i64, cap0_tag, cap0_payload, cap1_tag, cap1_payload, ...]
    fn emit_make_closure(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        scope: &mut FnScope,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.is_empty() {
            return Err("__make_closure requires lambda name".to_string());
        }
        let lambda_name = args[0].as_str().map_err(|e| e.to_string())?;
        let (func_id, _, _) = *lambdas.get(lambda_name)
            .ok_or_else(|| format!("lambda {} not found", lambda_name))?;

        let captures = &args[1..]; // captured variable symbols
        let capture_count = captures.len();

        // Allocate closure struct on heap via stack slot + runtime
        // Layout: [func_ptr, cap0_tag, cap0_payload, ...]
        let struct_size = (1 + capture_count * 2) * 8;
        let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            struct_size as u32,
            0,
        ));

        // Store func_ptr
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let func_ptr = builder.ins().func_addr(types::I64, func_ref);
        builder.ins().stack_store(func_ptr, slot, 0);

        // Store captured values
        for (i, cap) in captures.iter().enumerate() {
            let cap_name = cap.as_symbol().map_err(|e| e.to_string())?;
            let (tag_var, payload_var) = scope.get_var(cap_name)
                .ok_or_else(|| format!("capture variable {} not found", cap_name))?;
            let tag_val = builder.use_var(tag_var);
            let payload_val = builder.use_var(payload_var);
            let offset_tag = ((1 + i * 2) * 8) as i32;
            let offset_payload = ((1 + i * 2 + 1) * 8) as i32;
            builder.ins().stack_store(tag_val, slot, offset_tag);
            builder.ins().stack_store(payload_val, slot, offset_payload);
        }

        // Get pointer to the closure struct
        // NOTE: This points to the stack slot, which is only valid within this function frame.
        // For closures that escape, we'd need heap allocation. For now, use runtime bridge.
        let closure_ptr = builder.ins().stack_addr(types::I64, slot, 0);

        // Heap-allocate via lsp_closure_alloc bridge (copy stack data to heap)
        // For simplicity, we'll just return the stack addr — closures passed as args
        // to functions called in the same frame will work. For escaping closures,
        // a heap allocation bridge would be needed.

        let tag = builder.ins().iconst(types::I64, TAG_FN);
        Ok((tag, closure_ptr))
    }

    /// Call a closure: evaluate the callee variable, extract func_ptr and env_ptr,
    /// then do an indirect call.
    fn emit_closure_call(
        callee_name: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        let (tag_var, payload_var) = scope.get_var(callee_name)
            .ok_or_else(|| format!("undefined variable: {}", callee_name))?;
        let closure_ptr = builder.use_var(payload_var);
        let _ = builder.use_var(tag_var); // TAG_FN, we trust it

        // Load func_ptr from closure struct offset 0
        let func_ptr = builder.ins().load(types::I64, cranelift_codegen::ir::MemFlags::new(), closure_ptr, 0);

        // env_ptr = closure_ptr + 8 (start of captures)
        let env_ptr = builder.ins().iadd_imm(closure_ptr, 8);

        // Build call args: (env_ptr, arg0_tag, arg0_payload, ...)
        let mut call_args = vec![env_ptr];
        for arg in args {
            let (tag, payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges, lambdas)?;
            call_args.push(tag);
            call_args.push(payload);
        }

        // Build signature for indirect call: (env_ptr, N * (tag, payload)) → (tag, payload)
        let arg_count = args.len();
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // env_ptr
        for _ in 0..arg_count {
            sig.params.push(AbiParam::new(types::I64)); // tag
            sig.params.push(AbiParam::new(types::I64)); // payload
        }
        sig.returns.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));

        let sig_ref = builder.import_signature(sig);
        let call = builder.ins().call_indirect(sig_ref, func_ptr, &call_args);
        let results = builder.inst_results(call);
        Ok((results[0], results[1]))
    }

    fn emit_bridge_io(
        name: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err(format!("{} requires 1 argument", name));
        }
        let (tag, payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope, bridges, lambdas)?;
        let bridge_id = bridges.get(name)
            .ok_or_else(|| format!("bridge function {} not found", name))?;
        let local_func = module.declare_func_in_func(*bridge_id, builder.func);
        builder.ins().call(local_func, &[tag, payload]);

        let nil_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let nil_payload = builder.ins().iconst(types::I64, 0);
        Ok((nil_tag, nil_payload))
    }

    /// (loop [bindings...] body...)
    fn emit_loop(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.is_empty() {
            return Err("loop requires bindings".to_string());
        }

        let bindings = match &args[0] {
            Value::List(items) | Value::Vec(items) => items.to_vec(),
            _ => return Err("loop bindings must be a list or vector".to_string()),
        };
        if bindings.len() % 2 != 0 {
            return Err("loop bindings must have even number of elements".to_string());
        }

        // Collect binding names and evaluate initial values
        let mut names = Vec::new();
        let mut init_tags = Vec::new();
        let mut init_payloads = Vec::new();
        for chunk in bindings.chunks(2) {
            let name = chunk[0].as_symbol().map_err(|e| e.to_string())?.to_string();
            let (tag, payload) = Self::emit_expr(&chunk[1], builder, module, strings, functions, scope, bridges, lambdas)?;
            names.push(name);
            init_tags.push(tag);
            init_payloads.push(payload);
        }

        // Create loop header block with params for each binding (tag + payload)
        let loop_header = builder.create_block();
        for _ in 0..names.len() {
            builder.append_block_param(loop_header, types::I64); // tag
            builder.append_block_param(loop_header, types::I64); // payload
        }

        // Jump to loop header with initial values
        let mut jump_args = Vec::new();
        for i in 0..names.len() {
            jump_args.push(init_tags[i]);
            jump_args.push(init_payloads[i]);
        }
        builder.ins().jump(loop_header, &jump_args);

        // Switch to loop header and bind params to variables
        builder.switch_to_block(loop_header);
        // Don't seal yet — recur will add a back-edge
        for (i, name) in names.iter().enumerate() {
            let (tag_var, payload_var) = scope.declare_var(name, builder);
            let tag_val = builder.block_params(loop_header)[i * 2];
            let payload_val = builder.block_params(loop_header)[i * 2 + 1];
            builder.def_var(tag_var, tag_val);
            builder.def_var(payload_var, payload_val);
        }

        // Set loop context so recur can find us
        let prev_loop_ctx = scope.loop_ctx.take();
        scope.loop_ctx = Some((loop_header, names.clone()));

        // Compile body
        let body = &args[1..];
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in body {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges, lambdas)?;
            last_tag = tag;
            last_payload = payload;
        }

        // Restore loop context
        scope.loop_ctx = prev_loop_ctx;

        // Exit block: body finished without recur, return last value
        let exit_block = builder.create_block();
        builder.append_block_param(exit_block, types::I64);
        builder.append_block_param(exit_block, types::I64);
        builder.ins().jump(exit_block, &[last_tag, last_payload]);

        // Seal blocks now that all predecessors are known
        builder.seal_block(loop_header);
        builder.seal_block(exit_block);

        builder.switch_to_block(exit_block);
        let result_tag = builder.block_params(exit_block)[0];
        let result_payload = builder.block_params(exit_block)[1];
        Ok((result_tag, result_payload))
    }

    /// (recur args...)
    fn emit_recur(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        let (loop_header, ref names) = scope.loop_ctx.clone()
            .ok_or_else(|| "recur outside of loop".to_string())?;

        if args.len() != names.len() {
            return Err(format!("recur: expected {} args, got {}", names.len(), args.len()));
        }

        // Evaluate recur arguments
        let mut jump_args = Vec::new();
        for arg in args {
            let (tag, payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges, lambdas)?;
            jump_args.push(tag);
            jump_args.push(payload);
        }

        // Jump back to loop header
        builder.ins().jump(loop_header, &jump_args);

        // Create unreachable block for any code after recur
        let dead_block = builder.create_block();
        builder.switch_to_block(dead_block);
        builder.seal_block(dead_block);

        // Return dummy values (this code is unreachable)
        let tag = builder.ins().iconst(types::I64, TAG_NIL);
        let payload = builder.ins().iconst(types::I64, 0);
        Ok((tag, payload))
    }

    /// (list elem1 elem2 ...) — create a list via runtime bridge
    fn emit_list_literal(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.is_empty() {
            // Empty list = nil
            let tag = builder.ins().iconst(types::I64, TAG_NIL);
            let payload = builder.ins().iconst(types::I64, 0);
            return Ok((tag, payload));
        }

        // Evaluate all elements and store tag+payload pairs on stack
        let count = args.len();
        // Allocate stack slot: count * 2 * 8 bytes (tag + payload, each i64)
        let slot_size = (count * 2 * 8) as u32;
        let stack_slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            slot_size,
            0,
        ));

        for (i, arg) in args.iter().enumerate() {
            let (tag, payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges, lambdas)?;
            let offset_tag = (i * 2 * 8) as i32;
            let offset_payload = (i * 2 * 8 + 8) as i32;
            builder.ins().stack_store(tag, stack_slot, offset_tag);
            builder.ins().stack_store(payload, stack_slot, offset_payload);
        }

        let elements_ptr = builder.ins().stack_addr(types::I64, stack_slot, 0);
        let count_val = builder.ins().iconst(types::I64, count as i64);

        let bridge_id = bridges.get("list_new")
            .ok_or_else(|| "bridge function list_new not found".to_string())?;
        let local_func = module.declare_func_in_func(*bridge_id, builder.func);
        let call = builder.ins().call(local_func, &[count_val, elements_ptr]);
        let results = builder.inst_results(call);
        Ok((results[0], results[1]))
    }

    /// Generic bridge call for list operations: cons, first, rest, count, nth, empty?, concat
    fn emit_bridge_call(
        name: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        let bridge_id = bridges.get(name)
            .ok_or_else(|| format!("bridge function {} not found", name))?;

        let mut call_args = Vec::new();
        for arg in args {
            let (tag, payload) = Self::emit_expr(arg, builder, module, strings, functions, scope, bridges, lambdas)?;
            call_args.push(tag);
            call_args.push(payload);
        }

        let local_func = module.declare_func_in_func(*bridge_id, builder.func);
        let call = builder.ins().call(local_func, &call_args);
        let results = builder.inst_results(call);
        Ok((results[0], results[1]))
    }

    /// (do expr1 expr2 ...)
    fn emit_do(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, Vec<(FuncId, usize)>>,
        scope: &mut FnScope,
        bridges: &HashMap<String, FuncId>,
        lambdas: &HashMap<String, (FuncId, usize, usize)>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in args {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope, bridges, lambdas)?;
            last_tag = tag;
            last_payload = payload;
        }
        Ok((last_tag, last_payload))
    }

    /// Find free variables in an expression (variables used but not defined locally).
    fn free_vars(expr: &Value, bound: &std::collections::HashSet<String>) -> std::collections::HashSet<String> {
        use std::collections::HashSet;
        match expr {
            Value::Symbol(s) => {
                let name = s.as_str();
                if !bound.contains(name) {
                    let mut set = HashSet::new();
                    set.insert(name.to_string());
                    set
                } else {
                    HashSet::new()
                }
            }
            Value::List(items) | Value::Vec(items) => {
                if items.is_empty() {
                    return HashSet::new();
                }
                // Check for binding forms
                if let Value::Symbol(sym) = &items[0] {
                    match sym.as_str() {
                        "fn" if items.len() >= 3 => {
                            // (fn (params) body...) — params are bound
                            let mut inner_bound = bound.clone();
                            if let Value::List(params) | Value::Vec(params) = &items[1] {
                                for p in params.iter() {
                                    if let Value::Symbol(s) = p {
                                        inner_bound.insert(s.to_string());
                                    }
                                }
                            }
                            let mut fv = HashSet::new();
                            for e in &items[2..] {
                                fv.extend(Self::free_vars(e, &inner_bound));
                            }
                            return fv;
                        }
                        "let" if items.len() >= 2 => {
                            let mut inner_bound = bound.clone();
                            let mut fv = HashSet::new();
                            if let Value::List(bindings) | Value::Vec(bindings) = &items[1] {
                                for chunk in bindings.chunks(2) {
                                    if chunk.len() == 2 {
                                        fv.extend(Self::free_vars(&chunk[1], &inner_bound));
                                        if let Value::Symbol(s) = &chunk[0] {
                                            inner_bound.insert(s.to_string());
                                        }
                                    }
                                }
                            }
                            for e in &items[2..] {
                                fv.extend(Self::free_vars(e, &inner_bound));
                            }
                            return fv;
                        }
                        "loop" if items.len() >= 2 => {
                            let mut inner_bound = bound.clone();
                            let mut fv = HashSet::new();
                            if let Value::List(bindings) | Value::Vec(bindings) = &items[1] {
                                for chunk in bindings.chunks(2) {
                                    if chunk.len() == 2 {
                                        fv.extend(Self::free_vars(&chunk[1], &inner_bound));
                                        if let Value::Symbol(s) = &chunk[0] {
                                            inner_bound.insert(s.to_string());
                                        }
                                    }
                                }
                            }
                            for e in &items[2..] {
                                fv.extend(Self::free_vars(e, &inner_bound));
                            }
                            return fv;
                        }
                        "def" if items.len() >= 3 => {
                            let fv = Self::free_vars(&items[2], bound);
                            return fv;
                        }
                        "defun" | "quote" | "defmacro" => return HashSet::new(),
                        _ => {}
                    }
                }
                let mut fv = HashSet::new();
                for item in items.iter() {
                    fv.extend(Self::free_vars(item, bound));
                }
                fv
            }
            _ => std::collections::HashSet::new(),
        }
    }

    /// Pre-pass: collect all fn expressions, assign names, and register them.
    /// Returns lambdas to compile and rewrites fn expressions with lambda references.
    fn collect_lambdas(exprs: &[Value]) -> (Vec<Value>, Vec<LambdaInfo>) {
        let mut lambdas = Vec::new();
        let mut counter = 0usize;
        let rewritten: Vec<Value> = exprs.iter()
            .map(|e| Self::rewrite_fn_expr(e, &mut lambdas, &mut counter))
            .collect();
        (rewritten, lambdas)
    }

    /// Rewrite fn expressions to __lambda_N references, collecting LambdaInfos.
    fn rewrite_fn_expr(expr: &Value, lambdas: &mut Vec<LambdaInfo>, counter: &mut usize) -> Value {
        match expr {
            Value::List(items) if !items.is_empty() => {
                if let Value::Symbol(sym) = &items[0] {
                    if sym.as_str() == "fn" && items.len() >= 3 {
                        // (fn (params) body...)
                        let params: Vec<String> = if let Value::List(ps) | Value::Vec(ps) = &items[1] {
                            ps.iter().filter_map(|p| {
                                if let Value::Symbol(s) = p { Some(s.to_string()) } else { None }
                            }).collect()
                        } else {
                            vec![]
                        };

                        // Recursively rewrite body first
                        let body: Vec<Value> = items[2..].iter()
                            .map(|e| Self::rewrite_fn_expr(e, lambdas, counter))
                            .collect();

                        // Find free variables
                        let mut bound: std::collections::HashSet<String> = params.iter().cloned().collect();
                        // Add well-known names (builtins, special forms) so they don't count as captures
                        for name in &["+", "-", "*", "/", "%", "=", "<", ">", "<=", ">=", "!=",
                            "not", "if", "do", "let", "def", "defun", "fn", "loop", "recur",
                            "list", "cons", "first", "rest", "count", "nth", "empty?", "concat",
                            "println", "print", "str", "nil?", "number?", "string?", "list?", "fn?",
                            "true", "false", "nil", "when", "unless", "map", "filter", "reduce",
                            "apply", "identity", "match", "throw", "try", "catch", "quote",
                            "quasiquote"] {
                            bound.insert(name.to_string());
                        }

                        let mut fv: Vec<String> = Self::free_vars(
                            &Value::list(body.clone().into_iter().chain(std::iter::empty()).collect()),
                            &bound,
                        ).into_iter().collect();
                        fv.sort(); // deterministic order

                        let name = format!("__lambda_{}", *counter);
                        *counter += 1;

                        lambdas.push(LambdaInfo {
                            name: name.clone(),
                            params,
                            body,
                            captures: fv.clone(),
                        });

                        // Replace (fn ...) with (__make_closure "__lambda_N" cap1 cap2 ...)
                        let mut closure_expr = vec![
                            Value::symbol("__make_closure"),
                            Value::str(name),
                        ];
                        for cap in &fv {
                            closure_expr.push(Value::symbol(cap.clone()));
                        }
                        return Value::list(closure_expr);
                    }
                    if sym.as_str() == "quote" {
                        return expr.clone();
                    }
                }
                // Recursively rewrite children
                Value::list(items.iter().map(|e| Self::rewrite_fn_expr(e, lambdas, counter)).collect())
            }
            Value::Vec(items) => {
                Value::vec(items.iter().map(|e| Self::rewrite_fn_expr(e, lambdas, counter)).collect())
            }
            _ => expr.clone(),
        }
    }

    /// Macro expansion pre-pass: evaluate defmacro forms and expand macros in the AST.
    /// Uses the interpreter's Env to register macros (including prelude's when/unless).
    fn expand_macros(exprs: &[Value]) -> Result<Vec<Value>, String> {
        let mut env = Env::new();
        builtins::register(&mut env);
        prelude::load(&mut env).map_err(|e| format!("prelude load error: {}", e))?;

        let mut result = Vec::new();
        for expr in exprs {
            // Evaluate defmacro forms to register macros, then skip them
            if let Value::List(items) = expr {
                if let Some(Value::Symbol(sym)) = items.first() {
                    if sym.as_str() == "defmacro" {
                        eval::eval(expr, &mut env)
                            .map_err(|e| format!("macro definition error: {}", e))?;
                        continue;
                    }
                }
            }
            // Recursively expand macros in the expression
            let expanded = Self::expand_expr(expr, &mut env)?;
            result.push(expanded);
        }
        Ok(result)
    }

    /// Recursively expand macros in a single expression.
    fn expand_expr(expr: &Value, env: &mut Env) -> Result<Value, String> {
        match expr {
            Value::List(items) if !items.is_empty() => {
                // Check if head is a macro
                if let Value::Symbol(sym) = &items[0] {
                    if let Ok(val) = env.get(sym) {
                        if let Value::Macro(mac) = &val {
                            // Expand macro without evaluating the result
                            let expanded = eval::expand_macro(&mac, &items[1..], env)
                                .map_err(|e| format!("macro expansion error: {}", e))?;
                            // Recursively expand the result (macros may produce more macros)
                            return Self::expand_expr(&expanded, env);
                        }
                    }
                }
                // Not a macro call — recursively expand children
                let expanded_items: Result<Vec<Value>, String> = items.iter()
                    .map(|item| Self::expand_expr(item, env))
                    .collect();
                Ok(Value::list(expanded_items?))
            }
            Value::Vec(items) => {
                let expanded_items: Result<Vec<Value>, String> = items.iter()
                    .map(|item| Self::expand_expr(item, env))
                    .collect();
                Ok(Value::vec(expanded_items?))
            }
            _ => Ok(expr.clone()),
        }
    }

    /// Compile all expressions into an object file.
    pub fn compile_exprs(mut self, exprs: &[Value]) -> Result<Vec<u8>, String> {
        if exprs.is_empty() {
            return Err("nothing to compile".to_string());
        }

        // Pass 0: macro expansion
        let exprs = Self::expand_macros(exprs)?;

        // Pass 0.5: collect lambda (fn) expressions and rewrite them
        let (exprs, lambda_infos) = Self::collect_lambdas(&exprs);

        // Pass 1: collect strings
        self.collect_strings(&exprs)?;

        // Pass 2: declare all top-level functions
        self.declare_functions(&exprs)?;

        // Pass 2.5: declare and compile lambda functions
        // Lambda signature: (env_ptr, param0_tag, param0_payload, ...) → (tag, payload)
        for info in &lambda_infos {
            let param_count = info.params.len();
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // env_ptr (closure data)
            for _ in 0..param_count {
                sig.params.push(AbiParam::new(types::I64)); // tag
                sig.params.push(AbiParam::new(types::I64)); // payload
            }
            sig.returns.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let func_name = format!("_lsp_fn_{}", info.name);
            let func_id = self.module
                .declare_function(&func_name, Linkage::Local, &sig)
                .map_err(|e| e.to_string())?;
            self.lambdas.insert(info.name.clone(), (func_id, param_count, info.captures.len()));
        }
        // Compile lambda bodies
        for info in &lambda_infos {
            self.compile_lambda(info)?;
        }

        // Pass 3: compile each defun (single + multi-arity)
        // Collect defun info first to avoid borrow issues
        let defuns: Vec<(String, Vec<Value>, Vec<Value>)> = exprs.iter().flat_map(|expr| {
            let mut result = Vec::new();
            if let Value::List(items) = expr {
                if items.len() >= 3 {
                    if let (Value::Symbol(sym), Value::Symbol(name)) = (&items[0], &items[1]) {
                        if sym.as_str() == "defun" {
                            // Check if multi-arity
                            let is_multi = if let Value::List(first_clause) = &items[2] {
                                !first_clause.is_empty() && matches!(&first_clause[0], Value::List(_) | Value::Vec(_))
                            } else {
                                false
                            };

                            if is_multi {
                                for clause in &items[2..] {
                                    if let Value::List(clause_items) = clause {
                                        if clause_items.len() >= 2 {
                                            if let Value::List(params) = &clause_items[0] {
                                                let body = clause_items[1..].to_vec();
                                                result.push((name.to_string(), params.to_vec(), body));
                                            }
                                        }
                                    }
                                }
                            } else if items.len() >= 4 {
                                if let Value::List(params) = &items[2] {
                                    let body = items[3..].to_vec();
                                    result.push((name.to_string(), params.to_vec(), body));
                                }
                            }
                        }
                    }
                }
            }
            result
        }).collect();

        for (name, params, body) in &defuns {
            self.compile_defun(name, params, body)?;
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
                    &self.lambdas,
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
    fn test_compile_variadic_arithmetic() {
        let exprs = parse("(+ 1 2 3 4 5)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());

        let exprs = parse("(* 2 3 4)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
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
    fn test_compile_macro_when() {
        // when is defined in prelude — should be expanded to (if cond body nil)
        let exprs = parse("(when true 42)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_macro_unless() {
        let exprs = parse("(unless false 99)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_custom_macro() {
        let exprs = parse("(defmacro double (x) `(+ ~x ~x)) (double 21)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }

    #[test]
    fn test_compile_loop_recur_sum() {
        // Sum 1..10 using loop/recur
        let exprs = parse("(loop [i 0 sum 0] (if (= i 10) sum (recur (+ i 1) (+ sum i))))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_loop_recur_in_defun() {
        let exprs = parse("(defun sum-to (n) (loop [i 0 acc 0] (if (= i n) acc (recur (+ i 1) (+ acc i))))) (sum-to 100)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_loop_recur_simple() {
        let exprs = parse("(loop [x 5] (if (= x 0) x (recur (- x 1))))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_macro_in_defun() {
        // Macros inside function bodies should also be expanded
        let exprs = parse("(defun f (x) (when (> x 0) x)) (f 5)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_multi_arity() {
        let exprs = parse("(defun greet ((x) x) ((x y) (+ x y))) (greet 1) (greet 2 3)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_multi_arity_recursive() {
        let exprs = parse("(defun f ((n) (f n 0)) ((n acc) (if (= n 0) acc (f (- n 1) (+ acc n))))) (f 10)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_literal() {
        let exprs = parse("(list 1 2 3)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_empty() {
        let exprs = parse("(list)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_first_rest() {
        let exprs = parse("(def xs (list 1 2 3)) (first xs) (rest xs)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_cons() {
        let exprs = parse("(cons 0 (list 1 2 3))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_count() {
        let exprs = parse("(count (list 1 2 3))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_nth() {
        let exprs = parse("(nth (list 10 20 30) 1)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_empty_check() {
        let exprs = parse("(empty? (list)) (empty? (list 1))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_list_in_defun() {
        let exprs = parse("(defun sum-list (xs) (loop [remaining xs acc 0] (if (empty? remaining) acc (recur (rest remaining) (+ acc (first remaining)))))) (sum-list (list 1 2 3 4 5))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_destructure_vec() {
        let exprs = parse("(let [[a b c] (list 1 2 3)] (+ a b c))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_destructure_nested() {
        let exprs = parse("(let [[a [b c]] (list 1 (list 2 3))] (+ a b c))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_fn_simple() {
        // Lambda without captures
        let exprs = parse("(def add1 (fn (x) (+ x 1))) (add1 5)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_fn_closure() {
        // Lambda with closure capture
        let exprs = parse("(def offset 10) (def add-offset (fn (x) (+ x offset))) (add-offset 5)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_fn_as_arg() {
        // Pass lambda to a function
        let exprs = parse("(defun apply-fn (f x) (f x)) (apply-fn (fn (n) (+ n 1)) 10)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_match_literal() {
        let exprs = parse("(match 1 1 \"one\" 2 \"two\" _ \"other\")").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_match_wildcard() {
        let exprs = parse("(match 42 _ \"matched\")").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_match_binding() {
        let exprs = parse("(match 5 x (+ x 1))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_match_bool() {
        let exprs = parse("(match true true 1 false 0)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_match_nil() {
        let exprs = parse("(match nil nil \"nil\" _ \"not nil\")").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_throw() {
        let exprs = parse("(throw 42)").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_try_catch() {
        let exprs = parse("(try (throw 42) (catch e e))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }

    #[test]
    fn test_compile_try_no_error() {
        let exprs = parse("(try (+ 1 2) (catch e 0))").unwrap();
        let result = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(result.is_ok(), "compile error: {}", result.unwrap_err());
    }
}
