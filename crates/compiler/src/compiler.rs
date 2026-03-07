use std::collections::HashMap;

use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, DataDescription, DataId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use lisprint_core::value::Value;

/// Runtime value tag constants
pub const TAG_NIL: i64 = 0;
pub const TAG_BOOL: i64 = 1;
pub const TAG_INT: i64 = 2;
pub const TAG_FLOAT: i64 = 3;
pub const TAG_STR: i64 = 4;

/// Cranelift-based compiler for lisprint
///
/// Runtime value representation: each value is a (tag: i64, payload: i64) pair.
/// - NIL:   (0, 0)
/// - Bool:  (1, 0 or 1)
/// - Int:   (2, i64 value)
/// - Float: (3, f64 bits as i64)
/// - Str:   (4, pointer to null-terminated string data)
pub struct Compiler {
    module: ObjectModule,
    ctx: Context,
    func_ctx: FunctionBuilderContext,
    /// Pre-declared string constants: string content → DataId
    strings: HashMap<String, DataId>,
    next_str_id: usize,
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
        })
    }

    /// Pre-declare a string constant in the data section (deduplicating)
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
        bytes.push(0); // null-terminate
        desc.define(bytes.into_boxed_slice());

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| e.to_string())?;

        self.strings.insert(s.to_string(), data_id);
        Ok(data_id)
    }

    /// Walk the AST to pre-declare all string literals
    fn collect_strings(&mut self, exprs: &[Value]) -> Result<(), String> {
        for expr in exprs {
            self.collect_strings_in_expr(expr)?;
        }
        Ok(())
    }

    fn collect_strings_in_expr(&mut self, expr: &Value) -> Result<(), String> {
        match expr {
            Value::Str(s) => {
                self.ensure_string(s)?;
            }
            Value::List(items) | Value::Vec(items) => {
                for item in items.iter() {
                    self.collect_strings_in_expr(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Emit Cranelift IR for a literal, returning (tag, payload)
    fn emit_literal(
        expr: &Value,
        builder: &mut FunctionBuilder,
        module: &mut ObjectModule,
        strings: &HashMap<String, DataId>,
    ) -> Result<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value), String> {
        match expr {
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
            _ => Err(format!("cannot compile literal: {}", expr.type_name())),
        }
    }

    /// Compile expressions into an object file.
    /// The generated _lsp_main function returns (tag: i64, payload: i64).
    pub fn compile_exprs(mut self, exprs: &[Value]) -> Result<Vec<u8>, String> {
        if exprs.is_empty() {
            return Err("nothing to compile".to_string());
        }

        // Pass 1: pre-declare all string constants
        self.collect_strings(exprs)?;

        // Function signature: () -> (i64, i64)
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64)); // tag
        sig.returns.push(AbiParam::new(types::I64)); // payload

        let func_id = self.module
            .declare_function("_lsp_main", Linkage::Export, &sig)
            .map_err(|e| e.to_string())?;

        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, 0);

        // Pass 2: emit IR
        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.func_ctx);
            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let mut last_tag = builder.ins().iconst(types::I64, TAG_NIL);
            let mut last_payload = builder.ins().iconst(types::I64, 0);

            for expr in exprs {
                let (tag, payload) = Self::emit_literal(
                    expr,
                    &mut builder,
                    &mut self.module,
                    &self.strings,
                )?;
                last_tag = tag;
                last_payload = payload;
            }

            builder.ins().return_(&[last_tag, last_payload]);
            builder.finalize();
        }

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| e.to_string())?;

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
        let obj = Compiler::new().unwrap().compile_exprs(&exprs);
        assert!(obj.is_ok());
        assert!(!obj.unwrap().is_empty());
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
}
