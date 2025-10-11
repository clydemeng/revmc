use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{AccountInfo, Bytecode},
    Database,
};
use revm_primitives::{address, keccak256, B256, Bytes, Env, SpecId};
use revmc::{EvmCompiler, EvmContext, EvmLlvmBackend, OptimizationLevel};
// no interpreter table needed for JIT path
use std::sync::Arc;
use std::time::Instant;

use revmc_builtins as _;

const CODE_HEX: &str = include_str!("../../../data/hash_10k.rt.hex");

fn code() -> Bytes {
    let code = revm::primitives::hex::decode(CODE_HEX.trim()).expect("invalid hex");
    Bytes::from(code)
}

fn code_hash() -> B256 { keccak256(code()) }

revmc_context::extern_revmc! { fn merkle_hash; }

pub struct ExternalContext;
impl ExternalContext {
    fn get_function(&self, bytecode_hash: B256) -> Option<revmc_context::EvmCompilerFn> {
        if bytecode_hash == code_hash() { Some(revmc_context::EvmCompilerFn::new(merkle_hash)) } else { None }
    }
}

fn register_handler<DB: Database + 'static>(handler: &mut EvmHandler<'_, ExternalContext, DB>) {
    let prev = handler.execution.execute_frame.clone();
    handler.execution.execute_frame = Arc::new(move |frame, memory, tables, context| {
        let interpreter = frame.interpreter_mut();
        let h = interpreter.contract.hash.unwrap_or_default();
        if let Some(f) = context.external.get_function(h) { Ok(unsafe { f.call_with_interpreter_and_memory(interpreter, memory, context) }) } else { prev(frame, memory, tables, context) }
    });
}

fn build_evm_with_aot<'a, DB: Database + 'static>(db: DB) -> revm::Evm<'a, ExternalContext, DB> {
    revm::Evm::builder().with_db(db).with_external_context(ExternalContext).append_handler_register(register_handler).build()
}

fn build_evm_plain<'a, DB: Database>(db: DB) -> revm::Evm<'a, (), DB> {
    revm::Evm::builder().with_db(db).build()
}

fn main() {
    // args: n_iters, warmup
    let n_iters: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(10_000);
    let warmup: u64 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(1_000);

    let code_bytes = code();
    let hash = code_hash();
    let addr = revm::primitives::Address::with_last_byte(0x55);
    let selector: [u8; 4] = [0x30, 0x62, 0x7b, 0x7c]; // Benchmark()

    // AOT
    let db_aot = CacheDB::new(EmptyDB::new());
    let mut evm_aot = build_evm_with_aot(db_aot);
    evm_aot.db_mut().insert_account_info(addr, AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code_bytes.clone())), ..Default::default() });
    evm_aot.context.evm.env.tx.transact_to = revm_primitives::TransactTo::Call(addr);
    evm_aot.context.evm.env.tx.data = Bytes::from(selector.to_vec());

    // Interpreter
    let db_plain = CacheDB::new(EmptyDB::new());
    let mut evm_plain = build_evm_plain(db_plain);
    evm_plain.db_mut().insert_account_info(addr, AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code_bytes.clone())), ..Default::default() });
    evm_plain.context.evm.env.tx.transact_to = revm_primitives::TransactTo::Call(addr);
    evm_plain.context.evm.env.tx.data = Bytes::from(selector.to_vec());

    // JIT setup (compile bytecode and prepare interpreter+host)
    let context = revmc::llvm::inkwell::context::Context::create();
    let backend = EvmLlvmBackend::new(&context, false, OptimizationLevel::Aggressive).expect("jit backend");
    let mut compiler = EvmCompiler::new(backend);
    let spec_id = SpecId::CANCUN; // EOF is not required for this runtime
    let f = unsafe { compiler.jit("merkle_hash", &code_bytes[..], spec_id) }.expect("jit compile");
    // Build env/contract/host like revmc-cli
    let mut env = Env::default();
    env.tx.caller = address!("0000000000000000000000000000000000000001");
    env.tx.transact_to = revm_primitives::TransactTo::Call(addr);
    env.tx.data = Bytes::from(selector.to_vec());
    env.tx.gas_limit = 1_000_000_000;
    let analysed = {
        let analysed_code = code_bytes.clone();
        revm_interpreter::analysis::to_analysed(revm_primitives::Bytecode::new_raw(analysed_code))
    };
    let contract = revm_interpreter::Contract::new_env(&env, analysed, None);
    let mut host = revm_interpreter::DummyHost::new(env);
    let mut run_jit = || {
        let mut interpreter = revm_interpreter::Interpreter::new(contract.clone(), 1_000_000_000, false);
        host.clear();
        let (mut ecx, stack, stack_len) = EvmContext::from_interpreter_with_stack(&mut interpreter, &mut host);
        unsafe { f.call_noinline(Some(stack), Some(stack_len), &mut ecx) };
        // Do not re-run the interpreter, to avoid executing the program twice per iteration.
    };

    // Warmup
    for _ in 0..warmup { let _ = evm_aot.transact().unwrap(); }
    for _ in 0..warmup { let _ = evm_plain.transact().unwrap(); }
    for _ in 0..warmup { run_jit(); }

    // Timed
    let t0 = Instant::now();
    for _ in 0..n_iters { let _ = evm_aot.transact().unwrap(); }
    let aot_elapsed = t0.elapsed();

    let t1 = Instant::now();
    for _ in 0..n_iters { let _ = evm_plain.transact().unwrap(); }
    let plain_elapsed = t1.elapsed();

    let t2 = Instant::now();
    for _ in 0..n_iters { run_jit(); }
    let jit_elapsed = t2.elapsed();

    println!(
        "merkle_hash n_iters={} | AOT avg={:?} total={:?} | JIT avg={:?} total={:?} | Interpreter avg={:?} total={:?}",
        n_iters,
        aot_elapsed / (n_iters as u32), aot_elapsed,
        jit_elapsed / (n_iters as u32), jit_elapsed,
        plain_elapsed / (n_iters as u32), plain_elapsed
    );
}


