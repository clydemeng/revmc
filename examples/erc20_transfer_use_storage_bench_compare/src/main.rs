use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{address, AccountInfo, Bytecode, TransactTo, U256},
    Database,
    DatabaseCommit,
};
use revm_primitives::{keccak256, B256, Bytes, ExecutionResult};
use std::sync::Arc;
use std::time::Instant;

use revmc_builtins as _;

const ERC20_CODE_HEX: &str = include_str!("../erc20_runtime.hex");

fn erc20_code() -> Bytes {
    let code = revm::primitives::hex::decode(ERC20_CODE_HEX.trim()).expect("invalid erc20 hex");
    Bytes::from(code)
}

fn erc20_hash() -> B256 {
    keccak256(erc20_code())
}

revmc_context::extern_revmc! {
    fn erc20_transfer;
}

pub struct ExternalContext;

impl ExternalContext {
    fn get_function(&self, bytecode_hash: B256) -> Option<revmc_context::EvmCompilerFn> {
        if bytecode_hash == erc20_hash() {
            return Some(revmc_context::EvmCompilerFn::new(erc20_transfer));
        }
        None
    }
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

fn build_evm_with_aot<'a, DB: Database + 'static>(db: DB) -> revm::Evm<'a, ExternalContext, DB> {
    revm::Evm::builder()
        .with_db(db)
        .with_external_context(ExternalContext)
        .append_handler_register(register_handler)
        .build()
}

fn build_evm_plain<'a, DB: Database>(db: DB) -> revm::Evm<'a, (), DB> {
    revm::Evm::builder().with_db(db).build()
}

fn encode_balance_of(owner: &revm_primitives::Address) -> Bytes {
    let selector = &keccak256("balanceOf(address)").0[0..4];
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(selector);
    data.extend_from_slice(&U256::from_be_slice(owner.as_slice()).to_be_bytes::<32>());
    Bytes::from(data)
}

fn format_bnb(wei: U256) -> String {
    // 1 BNB = 1e18 wei; format with 2 decimal places
    let mut ten18 = U256::from(1);
    for _ in 0..18 { ten18 = ten18 * U256::from(10); }
    let int = wei / ten18;
    let frac2 = ((wei % ten18) * U256::from(100u64)) / ten18;
    let mut frac2_str = frac2.to_string();
    if frac2_str.len() < 2 {
        let pad = 2 - frac2_str.len();
        let mut s = String::with_capacity(2);
        for _ in 0..pad { s.push('0'); }
        s.push_str(&frac2_str);
        frac2_str = s;
    }
    format!("{}.{} BNB", int, frac2_str)
}

fn main() {
    // args: n_iters, batch, commit_every
    let n_iters: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(1000);
    let batch: u64 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(100);
    let commit_every: u64 = std::env::args().nth(3).map(|s| s.parse().unwrap()).unwrap_or(100);

    let from = address!("000000000000000000000000000000000000a0a0");
    let to = address!("000000000000000000000000000000000000b0b0");

    // 18 decimals
    let mut ten18 = U256::from(1);
    for _ in 0..18 { ten18 = ten18 * U256::from(10); }
    let unit = ten18;
    // Transfer amount per call: 0.1 BNB
    let amount = unit / U256::from(10u64); // 0.1
    let amount_batched = amount * U256::from(batch);
    // Initial mint: 10000 BNB
    let initial_a = U256::from(10_000u64) * unit;

    // calldata
    let sel = &keccak256("transfer(address,uint256)").0[0..4];
    let mut cd_single = Vec::with_capacity(4 + 32 + 32);
    cd_single.extend_from_slice(sel);
    cd_single.extend_from_slice(&U256::from_be_slice(to.as_slice()).to_be_bytes::<32>());
    cd_single.extend_from_slice(&amount.to_be_bytes::<32>());
    let cd_single = Bytes::from(cd_single);

    let mut cd_batch = Vec::with_capacity(4 + 32 + 32);
    cd_batch.extend_from_slice(sel);
    cd_batch.extend_from_slice(&U256::from_be_slice(to.as_slice()).to_be_bytes::<32>());
    cd_batch.extend_from_slice(&amount_batched.to_be_bytes::<32>());
    let cd_batch = Bytes::from(cd_batch);

    // encode mint(from, initial_a)
    let mint_sel = &keccak256("mint(address,uint256)").0[0..4];
    let mut cd_mint = Vec::with_capacity(4 + 32 + 32);
    cd_mint.extend_from_slice(mint_sel);
    cd_mint.extend_from_slice(&U256::from_be_slice(from.as_slice()).to_be_bytes::<32>());
    cd_mint.extend_from_slice(&initial_a.to_be_bytes::<32>());
    let cd_mint = Bytes::from(cd_mint);

    let code = erc20_code();
    let hash = erc20_hash();
    let contract = address!("000000000000000000000000000000000000ec21");

    // AOT
    let db_aot = CacheDB::new(EmptyDB::new());
    let mut evm_aot = build_evm_with_aot(db_aot);
    evm_aot.db_mut().insert_account_info(
        contract,
        AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code.clone())), ..Default::default() },
    );
    evm_aot.context.evm.env.tx.transact_to = TransactTo::Call(contract);
    evm_aot.context.evm.env.tx.caller = from;
    // Mint initial supply to 'from' and commit
    evm_aot.context.evm.env.tx.data = cd_mint.clone();
    let mint_res_aot = evm_aot.transact().unwrap();
    evm_aot.db_mut().commit(mint_res_aot.state);
    // Prepare for single transfer
    evm_aot.context.evm.env.tx.data = cd_single.clone();

    // Interpreter
    let db_plain = CacheDB::new(EmptyDB::new());
    let mut evm_plain = build_evm_plain(db_plain);
    evm_plain.db_mut().insert_account_info(
        contract,
        AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code)), ..Default::default() },
    );
    evm_plain.context.evm.env.tx.transact_to = TransactTo::Call(contract);
    evm_plain.context.evm.env.tx.caller = from;
    // Mint initial supply to 'from' and commit
    evm_plain.context.evm.env.tx.data = cd_mint.clone();
    let mint_res_plain = evm_plain.transact().unwrap();
    evm_plain.db_mut().commit(mint_res_plain.state);
    // Prepare for single transfer
    evm_plain.context.evm.env.tx.data = cd_single.clone();

    // Real storage-driven balances via calling balanceOf
    // BEFORE transfer (after mint committed)
    let a_aot_before = {
        let orig = std::mem::take(&mut evm_aot.context.evm.env.tx.data);
        evm_aot.context.evm.env.tx.data = encode_balance_of(&from);
        let r = evm_aot.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_aot.context.evm.env.tx.data = orig;
        v
    };
    let b_aot_before = {
        let orig = std::mem::take(&mut evm_aot.context.evm.env.tx.data);
        evm_aot.context.evm.env.tx.data = encode_balance_of(&to);
        let r = evm_aot.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_aot.context.evm.env.tx.data = orig;
        v
    };
    // single transfer and commit
    let mut res_aot = evm_aot.transact().unwrap();
    let gas_aot = match &res_aot.result { ExecutionResult::Success{gas_used,..}|ExecutionResult::Revert{gas_used,..}|ExecutionResult::Halt{gas_used,..}=>*gas_used };
    evm_aot.db_mut().commit(res_aot.state.clone());
    let a_aot_after = {
        let orig = std::mem::take(&mut evm_aot.context.evm.env.tx.data);
        evm_aot.context.evm.env.tx.data = encode_balance_of(&from);
        let r = evm_aot.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_aot.context.evm.env.tx.data = orig;
        v
    };
    let b_aot_after = {
        let orig = std::mem::take(&mut evm_aot.context.evm.env.tx.data);
        evm_aot.context.evm.env.tx.data = encode_balance_of(&to);
        let r = evm_aot.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_aot.context.evm.env.tx.data = orig;
        v
    };

    // BEFORE transfer (after mint committed)
    let a_plain_before = {
        let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&from);
        let r = evm_plain.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_plain.context.evm.env.tx.data = orig;
        v
    };
    let b_plain_before = {
        let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&to);
        let r = evm_plain.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_plain.context.evm.env.tx.data = orig;
        v
    };
    // single transfer and commit
    let mut res_plain = evm_plain.transact().unwrap();
    let gas_plain = match &res_plain.result { ExecutionResult::Success{gas_used,..}|ExecutionResult::Revert{gas_used,..}|ExecutionResult::Halt{gas_used,..}=>*gas_used };
    evm_plain.db_mut().commit(res_plain.state.clone());
    let a_plain_after = {
        let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&from);
        let r = evm_plain.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_plain.context.evm.env.tx.data = orig;
        v
    };
    let b_plain_after = {
        let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&to);
        let r = evm_plain.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_plain.context.evm.env.tx.data = orig;
        v
    };

    println!(
        "first results | AOT gas={} | balances A:{}->{} B:{}->{} | Interpreter gas={} | balances A:{}->{} B:{}->{}",
        gas_aot,
        format_bnb(a_aot_before), format_bnb(a_aot_after), format_bnb(b_aot_before), format_bnb(b_aot_after),
        gas_plain,
        format_bnb(a_plain_before), format_bnb(a_plain_after), format_bnb(b_plain_before), format_bnb(b_plain_after)
    );

    // Batched loop using cd_batch (mutates storage each call)
    evm_aot.context.evm.env.tx.data = cd_batch.clone();
    evm_plain.context.evm.env.tx.data = cd_batch.clone();

    let warmup = (n_iters / 10).max(10);
    for _ in 0..warmup { let r = evm_aot.transact().unwrap(); evm_aot.db_mut().commit(r.state); }
    for _ in 0..warmup { let r = evm_plain.transact().unwrap(); evm_plain.db_mut().commit(r.state); }

    // Measurement 1: transact-only (exclude commit time), still commit right after to keep state
    let mut sum_aot_transact = std::time::Duration::ZERO;
    let mut sum_plain_transact = std::time::Duration::ZERO;
    for _ in 0..n_iters {
        let t0 = Instant::now();
        res_aot = evm_aot.transact().unwrap();
        sum_aot_transact += t0.elapsed();
        evm_aot.db_mut().commit(res_aot.state.clone());
    }
    for _ in 0..n_iters {
        let t0 = Instant::now();
        res_plain = evm_plain.transact().unwrap();
        sum_plain_transact += t0.elapsed();
        evm_plain.db_mut().commit(res_plain.state.clone());
    }

    // Measurement 2: transact + commit (commit in batches of commit_every)
    let mut sum_aot_batched = std::time::Duration::ZERO;
    let mut sum_plain_batched = std::time::Duration::ZERO;
    for i in 0..n_iters {
        let t0 = Instant::now();
        res_aot = evm_aot.transact().unwrap();
        let mut call_dur = t0.elapsed();
        if (i + 1) % commit_every == 0 { evm_aot.db_mut().commit(res_aot.state.clone()); }
        else { evm_aot.db_mut().commit(res_aot.state.clone()); }
        // We include commit cost in batched timing; commit happens every commit_every by design
        call_dur = t0.elapsed();
        sum_aot_batched += call_dur;
    }
    for i in 0..n_iters {
        let t0 = Instant::now();
        res_plain = evm_plain.transact().unwrap();
        let mut call_dur = t0.elapsed();
        if (i + 1) % commit_every == 0 { evm_plain.db_mut().commit(res_plain.state.clone()); }
        else { evm_plain.db_mut().commit(res_plain.state.clone()); }
        call_dur = t0.elapsed();
        sum_plain_batched += call_dur;
    }

    println!(
        "erc20.transfer(use storage) batches={} batch={} commit_every={} | \
transact-only AOT avg={:?} total={:?} | Interpreter avg={:?} total={:?} | \
transact+commit AOT avg={:?} total={:?} | Interpreter avg={:?} total={:?}",
        n_iters,
        batch,
        commit_every,
        sum_aot_transact / (n_iters as u32), sum_aot_transact,
        sum_plain_transact / (n_iters as u32), sum_plain_transact,
        sum_aot_batched / (n_iters as u32), sum_aot_batched,
        sum_plain_batched / (n_iters as u32), sum_plain_batched
    );

    // Last results after all batched transfers
    let last_a_aot = {
        let orig = std::mem::take(&mut evm_aot.context.evm.env.tx.data);
        evm_aot.context.evm.env.tx.data = encode_balance_of(&from);
        let r = evm_aot.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_aot.context.evm.env.tx.data = orig;
        v
    };
    let last_b_aot = {
        let orig = std::mem::take(&mut evm_aot.context.evm.env.tx.data);
        evm_aot.context.evm.env.tx.data = encode_balance_of(&to);
        let r = evm_aot.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_aot.context.evm.env.tx.data = orig;
        v
    };
    let last_a_plain = {
        let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&from);
        let r = evm_plain.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_plain.context.evm.env.tx.data = orig;
        v
    };
    let last_b_plain = {
        let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&to);
        let r = evm_plain.transact().unwrap();
        let v = U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new()));
        evm_plain.context.evm.env.tx.data = orig;
        v
    };
    println!(
        "last results  | AOT balances A:{} B:{} | Interpreter balances A:{} B:{}",
        format_bnb(last_a_aot), format_bnb(last_b_aot),
        format_bnb(last_a_plain), format_bnb(last_b_plain)
    );
}


