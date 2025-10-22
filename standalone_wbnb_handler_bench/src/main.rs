use eyre::Result;
use revm_context::ContextTr;
use revmc_builtins as _;
use revm_database::InMemoryDB;
use revm_handler::{ExecuteCommitEvm, MainBuilder, MainContext};
use revm_primitives::{address, Address, Bytes, TxKind, U256, keccak256};
use revm_state::AccountInfo;
use revm_bytecode::Bytecode;
use std::time::Instant;

fn wbnb_code() -> Bytes {
    Bytes::from(std::fs::read("/Users/ruojunm/workspace_2025/rust/remvc_workspace/const/const-revmc/examples/wbnb/wbnb.runtime.bin").expect("read wbnb.runtime.bin"))
}

fn encode_transfer(to: Address, amount: U256) -> Bytes {
    let mut data = Vec::with_capacity(4 + 32 + 32);
    data.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(to.as_slice());
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
}

fn encode_deposit() -> Bytes { Bytes::from(vec![0xd0, 0xe3, 0x0d, 0xb0]) }

fn encode_balance_of(who: Address) -> Bytes {
    let mut data = Vec::with_capacity(4 + 32);
    // balanceOf(address)
    data.extend_from_slice(&[0x70, 0xa0, 0x82, 0x31]);
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(who.as_slice());
    Bytes::from(data)
}

fn main() -> Result<()> {
    let iters: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(5000);
    let from = address!("000000000000000000000000000000000000a0a0");
    let to = address!("000000000000000000000000000000000000BEEF");
    let contract = address!("0000000000000000000000000000000000009999");
    let amount = U256::from(1u64);
    let warmup = 100u64;

    let code = wbnb_code();
    let hash = keccak256(code.clone());

    // AOT integrate via execute-frame override
    revmc_context::extern_revmc! { fn wbnb; }
    fn register_handler<DB: revm::Database + 'static>(handler: &mut revm::handler::register::EvmHandler<'_, (), DB>) {
        let prev = handler.execution.execute_frame.clone();
        handler.execution.execute_frame = std::sync::Arc::new(move |frame, memory, tables, context| {
            let interpreter = frame.interpreter_mut();
            let h = interpreter.contract.hash.unwrap_or_default();
            // Use wbnb if code hash matches the loaded code
            if h == keccak256(wbnb_code()) { Ok(unsafe { revmc_context::EvmCompilerFn::new(wbnb).call_with_interpreter_and_memory(interpreter, memory, context) }) } else { prev(frame, memory, tables, context) }
        });
    }
    // rebuild evm with aot handler
    let mut evm = evm
        .modify()
        .append_handler_register(register_handler)
        .build();

    // Build MainnetContext with InMemoryDB
    let ctx = revm_context::Context::mainnet().with_db(InMemoryDB::default());
    let mut evm = ctx.build_mainnet();

    // Insert code and fund caller
    let mut info = AccountInfo::default();
    let b = Bytecode::new_raw(code.clone());
    info.set_code_and_hash(b, hash);
    evm.ctx.db_mut().insert_account_info(contract, info);
    let initial = amount * U256::from(iters + warmup);
    evm.ctx.db_mut().insert_account_info(from, AccountInfo { balance: initial, ..Default::default() });

    // Deposit via msg.value
    {
        let tx = revm_context::TxEnv::builder()
            .caller(from)
            .kind(TxKind::Call(contract))
            .data(encode_deposit())
            .gas_limit(30_000_000)
            .gas_price(0)
            .value(initial)
            .nonce(0)
            .build().unwrap();
        let _ = evm.transact_commit(tx)?;
    }

    // Warmup
    for i in 0..warmup {
        let tx = revm_context::TxEnv::builder()
            .caller(from)
            .kind(TxKind::Call(contract))
            .data(encode_transfer(to, amount))
            .gas_limit(30_000_000)
            .gas_price(0)
            .nonce(1 + i)
            .build().unwrap();
        let _ = evm.transact_commit(tx)?;
    }

    // Timed loop
    let t = Instant::now();
    for i in 0..iters {
        let tx = revm_context::TxEnv::builder()
            .caller(from)
            .kind(TxKind::Call(contract))
            .data(encode_transfer(to, amount))
            .gas_limit(30_000_000)
            .gas_price(0)
            //.nonce(0)
            .nonce(1 + warmup + i)
            .build().unwrap();
        let _ = evm.transact_commit(tx)?;
    }
    let d = t.elapsed();

    // Verify token balances via balanceOf for A and B
    let qnonce = 1 + warmup + iters;
    // Query A
    let tx_a = revm_context::TxEnv::builder()
        .caller(from)
        .kind(TxKind::Call(contract))
        .data(encode_balance_of(from))
        .gas_limit(30_000_000)
        .gas_price(0)
        .nonce(qnonce)
        .build().unwrap();
    let out_a = evm.transact_commit(tx_a)?;
    let a_bal = out_a.output().map(|b| U256::from_be_slice(b.as_ref())).unwrap_or(U256::ZERO);

    // Query B
    let tx_b = revm_context::TxEnv::builder()
        .caller(from)
        .kind(TxKind::Call(contract))
        .data(encode_balance_of(to))
        .gas_limit(30_000_000)
        .gas_price(0)
        .nonce(qnonce + 1)
        .build().unwrap();
    let out_b = evm.transact_commit(tx_b)?;
    let b_bal = out_b.output().map(|b| U256::from_be_slice(b.as_ref())).unwrap_or(U256::ZERO);

    println!(
        "standalone handler | n_iters={} | Interpreter avg={:?} total={:?} | A bal={} B bal={}",
        iters,
        d / (iters as u32),
        d,
        a_bal,
        b_bal
    );
    Ok(())
}


