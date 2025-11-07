use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{AccountInfo, Bytecode},
    Database, DatabaseCommit,
};
use revm_primitives::{keccak256, B256, Bytes, TransactTo, SpecId, U256};
use std::time::Instant;

use revmc_builtins as _;

const RUNTIME_HEX: &str = include_str!("../../../data/pancake_pair_v2.runtime.hex");

fn code() -> Bytes { Bytes::from(revm::primitives::hex::decode(RUNTIME_HEX.trim()).expect("invalid hex")) }
fn code_hash() -> B256 { keccak256(code()) }

revmc_context::extern_revmc! { fn pancake_pair; }

pub struct ExternalContext { target_hash: B256, aot_fn: revmc_context::EvmCompilerFn }
impl ExternalContext { #[inline] fn get_function(&self, h: B256) -> Option<revmc_context::EvmCompilerFn> { if h == self.target_hash { Some(self.aot_fn) } else { None } } }

fn register_handler<DB: Database + 'static>(handler: &mut EvmHandler<'_, ExternalContext, DB>) {
    let prev = handler.execution.execute_frame.clone();
    handler.execution.execute_frame = std::sync::Arc::new(move |frame, memory, tables, context| {
        let interpreter = frame.interpreter_mut();
        let h = interpreter.contract.hash.unwrap_or_default();
        if let Some(f) = context.external.get_function(h) { Ok(unsafe { f.call_with_interpreter_and_memory(interpreter, memory, context) }) } else { prev(frame, memory, tables, context) }
    });
}

fn build_evm_with_aot<'a, DB: Database + 'static>(db: DB, target_hash: B256) -> revm::Evm<'a, ExternalContext, DB> {
    let aot = revmc_context::EvmCompilerFn::new(pancake_pair);
    revm::Evm::builder()
        .with_spec_id(SpecId::LONDON)
        .with_db(db)
        .with_external_context(ExternalContext { target_hash, aot_fn: aot })
        .append_handler_register(register_handler)
        .build()
}

fn selector_get_reserves() -> Bytes { Bytes::from(vec![0x09,0x02,0xf1,0xac]) } // getReserves()

fn main() {
    let pairs: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(100);
    let iters: u64 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(1000);
    let code_bytes = code();
    let hash = code_hash();
    let selector = selector_get_reserves();

    // AOT
    let db_aot = CacheDB::new(EmptyDB::new());
    let mut evm_aot = build_evm_with_aot(db_aot, hash);
    // Interpreter
    let db_plain = CacheDB::new(EmptyDB::new());
    let mut evm_plain = revm::Evm::builder().with_spec_id(SpecId::LONDON).with_db(db_plain).build();

    // Seed N pairs as distinct addresses and insert code
    for i in 0..pairs {
        let addr = revm_primitives::Address::with_last_byte((i % 256) as u8);
        let info = AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code_bytes.clone())), ..Default::default() };
        evm_aot.db_mut().insert_account_info(addr, info.clone());
        evm_plain.db_mut().insert_account_info(addr, info);
    }

    // Set basefee to zero to avoid KZG-related precompile paths
    evm_aot.context.evm.env.block.basefee = U256::ZERO;
    evm_plain.context.evm.env.block.basefee = U256::ZERO;

    // Bench AOT: loop pairs x iters
    let mut aot_transact_ns: u128 = 0; let mut aot_commit_ns: u128 = 0;
    for i in 0..pairs {
        let addr = revm_primitives::Address::with_last_byte((i % 256) as u8);
        evm_aot.context.evm.env.tx.transact_to = TransactTo::Call(addr);
        evm_aot.context.evm.env.tx.data = selector.clone();
        evm_aot.context.evm.env.tx.nonce = Some(0);
        for _ in 0..iters {
            let t0 = Instant::now();
            let r = evm_aot.transact_preverified().unwrap();
            aot_transact_ns += t0.elapsed().as_nanos();
            let t1 = Instant::now();
            evm_aot.db_mut().commit(r.state);
            aot_commit_ns += t1.elapsed().as_nanos();
            evm_aot.context.evm.env.tx.nonce = evm_aot.context.evm.env.tx.nonce.map(|n| n + 1);
        }
    }

    // Bench Interpreter
    let mut int_transact_ns: u128 = 0; let mut int_commit_ns: u128 = 0;
    for i in 0..pairs {
        let addr = revm_primitives::Address::with_last_byte((i % 256) as u8);
        evm_plain.context.evm.env.tx.transact_to = TransactTo::Call(addr);
        evm_plain.context.evm.env.tx.data = selector.clone();
        evm_plain.context.evm.env.tx.nonce = Some(0);
        for _ in 0..iters {
            let t0 = Instant::now();
            let r = evm_plain.transact_preverified().unwrap();
            int_transact_ns += t0.elapsed().as_nanos();
            let t1 = Instant::now();
            evm_plain.db_mut().commit(r.state);
            int_commit_ns += t1.elapsed().as_nanos();
            evm_plain.context.evm.env.tx.nonce = evm_plain.context.evm.env.tx.nonce.map(|n| n + 1);
        }
    }

    let aot_total = std::time::Duration::from_nanos((aot_transact_ns + aot_commit_ns) as u64);
    let int_total = std::time::Duration::from_nanos((int_transact_ns + int_commit_ns) as u64);
    println!("pancake_pair_getreserves pairs={} iters={} | AOT avg={:?} total={:?} | Interpreter avg={:?} total={:?}",
             pairs, iters,
             std::time::Duration::from_nanos(((aot_transact_ns + aot_commit_ns) as u64) / (pairs*iters) as u64), aot_total,
             std::time::Duration::from_nanos(((int_transact_ns + int_commit_ns) as u64) / (pairs*iters) as u64), int_total);
}

