use revmc::{
    primitives::SpecId,
    EvmCompiler, EvmLlvmBackend, OptimizationLevel, Result,
};
use std::path::PathBuf;

fn main() -> Result<()> {
    revmc_build::emit();

    // Use provided hash_10k runtime (many keccak ops, minimal storage)
    let code_hex = include_str!("../../data/hash_10k.rt.hex");
    let code = revmc::primitives::hex::decode(code_hex.trim()).expect("invalid hex");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, true, OptimizationLevel::Aggressive)?;
    let mut compiler = EvmCompiler::new(backend);
    compiler.translate("merkle_hash", &code, SpecId::CANCUN)?;
    let object = out_dir.join("merkle_hash").with_extension("o");
    compiler.write_object_to_file(&object)?;
    cc::Build::new().object(&object).static_flag(true).compile("merkle_hash");

    Ok(())
}


