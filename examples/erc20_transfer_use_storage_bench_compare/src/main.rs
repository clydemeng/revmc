use revm::{
    db::{CacheDB, EmptyDB},
    handler::register::EvmHandler,
    primitives::{address, AccountInfo, Bytecode, TransactTo, U256},
    Database,
    DatabaseCommit,
};
use revm_primitives::{keccak256, B256, Bytes, ExecutionResult, SpecId};
use revmc::{EvmCompiler, EvmLlvmBackend, OptimizationLevel};
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

revmc_context::extern_revmc! { fn erc20_transfer; }

pub struct ExternalContext { jit_fn: Option<revmc_context::EvmCompilerFn> }

impl ExternalContext {
    fn new(jit_fn: Option<revmc_context::EvmCompilerFn>) -> Self { Self { jit_fn } }
    fn get_function(&self, bytecode_hash: B256) -> Option<revmc_context::EvmCompilerFn> {
        if bytecode_hash != erc20_hash() { return None; }
        if let Some(f) = self.jit_fn { Some(f) } else { Some(revmc_context::EvmCompilerFn::new(erc20_transfer)) }
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

fn build_evm_with_ctx<'a, DB: Database + 'static>(db: DB, ctx: ExternalContext) -> revm::Evm<'a, ExternalContext, DB> {
    revm::Evm::builder().with_db(db).with_external_context(ctx).append_handler_register(register_handler).build()
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
    // args: mode(aot|jit|interp), n_iters, batch, commit_every
    let mode = std::env::args().nth(1).unwrap_or_else(|| "aot".into());
    let n_iters: u64 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(1000);
    let batch: u64 = std::env::args().nth(3).map(|s| s.parse().unwrap()).unwrap_or(100);
    let commit_every: u64 = std::env::args().nth(4).map(|s| s.parse().unwrap()).unwrap_or(1);

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

    // Prepare selected engine (AOT/JIT via external context; interpreter separately)
    // Compile JIT if requested
    let mut jit_fn: Option<revmc_context::EvmCompilerFn> = None;
    if mode == "jit" {
        // Leak context and compiler so the JITted function pointer remains valid for the whole run.
        let cx = Box::leak(Box::new(revmc::llvm::inkwell::context::Context::create()));
        let backend = EvmLlvmBackend::new(cx, false, OptimizationLevel::Aggressive).expect("jit backend");
        let compiler = Box::leak(Box::new(EvmCompiler::new(backend)));
        let f = unsafe { compiler.jit("erc20", &code[..], SpecId::CANCUN) }.expect("jit compile");
        jit_fn = Some(f);
    }

    // External engine (AOT or JIT depending on jit_fn)
    let db_ext = CacheDB::new(EmptyDB::new());
    let mut evm_ext = build_evm_with_ctx(db_ext, ExternalContext::new(jit_fn));
    evm_ext.db_mut().insert_account_info(
        contract,
        AccountInfo { code_hash: hash, code: Some(Bytecode::new_raw(code.clone())), ..Default::default() },
    );
    evm_ext.context.evm.env.tx.transact_to = TransactTo::Call(contract);
    evm_ext.context.evm.env.tx.caller = from;
    // Mint to 'from' and commit
    evm_ext.context.evm.env.tx.data = cd_mint.clone();
    let mint_res_ext = evm_ext.transact().unwrap();
    evm_ext.db_mut().commit(mint_res_ext.state);
    // Prepare for single transfer
    evm_ext.context.evm.env.tx.data = cd_single.clone();

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

    // FIRST: per selected mode
    let (a_before, b_before, a_after, b_after, first_gas) = if mode == "interp" {
        let orig: Bytes = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
        evm_plain.context.evm.env.tx.data = encode_balance_of(&from);
        let av = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_plain.context.evm.env.tx.data = encode_balance_of(&to);
        let bv = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_plain.context.evm.env.tx.data = cd_single.clone();
        let res = evm_plain.transact().unwrap();
        let gas = match &res.result { ExecutionResult::Success{gas_used,..}|ExecutionResult::Revert{gas_used,..}|ExecutionResult::Halt{gas_used,..}=>*gas_used };
        evm_plain.db_mut().commit(res.state.clone());
        evm_plain.context.evm.env.tx.data = encode_balance_of(&from);
        let av2 = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_plain.context.evm.env.tx.data = encode_balance_of(&to);
        let bv2 = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_plain.context.evm.env.tx.data = orig;
        (av, bv, av2, bv2, gas)
    } else {
        let orig = std::mem::take(&mut evm_ext.context.evm.env.tx.data);
        evm_ext.context.evm.env.tx.data = encode_balance_of(&from);
        let av = { let r = evm_ext.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_ext.context.evm.env.tx.data = encode_balance_of(&to);
        let bv = { let r = evm_ext.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_ext.context.evm.env.tx.data = cd_single.clone();
        let res = evm_ext.transact().unwrap();
        let gas = match &res.result { ExecutionResult::Success{gas_used,..}|ExecutionResult::Revert{gas_used,..}|ExecutionResult::Halt{gas_used,..}=>*gas_used };
        evm_ext.db_mut().commit(res.state.clone());
        evm_ext.context.evm.env.tx.data = encode_balance_of(&from);
        let av2 = { let r = evm_ext.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_ext.context.evm.env.tx.data = encode_balance_of(&to);
        let bv2 = { let r = evm_ext.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
        evm_ext.context.evm.env.tx.data = orig;
        (av, bv, av2, bv2, gas)
    };

    println!(
        "first results | mode={} gas={} | balances A:{}->{} B:{}->{}",
        mode, first_gas, format_bnb(a_before), format_bnb(a_after), format_bnb(b_before), format_bnb(b_after)
    );

    // Batched transact+commit for selected mode
    let warmup = (n_iters / 10).max(10);
    match mode.as_str() {
        "interp" => {
            evm_plain.context.evm.env.tx.data = cd_batch.clone();
            for _ in 0..warmup { let r = evm_plain.transact().unwrap(); evm_plain.db_mut().commit(r.state); }
            let t = Instant::now();
            for i in 0..n_iters {
                let r = evm_plain.transact().unwrap();
                evm_plain.db_mut().commit(r.state);
                if (i + 1) % commit_every == 0 {}
            }
            let elapsed = t.elapsed();
            println!("erc20.transfer(use storage) mode=interp batches={} batch={} commit_every={} | avg/call={:?} total={:?}", n_iters, batch, commit_every, elapsed / (n_iters as u32), elapsed);
        }
        "jit" | "aot" => {
            evm_ext.context.evm.env.tx.data = cd_batch.clone();
            for _ in 0..warmup { let r = evm_ext.transact().unwrap(); evm_ext.db_mut().commit(r.state); }
            let t = Instant::now();
            for i in 0..n_iters {
                let r = evm_ext.transact().unwrap();
                evm_ext.db_mut().commit(r.state);
                if (i + 1) % commit_every == 0 {}
            }
            let elapsed = t.elapsed();
            println!("erc20.transfer(use storage) mode={} batches={} batch={} commit_every={} | avg/call={:?} total={:?}", mode, n_iters, batch, commit_every, elapsed / (n_iters as u32), elapsed);
        }
        _ => eprintln!("unknown mode: {} (use aot|jit|interp)", mode),
    }

    // Last results after all batched transfers
    // LAST: print balances for selected mode
    match mode.as_str() {
        "interp" => {
            let orig = std::mem::take(&mut evm_plain.context.evm.env.tx.data);
            evm_plain.context.evm.env.tx.data = encode_balance_of(&from);
            let a = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
            evm_plain.context.evm.env.tx.data = encode_balance_of(&to);
            let b = { let r = evm_plain.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
            evm_plain.context.evm.env.tx.data = orig;
            println!("last results  | mode=interp balances A:{} B:{}", format_bnb(a), format_bnb(b));
        }
        "jit" | "aot" => {
            let orig = std::mem::take(&mut evm_ext.context.evm.env.tx.data);
            evm_ext.context.evm.env.tx.data = encode_balance_of(&from);
            let a = { let r = evm_ext.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
            evm_ext.context.evm.env.tx.data = encode_balance_of(&to);
            let b = { let r = evm_ext.transact().unwrap(); U256::from_be_slice(r.result.output().unwrap_or(&Bytes::new())) };
            evm_ext.context.evm.env.tx.data = orig;
            println!("last results  | mode={} balances A:{} B:{}", mode, format_bnb(a), format_bnb(b));
        }
        _ => {}
    }
}


