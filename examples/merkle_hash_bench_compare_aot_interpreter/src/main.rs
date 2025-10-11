use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{AccountInfo, Bytecode},
    Database,
};
use revm_primitives::{keccak256, B256, Bytes};
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
    let n_iters: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(2_000);
    let warmup: u64 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(2_000);

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
    evm_plain.db_mut().insert_account_info(addr, AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code_bytes)), ..Default::default() });
    evm_plain.context.evm.env.tx.transact_to = revm_primitives::TransactTo::Call(addr);
    evm_plain.context.evm.env.tx.data = Bytes::from(selector.to_vec());

    // Warmup
    for _ in 0..warmup { let _ = evm_aot.transact().unwrap(); }
    for _ in 0..warmup { let _ = evm_plain.transact().unwrap(); }

    // Timed
    let t0 = Instant::now();
    for _ in 0..n_iters { let _ = evm_aot.transact().unwrap(); }
    let aot_elapsed = t0.elapsed();

    let t1 = Instant::now();
    for _ in 0..n_iters { let _ = evm_plain.transact().unwrap(); }
    let plain_elapsed = t1.elapsed();

    println!(
        "merkle_hash (AOT vs Interpreter) n_iters={} | AOT avg={:?} total={:?} | Interpreter avg={:?} total={:?}",
        n_iters,
        aot_elapsed / (n_iters as u32), aot_elapsed,
        plain_elapsed / (n_iters as u32), plain_elapsed
    );
}


