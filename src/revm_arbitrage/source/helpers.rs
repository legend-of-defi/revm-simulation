use alloy::{
    network::Ethereum, primitives::{Address, Bytes, TxKind, U256}, providers::{Provider, ProviderBuilder, RootProvider}, rpc::types::{TransactionInput, TransactionRequest}, sol_types::SolValue, transports::http::{Client, Http}
};
use anyhow::{anyhow, Result};
use revm::primitives::{keccak256, AccountInfo, Bytecode, Account, B256, EvmStorageSlot};
use revm::{
    db::{CacheDB, DatabaseCommit, DatabaseRef},
    primitives::{ExecutionResult, Output, TransactTo},
    Database, Evm,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use revm::db::AlloyDB;
use std::collections::HashMap;
use alloy::network::TransactionBuilder;
use alloy::primitives::map::foldhash::fast::RandomState;

pub const ONE_ETHER: U256 = U256::from_limbs([1_000_000_000_000_000_000u64, 0, 0, 0]);

pub fn measure_start(label: &str) -> (String, Instant) {
    (label.to_string(), Instant::now())
}

pub fn measure_end(start: (String, Instant)) -> Duration {
    let elapsed = start.1.elapsed();
    println!("Elapsed: {:.2?} for '{}'", elapsed, start.0);
    elapsed
}

pub fn volumes(from: U256, to: U256, count: usize) -> Vec<U256> {
    let start = U256::ZERO;
    let mut volumes = Vec::new();
    let distance = to - from;
    let step = distance / U256::from(count);

    for i in 1..(count + 1) {
        let current = start + step * U256::from(i);
        volumes.push(current);
    }

    volumes.reverse();
    volumes
}

pub fn build_tx(to: Address, from: Address, calldata: Bytes, base_fee: u128) -> TransactionRequest {
    let mut tx = TransactionRequest::default();
    tx.to = Some(TxKind::Call(to));
    tx.from = Some(from);
    tx.input = TransactionInput::from(calldata);
    tx.nonce = Some(0);
    tx.gas = Some(1000000);
    tx.max_fee_per_gas = Some(base_fee);
    tx.max_priority_fee_per_gas = Some(0);
    tx
}

pub type 
AlloyCacheDB = CacheDB<AlloyDB<Http<Client>, Ethereum, Arc<RootProvider<Http<Client>>>>>;

pub fn revm_call(
    from: Address,
    to: Address,
    calldata: Bytes,
    cache_db: &mut AlloyCacheDB,
) -> Result<Bytes> {
    let mut evm = Evm::builder()
        .with_db(cache_db)
        .modify_tx_env(|tx| {
            tx.caller = from;
            tx.transact_to = TransactTo::Call(to);
            tx.data = calldata;
            tx.value = U256::ZERO;
        })
        .build();

    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;

    let value = match result {
        ExecutionResult::Success {
            output: Output::Call(value),
            ..
        } => value,
        result => {
            return Err(anyhow!("execution failed: {result:?}"));
        }
    };

    Ok(value)
}

pub fn revm_revert(
    from: Address,
    to: Address,
    calldata: Bytes,
    cache_db: &mut AlloyCacheDB,
) -> Result<Bytes> {
    let mut evm = Evm::builder()
        .with_db(cache_db)
        .modify_tx_env(|tx| {
            tx.caller = from;
            tx.transact_to = TransactTo::Call(to);
            tx.data = calldata;
            tx.value = U256::ZERO;
        })
        .build();
    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;

    let value = match result {
        ExecutionResult::Revert { output: value, .. } => value,
        _ => {
            panic!("It should never happen!");
        }
    };

    Ok(value)
}


pub async fn init_account<P>(
    address: Address,
    cache_db: &mut AlloyCacheDB,
    provider: Arc<P>,
) -> Result<()> 
where
    P: Provider<Ethereum> + 'static
{
    let db = CacheDB::new(AlloyDB::new(ProviderBuilder::new().on_http(Url::parse("https://eth.merkle.io").unwrap()), Default::default()));

    let cache_key = format!("bytecode-{:?}", address);
    let bytecode = match cacache::read(&cache_dir(), cache_key.clone()).await {
        Ok(bytecode) => {
            let bytecode = Bytes::from(bytecode);
            Bytecode::new_raw(bytecode)
        }
        Err(_e) => {
            let bytecode = provider.get_code_at(address).await?;
            let bytecode_result = Bytecode::new_raw(bytecode.clone());
            let bytecode = bytecode.to_vec();
            cacache::write(&cache_dir(), cache_key, bytecode.clone()).await?;
            bytecode_result
        }
    };
    let code_hash = bytecode.hash_slow();
    let acc_info = AccountInfo {
        balance: U256::ZERO,
        nonce: 0_u64,
        code: Some(bytecode),
        code_hash,
    };
    cache_db.insert_account_info(address, acc_info);
    Ok(())
}

pub fn init_account_with_bytecode(
    address: Address,
    bytecode: Bytecode,
    cache_db: &mut AlloyCacheDB,
) -> Result<()> {
    let code_hash = bytecode.hash_slow();
    let acc_info = AccountInfo {
        balance: U256::ZERO,
        nonce: 0_u64,
        code: Some(bytecode),
        code_hash,
    };

    cache_db.insert_account_info(address, acc_info);
    Ok(())
}

pub fn insert_mapping_storage_slot(
    contract: Address,
    slot: U256,
    slot_address: Address,
    value: U256,
    cache_db: &mut AlloyCacheDB,
) -> Result<()> {
    let tuple = (slot_address, slot);
    let encoded = tuple.abi_encode();
    let hashed_balance_slot = keccak256(encoded);
    cache_db.insert_account_storage(contract, hashed_balance_slot.into(), value)?;
    Ok(())
}

fn cache_dir() -> String {
    ".evm_cache".to_string()
}

