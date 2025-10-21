use revmc::{
    primitives::SpecId,
    EvmCompiler, EvmLlvmBackend, OptimizationLevel, Result,
};
use std::path::PathBuf;

fn main() -> Result<()> {
    revmc_build::emit();

    // Use teammate's WBNB runtime bytecode
    let code = std::fs::read("/Users/ruojunm/workspace_2025/rust/remvc_workspace/const/const-revmc/examples/wbnb/wbnb.runtime.bin").expect("read wbnb.runtime.bin");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, true, OptimizationLevel::Aggressive)?;
    let mut compiler = EvmCompiler::new(backend);
    compiler.translate("wbnb", &code, SpecId::CANCUN)?;
    let object = out_dir.join("wbnb").with_extension("o");
    compiler.write_object_to_file(&object)?;
    cc::Build::new().object(&object).static_flag(true).compile("wbnb");

    Ok(())
}


