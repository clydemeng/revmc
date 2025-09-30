use revmc::{
    primitives::SpecId,
    EvmCompiler, EvmLlvmBackend, OptimizationLevel, Result,
};
use std::path::PathBuf;

// Compile fibonacci bytecode AOT and link it as a static lib for the example.
fn main() -> Result<()> {
    // Emit symbols needed by compiled bytecodes when dynamically loaded.
    revmc_build::emit();

    // Fibonacci sample, same as examples/runner/src/common.rs
    // 5f355f60015b8215601a578181019150909160019003916005565b9150505f5260205ff3
    const FIBONACCI_CODE: &[u8] = &[
        0x5f, 0x35, 0x5f, 0x60, 0x01, 0x5b, 0x82, 0x15, 0x60, 0x1a, 0x57, 0x81, 0x81, 0x01, 0x91,
        0x50, 0x90, 0x91, 0x60, 0x01, 0x90, 0x03, 0x91, 0x60, 0x05, 0x56, 0x5b, 0x91, 0x50, 0x50,
        0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3,
    ];

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);

    // AOT compile fibonacci
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, true, OptimizationLevel::Aggressive)?;
    let mut compiler = EvmCompiler::new(backend);
    compiler.translate("fibonacci", FIBONACCI_CODE, SpecId::CANCUN)?;
    let object = out_dir.join("fibonacci").with_extension("o");
    compiler.write_object_to_file(&object)?;

    // Turn the .o into a static library (libfibonacci.a)
    cc::Build::new().object(&object).static_flag(true).compile("fibonacci");

    Ok(())
}


