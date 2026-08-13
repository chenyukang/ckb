use ckb_app_config::StoreConfig;
use ckb_db::RocksDB;
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_error::Error;
use ckb_store::{ChainDB, ChainStore, DaoTreasuryState};
use ckb_types::{
    core::{Capacity, ScriptHashType},
    packed,
    prelude::*,
};
use std::{collections::HashMap, sync::Arc};

const VERSION: &str = "20260813000000";

/// Seeds deterministic DAO treasury accounting from the canonical live cell set.
pub struct AddDaoTreasuryState;

impl Migration for AddDaoTreasuryState {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let genesis = chain_db
            .get_block_hash(0)
            .and_then(|hash| chain_db.get_block(&hash))
            .expect("initialized database has a genesis block");
        let dao_type_hash = genesis
            .transaction(0)
            .and_then(|cellbase| cellbase.output(ckb_chain_spec::OUTPUT_INDEX_DAO as usize))
            .and_then(|output| output.type_().to_opt())
            .map(|script| script.calc_script_hash());

        let progress = pb(1);
        progress.set_style(
            ProgressStyle::default_spinner()
                .template("{prefix:.bold.dim} {spinner} {wide_msg}")
                .expect("valid progress template"),
        );
        let tip = chain_db
            .get_tip_header()
            .expect("initialized database has a tip header");
        progress.set_length(tip.number() + 1);
        progress.set_message("replaying Nervos DAO deposit capacity");

        let mut dao_deposit_capacity = Capacity::zero();
        let counted_capacity = |output: packed::CellOutput,
                                data: ckb_types::bytes::Bytes|
         -> Result<Option<Capacity>, Error> {
            let is_deposit = dao_type_hash.as_ref().is_some_and(|dao_type_hash| {
                output
                    .type_()
                    .to_opt()
                    .map(|script| {
                        Into::<u8>::into(script.hash_type())
                            == Into::<u8>::into(ScriptHashType::Type)
                            && script.code_hash() == *dao_type_hash
                    })
                    .unwrap_or(false)
                    && data.as_ref() == [0u8; 8]
            });
            if !is_deposit {
                return Ok(None);
            }
            let occupied = output.occupied_capacity(Capacity::bytes(data.len())?)?;
            let capacity: Capacity = output.capacity().into();
            Ok(Some(capacity.safe_sub(occupied)?))
        };

        let mut txn = chain_db.begin_transaction();
        let mut live_deposits = HashMap::<packed::OutPoint, Capacity>::new();
        for number in 0..=tip.number() {
            let block = chain_db
                .get_block_hash(number)
                .and_then(|hash| chain_db.get_block(&hash))
                .expect("canonical block exists during migration");

            for transaction in block.transactions() {
                for input in transaction.inputs() {
                    if input.previous_output().is_null() {
                        continue;
                    }
                    if let Some(capacity) = live_deposits.remove(&input.previous_output()) {
                        dao_deposit_capacity = dao_deposit_capacity.safe_sub(capacity)?;
                    }
                }
                for (index, (output, data)) in transaction.outputs_with_data_iter().enumerate() {
                    if let Some(capacity) = counted_capacity(output, data)? {
                        let index = u32::try_from(index)
                            .expect("CKB transaction output count is bounded by serialized size");
                        let out_point = packed::OutPoint::new_builder()
                            .tx_hash(transaction.hash())
                            .index(index)
                            .build();
                        live_deposits.insert(out_point, capacity);
                        dao_deposit_capacity = dao_deposit_capacity.safe_add(capacity)?;
                    }
                }
            }

            txn.insert_dao_treasury_state(
                &block.hash(),
                DaoTreasuryState {
                    dao_deposit_capacity,
                    pending_treasury: Capacity::zero(),
                    treasury_emission: Capacity::zero(),
                },
            )?;
            progress.inc(1);
            if number != 0 && number % 10_000 == 0 {
                txn.commit()?;
                txn = chain_db.begin_transaction();
            }
        }
        txn.commit()?;
        progress.finish_with_message("DAO treasury state initialized");

        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        VERSION
    }
}
