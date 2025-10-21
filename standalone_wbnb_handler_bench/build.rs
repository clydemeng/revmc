use revmc::{EvmCompiler, EvmLlvmBackend, OptimizationLevel, primitives::SpecId};
use std::path::PathBuf;

fn main() {
    revmc_build::emit();

    // Read WBNB runtime from teammate path
    let code = std::fs::read("/Users/ruojunm/workspace_2025/rust/remvc_workspace/const/const-revmc/examples/wbnb/wbnb.runtime.bin").expect("read wbnb.runtime.bin");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, true, OptimizationLevel::Aggressive).expect("backend");
    let mut compiler = EvmCompiler::new(backend);
    compiler.translate("wbnb", &code, SpecId::CANCUN).expect("translate");
    let object = out_dir.join("wbnb").with_extension("o");
    compiler.write_object_to_file(&object).expect("write object");
    cc::Build::new().object(&object).static_flag(true).compile("wbnb");
}


