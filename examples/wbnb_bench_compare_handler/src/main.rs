use revm_database::InMemoryDB;
use revm_handler::{ExecuteCommitEvm, MainnetContext, MainnetEvm};
use revm_primitives::{address, Address, B256, Bytes, TxKind, U256, Bytecode, SpecId, TxEnv};
use std::time::Instant;

use revmc_builtins as _;

fn code() -> Bytes { Bytes::from(std::fs::read("/Users/ruojunm/workspace_2025/rust/remvc_workspace/const/const-revmc/examples/wbnb/wbnb.runtime.bin").expect("read wbnb.runtime.bin")) }
fn code_hash(code: &[u8]) -> B256 { Bytecode::new_raw(Bytes::copy_from_slice(code)).hash_slow() }

fn encode_transfer(to: Address, amount: U256) -> Bytes {
    let mut data = Vec::with_capacity(4 + 32 + 32);
    data.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(to.as_slice());
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
}

fn encode_deposit() -> Bytes { Bytes::from(vec![0xd0, 0xe3, 0x0d, 0xb0]) }

fn main() {
    let iters: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(5000);
    let contract = address!("0000000000000000000000000000000000009999");
    let from = revm::database::BENCH_CALLER; // same as teammate
    let to = address!("000000000000000000000000000000000000BEEF");
    let amount = U256::from(1u64);
    let warmup = 100u64;

    let code = code();
    let hash = code_hash(&code);

    // Build mainnet context EVM
    let ctx = MainnetContext::new(InMemoryDB::default(), SpecId::default());
    let mut evm: MainnetEvm<(), InMemoryDB> = revm_handler::MainnetEvm::new(ctx, revm_handler::instructions::EthInstructions::new_mainnet(), revm_handler::precompile_provider::EthPrecompiles::default());

    // Insert code into state
    let mut info = revm_primitives::AccountInfo::default();
    let b = Bytecode::new_raw(code.clone());
    info.set_code_and_hash(b, hash);
    evm.ctx.db_mut().insert_account_info(contract, info);

    // Fund caller and deposit
    let initial = amount * U256::from(iters + warmup);
    evm.ctx.db_mut().insert_account_info(from, revm_primitives::AccountInfo { balance: initial, ..Default::default() });

    // deposit via msg.value
    {
        let tx = TxEnv::builder()
            .caller(from)
            .kind(TxKind::Call(contract))
            .data(encode_deposit())
            .gas_limit(30_000_000)
            .gas_price(U256::ZERO)
            .value(initial)
            .nonce(0)
            .build()
            .unwrap();
        let _ = evm.transact_commit(tx).unwrap();
    }

    // Warmup
    for i in 0..warmup {
        let tx = TxEnv::builder()
            .caller(from)
            .kind(TxKind::Call(contract))
            .data(encode_transfer(to, amount))
            .gas_limit(30_000_000)
            .gas_price(U256::ZERO)
            .nonce(1 + i)
            .build()
            .unwrap();
        let _ = evm.transact_commit(tx).unwrap();
    }

    // Timed loop
    let t = Instant::now();
    for i in 0..iters {
        let tx = TxEnv::builder()
            .caller(from)
            .kind(TxKind::Call(contract))
            .data(encode_transfer(to, amount))
            .gas_limit(30_000_000)
            .gas_price(U256::ZERO)
            .nonce(1 + warmup + i)
            .build()
            .unwrap();
        let _ = evm.transact_commit(tx).unwrap();
    }
    let d = t.elapsed();

    println!("wbnb-handler n_iters={} | Interpreter avg={:?} total={:?}", iters, d / (iters as u32), d);
}


