use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{address, AccountInfo, Bytecode, TransactTo, U256},
    Database,
};
use revm_primitives::{B256, ExecutionResult};
use std::time::Instant;
use std::sync::Arc;

// Ensure builtins are linked for compiled bytecodes
use revmc_builtins as _;

// Provide fibonacci bytecode and hash (same as examples/runner/src/common.rs)
const FIBONACCI_CODE: &[u8] = &[
    0x5f, 0x35, 0x5f, 0x60, 0x01, 0x5b, 0x82, 0x15, 0x60, 0x1a, 0x57, 0x81, 0x81, 0x01, 0x91,
    0x50, 0x90, 0x91, 0x60, 0x01, 0x90, 0x03, 0x91, 0x60, 0x05, 0x56, 0x5b, 0x91, 0x50, 0x50, 0x5f,
    0x52, 0x60, 0x20, 0x5f, 0xf3,
];
const FIBONACCI_HASH: [u8; 32] = [
    0xab, 0x1a, 0xd1, 0x21, 0x10, 0x02, 0xe1, 0xdd, 0xb8, 0xd9, 0xa4, 0xef, 0x58, 0xa9, 0x02, 0x22,
    0x48, 0x51, 0xf6, 0xa0, 0x27, 0x3e, 0xe3, 0xe8, 0x72, 0x76, 0xa8, 0xd2, 0x1e, 0x64, 0x9c, 0xe8,
];

// Statically linked AOT-compiled symbol
revmc_context::extern_revmc! {
    fn fibonacci;
}

pub struct ExternalContext;

impl ExternalContext {
    fn get_function(&self, bytecode_hash: B256) -> Option<revmc_context::EvmCompilerFn> {
        if bytecode_hash == B256::from(FIBONACCI_HASH) {
            return Some(revmc_context::EvmCompilerFn::new(fibonacci));
        }
        None
    }
}

fn build_evm_with_aot<'a, DB: Database + 'static>(db: DB) -> revm::Evm<'a, ExternalContext, DB> {
    revm::Evm::builder()
        .with_db(db)
        .with_external_context(ExternalContext)
        .append_handler_register(register_handler)
        .build()
}

fn register_handler<DB: Database + 'static>(handler: &mut EvmHandler<'_, ExternalContext, DB>) {
    let prev = handler.execution.execute_frame.clone();
    handler.execution.execute_frame = Arc::new(move |frame, memory, tables, context| {
        let interpreter = frame.interpreter_mut();
        let bytecode_hash = interpreter.contract.hash.unwrap_or_default();
        if let Some(f) = context.external.get_function(bytecode_hash) {
            Ok(unsafe { f.call_with_interpreter_and_memory(interpreter, memory, context) })
        } else {
            prev(frame, memory, tables, context)
        }
    });
}

fn build_evm_plain<'a, DB: revm::Database>(db: DB) -> revm::Evm<'a, (), DB> {
    revm::Evm::builder().with_db(db).build()
}

fn main() {
    // Hardcode fib(255) and accept only n_iters as optional CLI arg.
    let n_iters: u64 = std::env::args()
        .nth(1)
        .map(|s| s.parse().unwrap())
        .unwrap_or(1000);
    let num = U256::from(255);
    // The bytecode runs fib(input + 1), so pass 254 to compute fib(255).
    let actual_num = num.saturating_sub(U256::from(1));

    let fib_addr = address!("0000000000000000000000000000000000001235");

    // AOT path
    let db_aot = CacheDB::new(EmptyDB::new());
    let mut evm_aot = build_evm_with_aot(db_aot);
    evm_aot.db_mut().insert_account_info(
        fib_addr,
        AccountInfo {
            code_hash: B256::from(FIBONACCI_HASH),
            code: Some(Bytecode::new_raw(FIBONACCI_CODE.into())),
            ..Default::default()
        },
    );
    evm_aot.context.evm.env.tx.transact_to = TransactTo::Call(fib_addr);
    evm_aot.context.evm.env.tx.data = actual_num.to_be_bytes_vec().into();
    // First run to capture the expected output
    let mut res_aot = evm_aot.transact().unwrap();
    let out_aot = U256::from_be_slice(res_aot.result.output().unwrap());
    let gas_aot = match &res_aot.result {
        ExecutionResult::Success { gas_used, .. }
        | ExecutionResult::Revert { gas_used, .. }
        | ExecutionResult::Halt { gas_used, .. } => *gas_used,
    };

    // Warmup for AOT
    let warmup = (n_iters / 10).max(10);
    for _ in 0..warmup {
        evm_aot.context.evm.env.tx.data = actual_num.to_be_bytes_vec().into();
        let _ = evm_aot.transact().unwrap();
    }

    // Timed AOT runs
    let start_aot = Instant::now();
    for _ in 0..n_iters {
        evm_aot.context.evm.env.tx.data = actual_num.to_be_bytes_vec().into();
        res_aot = evm_aot.transact().unwrap();
    }
    let elapsed_aot = start_aot.elapsed();

    // Interpreter path (no AOT handler)
    let db_plain = CacheDB::new(EmptyDB::new());
    let mut evm_plain = build_evm_plain(db_plain);
    evm_plain.db_mut().insert_account_info(
        fib_addr,
        AccountInfo {
            code_hash: B256::from(FIBONACCI_HASH),
            code: Some(Bytecode::new_raw(FIBONACCI_CODE.into())),
            ..Default::default()
        },
    );
    evm_plain.context.evm.env.tx.transact_to = TransactTo::Call(fib_addr);
    evm_plain.context.evm.env.tx.data = actual_num.to_be_bytes_vec().into();
    // First run to capture interpreter output
    let mut res_plain = evm_plain.transact().unwrap();
    let out_plain = U256::from_be_slice(res_plain.result.output().unwrap());
    let gas_plain = match &res_plain.result {
        ExecutionResult::Success { gas_used, .. }
        | ExecutionResult::Revert { gas_used, .. }
        | ExecutionResult::Halt { gas_used, .. } => *gas_used,
    };

    // Print first iteration results (from the initial runs)
    println!(
        "first results | AOT={} (gas={}) | Interpreter={} (gas={})",
        out_aot, gas_aot, out_plain, gas_plain
    );

    // Warmup for interpreter
    for _ in 0..warmup {
        evm_plain.context.evm.env.tx.data = actual_num.to_be_bytes_vec().into();
        let _ = evm_plain.transact().unwrap();
    }

    // Timed interpreter runs
    let start_plain = Instant::now();
    for _ in 0..n_iters {
        evm_plain.context.evm.env.tx.data = actual_num.to_be_bytes_vec().into();
        res_plain = evm_plain.transact().unwrap();
    }
    let elapsed_plain = start_plain.elapsed();

    println!(
        "fib({}) n_iters={} | AOT avg={:?} total={:?} | Interpreter avg={:?} total={:?}",
        num,
        n_iters,
        elapsed_aot / (n_iters as u32),
        elapsed_aot,
        elapsed_plain / (n_iters as u32),
        elapsed_plain
    );

    if out_aot != out_plain {
        eprintln!("warning: results differ: AOT={} vs INTERP={}", out_aot, out_plain);
    }
}


