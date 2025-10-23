use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{AccountInfo, Bytecode},
    Database,
    DatabaseCommit,
};
use revm_primitives::{address, keccak256, B256, Bytes, TransactTo, U256};
use std::time::Instant;

use revmc_builtins as _;

fn code() -> Bytes { Bytes::from(std::fs::read("/Users/ruojunm/workspace_2025/rust/remvc_workspace/const/const-revmc/examples/wbnb/wbnb.runtime.bin").expect("read wbnb.runtime.bin")) }
fn code_hash() -> B256 { keccak256(code()) }

revmc_context::extern_revmc! { fn wbnb; }

pub struct ExternalContext { target_hash: B256, aot_fn: revmc_context::EvmCompilerFn }
impl ExternalContext {
    #[inline]
    fn get_function(&self, bytecode_hash: B256) -> Option<revmc_context::EvmCompilerFn> {
        if bytecode_hash == self.target_hash { Some(self.aot_fn) } else { None }
    }
}

fn register_handler<DB: Database + 'static>(handler: &mut EvmHandler<'_, ExternalContext, DB>) {
    let prev = handler.execution.execute_frame.clone();
    handler.execution.execute_frame = std::sync::Arc::new(move |frame, memory, tables, context| {
        let interpreter = frame.interpreter_mut();
        let h = interpreter.contract.hash.unwrap_or_default();
        if let Some(f) = context.external.get_function(h) { Ok(unsafe { f.call_with_interpreter_and_memory(interpreter, memory, context) }) } else { prev(frame, memory, tables, context) }
    });
}

fn build_evm_with_aot<'a, DB: Database + 'static>(db: DB, target_hash: B256) -> revm::Evm<'a, ExternalContext, DB> {
    let aot = revmc_context::EvmCompilerFn::new(wbnb);
    revm::Evm::builder()
        .with_db(db)
        .with_external_context(ExternalContext { target_hash, aot_fn: aot })
        .append_handler_register(register_handler)
        .build()
}

fn encode_transfer(to: revm_primitives::Address, amount: U256) -> Bytes {
    let mut data = Vec::with_capacity(4 + 32 + 32);
    data.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(to.as_slice());
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
}

fn encode_deposit() -> Bytes { Bytes::from(vec![0xd0, 0xe3, 0x0d, 0xb0]) }

fn encode_balance_of(owner: revm_primitives::Address) -> Bytes {
    // precomputed selector for balanceOf(address)
    const SELECTOR: [u8;4] = [0x70, 0xa0, 0x82, 0x31];
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&SELECTOR);
    data.extend_from_slice(&U256::from_be_slice(owner.as_slice()).to_be_bytes::<32>());
    Bytes::from(data)
}

fn main() {
    // args: iters (number of transfers)
    let iters: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(5000);
    let contract = address!("0000000000000000000000000000000000009999");
    let from = address!("0000000000000000000000000000000000000001");
    let to = address!("000000000000000000000000000000000000BEEF");
    // variable per-iter amount; we'll compute 1,2,3,... later
    let warmup = 0u64;

    let code_bytes = code();
    let hash = code_hash();

    // Compute total initial deposit needed: sum_{k=1..iters} k = iters*(iters+1)/2
    let initial = (U256::from(iters) * U256::from(iters + 1)) / U256::from(2u64);

    // Build AOT EVM
    let db_aot = CacheDB::new(EmptyDB::new());
    let mut evm_aot = build_evm_with_aot(db_aot, hash);
    evm_aot.db_mut().insert_account_info(contract, AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code_bytes.clone())), ..Default::default() });
    evm_aot.context.evm.env.tx.transact_to = TransactTo::Call(contract);
    evm_aot.context.evm.env.tx.caller = from;
    evm_aot.context.evm.env.tx.gas_limit = 30_000_000;
    evm_aot.context.evm.env.tx.gas_price = U256::ZERO;
    evm_aot.context.evm.env.block.basefee = U256::ZERO;
    evm_aot.context.evm.env.tx.nonce = Some(0u64);

    // Build Interpreter EVM
    let db_plain = CacheDB::new(EmptyDB::new());
    let mut evm_plain = revm::Evm::builder().with_db(db_plain).build();
    evm_plain.db_mut().insert_account_info(contract, AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code_bytes)), ..Default::default() });
    evm_plain.context.evm.env.tx.transact_to = TransactTo::Call(contract);
    evm_plain.context.evm.env.tx.caller = from;
    evm_plain.context.evm.env.tx.gas_limit = 30_000_000;
    evm_plain.context.evm.env.tx.gas_price = U256::ZERO;
    evm_plain.context.evm.env.block.basefee = U256::ZERO;
    evm_plain.context.evm.env.tx.nonce = Some(0u64);

    // Fund caller to cover msg.value for deposit
    evm_aot.db_mut().insert_account_info(from, AccountInfo { balance: initial, ..Default::default() });
    evm_plain.db_mut().insert_account_info(from, AccountInfo { balance: initial, ..Default::default() });

    // Initialize balance via deposit (msg.value)
    // AOT deposit
    evm_aot.context.evm.env.tx.data = encode_deposit();
    evm_aot.context.evm.env.tx.value = initial;
    let r1 = evm_aot.transact().unwrap();
    evm_aot.db_mut().commit(r1.state);
    evm_aot.context.evm.env.tx.value = U256::ZERO;
    evm_aot.context.evm.env.tx.nonce = evm_aot.context.evm.env.tx.nonce.map(|n| n + 1);
    // Interpreter deposit
    evm_plain.context.evm.env.tx.data = encode_deposit();
    evm_plain.context.evm.env.tx.value = initial;
    let r2 = evm_plain.transact().unwrap();
    evm_plain.db_mut().commit(r2.state);
    evm_plain.context.evm.env.tx.value = U256::ZERO;
    evm_plain.context.evm.env.tx.nonce = evm_plain.context.evm.env.tx.nonce.map(|n| n + 1);

    // Prepare transfer calldata
    // we will set per-iter calldata inside the loops (1,2,3,...)

    // Measure AOT (split transact vs commit), use preverified to reduce validation overhead
    let mut aot_transact_ns: u128 = 0;
    let mut aot_commit_ns: u128 = 0;
    for i in 0..iters {
        // set variable amount (1 wei, 2 wei, ...)
        evm_aot.context.evm.env.tx.data = encode_transfer(to, U256::from(i + 1));
        let t0 = Instant::now();
        let r = evm_aot.transact_preverified().unwrap();
        aot_transact_ns += t0.elapsed().as_nanos();
        let t1 = Instant::now();
        evm_aot.db_mut().commit(r.state);
        aot_commit_ns += t1.elapsed().as_nanos();
        evm_aot.context.evm.env.tx.nonce = evm_aot.context.evm.env.tx.nonce.map(|n| n + 1);
    }
    let d_aot = std::time::Duration::from_nanos((aot_transact_ns + aot_commit_ns) as u64);

    // Measure Interpreter (split) using preverified
    let mut int_transact_ns: u128 = 0;
    let mut int_commit_ns: u128 = 0;
    for i in 0..iters {
        evm_plain.context.evm.env.tx.data = encode_transfer(to, U256::from(i + 1));
        let t0 = Instant::now();
        let r = evm_plain.transact_preverified().unwrap();
        int_transact_ns += t0.elapsed().as_nanos();
        let t1 = Instant::now();
        evm_plain.db_mut().commit(r.state);
        int_commit_ns += t1.elapsed().as_nanos();
        evm_plain.context.evm.env.tx.nonce = evm_plain.context.evm.env.tx.nonce.map(|n| n + 1);
    }
    let d_interp = std::time::Duration::from_nanos((int_transact_ns + int_commit_ns) as u64);

    // Verify balances
    let bal_of_from = encode_balance_of(from);
    let bal_of_to = encode_balance_of(to);

    evm_aot.context.evm.env.tx.data = bal_of_from.clone();
    let av_a = { let r = evm_aot.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
    evm_aot.context.evm.env.tx.data = bal_of_to.clone();
    let bv_a = { let r = evm_aot.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };

    println!("AOT balances: from={} to={}", av_a, bv_a);

    evm_plain.context.evm.env.tx.data = bal_of_from.clone();
    let av_i = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
    evm_plain.context.evm.env.tx.data = bal_of_to.clone();
    let bv_i = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };

    let moved = (U256::from(iters) * U256::from(iters + 1)) / U256::from(2u64);
    let ok_aot = av_a == U256::ZERO && bv_a == moved;
    let ok_int = av_i == U256::ZERO && bv_i == moved;

    println!("Interpreter balances: from={} to={}", av_i, bv_i);

    println!(
        "wbnb n_iters={} | AOT avg={:?} total={:?} (transact={:?}, commit={:?}) verify={} | Interpreter avg={:?} total={:?} (transact={:?}, commit={:?}) verify={}",
        iters,
        d_aot / (iters as u32), d_aot, std::time::Duration::from_nanos((aot_transact_ns as u64) / (iters as u64)), std::time::Duration::from_nanos((aot_commit_ns as u64) / (iters as u64)), ok_aot,
        d_interp / (iters as u32), d_interp, std::time::Duration::from_nanos((int_transact_ns as u64) / (iters as u64)), std::time::Duration::from_nanos((int_commit_ns as u64) / (iters as u64)), ok_int
    );
}


