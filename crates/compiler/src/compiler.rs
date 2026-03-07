use std::collections::HashMap;

use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{default_libcall_names, DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use lisprint_core::value::Value;

/// Runtime value tag constants
pub const TAG_NIL: i64 = 0;
pub const TAG_BOOL: i64 = 1;
pub const TAG_INT: i64 = 2;
pub const TAG_FLOAT: i64 = 3;
pub const TAG_STR: i64 = 4;

/// Tracks local variables within a function being compiled.
/// Each Lisp value is represented as two Cranelift Variables: (tag, payload).
struct FnScope {
    locals: HashMap<String, (Variable, Variable)>,
    next_var: u32,
}

impl FnScope {
    fn new() -> Self {
        Self {
            locals: HashMap::new(),
            next_var: 0,
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

        Ok(Self {
            module,
            ctx,
            func_ctx,
            strings: HashMap::new(),
            next_str_id: 0,
            functions: HashMap::new(),
            next_func_idx: 1, // 0 is reserved for _lsp_main
        })
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
                        "def" => return Self::emit_def(&items[1..], builder, module, strings, functions, scope),
                        "do" => return Self::emit_do(&items[1..], builder, module, strings, functions, scope),
                        "if" => return Self::emit_if(&items[1..], builder, module, strings, functions, scope),
                        "let" => return Self::emit_let(&items[1..], builder, module, strings, functions, scope),
                        "+" | "-" | "*" | "/" | "%" =>
                            return Self::emit_arith(sym.as_str(), &items[1..], builder, module, strings, functions, scope),
                        "=" | "<" | ">" | "<=" | ">=" | "!=" =>
                            return Self::emit_cmp(sym.as_str(), &items[1..], builder, module, strings, functions, scope),
                        "not" => return Self::emit_not(&items[1..], builder, module, strings, functions, scope),
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
                                arg_expr, builder, module, strings, functions, scope,
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
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err("def requires 2 arguments (name value)".to_string());
        }
        let name = args[0].as_symbol().map_err(|e| e.to_string())?;
        let (tag, payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope)?;
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
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() < 2 || args.len() > 3 {
            return Err("if requires 2 or 3 arguments".to_string());
        }

        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope)?;

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
        let (then_tag, then_payload) = Self::emit_expr(&args[1], builder, module, strings, functions, scope)?;
        builder.ins().jump(merge_block, &[then_tag, then_payload]);

        // Else branch
        builder.switch_to_block(else_block);
        builder.seal_block(else_block);
        let (else_tag, else_payload) = if args.len() == 3 {
            Self::emit_expr(&args[2], builder, module, strings, functions, scope)?
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
            let (tag, payload) = Self::emit_expr(&chunk[1], builder, module, strings, functions, scope)?;
            let (tag_var, payload_var) = scope.declare_var(name, builder);
            builder.def_var(tag_var, tag);
            builder.def_var(payload_var, payload);
        }

        let body = &args[1..];
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in body {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope)?;
            last_tag = tag;
            last_payload = payload;
        }
        Ok((last_tag, last_payload))
    }

    /// Arithmetic: +, -, *, /, %
    fn emit_arith(
        op: &str,
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err(format!("{} requires 2 arguments", op));
        }
        let (_, lhs) = Self::emit_expr(&args[0], builder, module, strings, functions, scope)?;
        let (_, rhs) = Self::emit_expr(&args[1], builder, module, strings, functions, scope)?;

        let result = match op {
            "+" => builder.ins().iadd(lhs, rhs),
            "-" => builder.ins().isub(lhs, rhs),
            "*" => builder.ins().imul(lhs, rhs),
            "/" => builder.ins().sdiv(lhs, rhs),
            "%" => builder.ins().srem(lhs, rhs),
            _ => unreachable!(),
        };

        let tag = builder.ins().iconst(types::I64, TAG_INT);
        Ok((tag, result))
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
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 2 {
            return Err(format!("{} requires 2 arguments", op));
        }
        let (_, lhs) = Self::emit_expr(&args[0], builder, module, strings, functions, scope)?;
        let (_, rhs) = Self::emit_expr(&args[1], builder, module, strings, functions, scope)?;

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
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        if args.len() != 1 {
            return Err("not requires 1 argument".to_string());
        }
        let (cond_tag, cond_payload) = Self::emit_expr(&args[0], builder, module, strings, functions, scope)?;

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

    /// (do expr1 expr2 ...)
    fn emit_do(
        args: &[Value],
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
        functions: &HashMap<String, (FuncId, usize)>,
        scope: &mut FnScope,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
        let mut last_payload = builder.ins().iconst(types::I64, 0);
        for expr in args {
            let (tag, payload) = Self::emit_expr(expr, builder, module, strings, functions, scope)?;
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
    fn test_compile_fibonacci() {
        let exprs = parse("(defun fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 10)").unwrap();
        assert!(Compiler::new().unwrap().compile_exprs(&exprs).is_ok());
    }
}
