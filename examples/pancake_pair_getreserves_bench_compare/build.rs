use revmc::{EvmCompiler, EvmLlvmBackend, OptimizationLevel, primitives::SpecId};
use std::path::PathBuf;

fn main() {
    revmc_build::emit();
    let code_hex = include_str!("../../data/pancake_pair_v2.runtime.hex");
    let code = revmc::primitives::hex::decode(code_hex.trim()).expect("pair runtime hex");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, true, OptimizationLevel::Aggressive).expect("backend");
    let mut compiler = EvmCompiler::new(backend);
    compiler.translate("pancake_pair", &code, SpecId::CANCUN).expect("translate");
    let object = out_dir.join("pancake_pair").with_extension("o");
    compiler.write_object_to_file(&object).expect("write object");
    cc::Build::new().object(&object).static_flag(true).compile("pancake_pair");
}

