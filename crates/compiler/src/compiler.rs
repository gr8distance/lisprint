use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

/// Cranelift-based compiler for lisprint
pub struct Compiler {
    module: ObjectModule,
    ctx: Context,
    func_ctx: FunctionBuilderContext,
}

impl Compiler {
    /// Create a new compiler targeting the host platform
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
        })
    }

    /// Compile a simple expression that returns an i64
    /// This is a proof-of-concept for 4-1
    /// Consumes the compiler since ObjectModule::finish takes ownership
    pub fn compile_i64_constant(mut self, value: i64) -> Result<Vec<u8>, String> {
        // Define main function signature: () -> i64
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));

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

            let val = builder.ins().iconst(types::I64, value);
            builder.ins().return_(&[val]);

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

    #[test]
    fn test_compiler_creation() {
        let compiler = Compiler::new();
        assert!(compiler.is_ok(), "Compiler should initialize successfully");
    }

    #[test]
    fn test_compile_i64_constant() {
        let compiler = Compiler::new().unwrap();
        let obj = compiler.compile_i64_constant(42);
        assert!(obj.is_ok(), "Should compile i64 constant");
        let bytes = obj.unwrap();
        assert!(!bytes.is_empty(), "Object file should not be empty");
    }
}
