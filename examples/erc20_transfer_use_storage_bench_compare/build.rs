use revmc::{
    primitives::SpecId,
    EvmCompiler, EvmLlvmBackend, OptimizationLevel, Result,
};
use std::path::PathBuf;

fn main() -> Result<()> {
    revmc_build::emit();

    // Use locally compiled SimpleERC20 runtime
    let code_hex = include_str!("./erc20_runtime.hex");
    let code = revmc::primitives::hex::decode(code_hex.trim()).expect("invalid erc20 hex");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, true, OptimizationLevel::Aggressive)?;
    let mut compiler = EvmCompiler::new(backend);
    compiler.translate("erc20_transfer", &code, SpecId::CANCUN)?;
    let object = out_dir.join("erc20_transfer").with_extension("o");
    compiler.write_object_to_file(&object)?;
    cc::Build::new().object(&object).static_flag(true).compile("erc20_transfer");

    Ok(())
}


