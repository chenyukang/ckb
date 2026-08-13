use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::{DaoError, extract_dao_data, pack_dao_data};
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, BlockView, Capacity, EpochExt, EpochNumberWithFraction,
        HeaderBuilder, HeaderView, ScriptHashType, TransactionBuilder, TransactionInfo,
        capacity_bytes,
        cell::{CellMetaBuilder, ResolvedTransaction},
    },
    h256,
    packed::{CellOutput, Script, WitnessArgs},
    prelude::*,
    utilities::DIFF_TWO,
};
use tempfile::TempDir;

use crate::{DaoCalculator, secondary_issuance_breakdown};

#[test]
fn secondary_issuance_breakdown_preserves_baseline_and_assigns_rounding_to_dao() {
    let breakdown = secondary_issuance_breakdown(
        Capacity::shannons(101),
        Capacity::shannons(1_000),
        Capacity::shannons(600),
        Capacity::shannons(350),
    )
    .unwrap();

    assert_eq!(breakdown.miner, Capacity::shannons(60));
    assert_eq!(breakdown.treasury, Capacity::shannons(5));
    assert_eq!(breakdown.nervos_dao, Capacity::shannons(36));
    assert_eq!(
        breakdown
            .miner
            .safe_add(breakdown.nervos_dao)
            .and_then(|capacity| capacity.safe_add(breakdown.treasury))
            .unwrap(),
        breakdown.total
    );
}

#[test]
fn secondary_issuance_breakdown_rejects_inconsistent_state() {
    let result = secondary_issuance_breakdown(
        Capacity::shannons(100),
        Capacity::shannons(1_000),
        Capacity::shannons(700),
        Capacity::shannons(301),
    );

    assert_eq!(result.unwrap_err(), DaoError::InvalidTreasuryState);
}

#[test]
fn secondary_issuance_breakdown_tracks_epoch_block_rounding() {
    let epoch = EpochExt::new_builder()
        .number(1)
        .start_number(100)
        .length(3)
        .build();
    let epoch_reward = Capacity::shannons(1_000);
    let totals = (100..103)
        .map(|number| {
            let secondary = epoch
                .secondary_block_issuance(number, epoch_reward)
                .unwrap();
            secondary_issuance_breakdown(
                secondary,
                Capacity::shannons(1_000),
                Capacity::shannons(600),
                Capacity::shannons(350),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        totals.iter().map(|value| value.total.as_u64()).sum::<u64>(),
        epoch_reward.as_u64()
    );
    assert_eq!(totals[0].total, Capacity::shannons(334));
    assert_eq!(totals[2].total, Capacity::shannons(333));
    assert!(totals.iter().all(|value| {
        value
            .miner
            .safe_add(value.nervos_dao)
            .and_then(|capacity| capacity.safe_add(value.treasury))
            .unwrap()
            == value.total
    }));
}

#[test]
fn dao_deposit_capacity_change_only_tracks_deposit_phase() {
    let consensus = Consensus::default();
    let dao_type = Script::new_builder()
        .code_hash(consensus.dao_type_hash())
        .hash_type(ScriptHashType::Type)
        .build();
    let deposit_output = CellOutput::new_builder()
        .capacity(capacity_bytes!(1_000))
        .type_(Some(dao_type.clone()).pack())
        .build();
    let withdrawing_output = deposit_output.clone();
    let deposit_data = Bytes::from(vec![0u8; 8]);
    let withdrawing_data = Bytes::from(42u64.to_le_bytes().to_vec());
    let spent_deposit =
        CellMetaBuilder::from_cell_output(deposit_output.clone(), deposit_data.clone()).build();
    let spent_withdrawing =
        CellMetaBuilder::from_cell_output(withdrawing_output.clone(), withdrawing_data.clone())
            .build();
    let tx = TransactionBuilder::default()
        .output(withdrawing_output)
        .output_data(withdrawing_data)
        .output(deposit_output.clone())
        .output_data(deposit_data)
        .build();
    let rtx = ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![spent_deposit, spent_withdrawing],
        resolved_dep_groups: vec![],
    };

    let parent = HeaderBuilder::default()
        .number(1_000)
        .epoch(EpochNumberWithFraction::new(1, 0, 1_000))
        .build();
    let (_tmp_dir, store, _) = prepare_store(&parent, Some(0));
    let data_loader = store.borrow_as_data_loader();
    let change = DaoCalculator::new(&consensus, &data_loader)
        .dao_deposit_capacity_change([rtx].iter())
        .unwrap();
    let occupied = deposit_output
        .occupied_capacity(Capacity::bytes(8).unwrap())
        .unwrap();
    let expected = capacity_bytes!(1_000).safe_sub(occupied).unwrap();
    assert_eq!(change.added, expected);
    assert_eq!(change.removed, expected);
}

fn prepare_store(
    parent: &HeaderView,
    epoch_start: Option<BlockNumber>,
) -> (TempDir, ChainDB, HeaderView) {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();

    let parent_block = BlockBuilder::default().header(parent.clone()).build();

    txn.insert_block(&parent_block).unwrap();
    txn.attach_block(&parent_block).unwrap();

    let epoch_ext = EpochExt::new_builder()
        .number(parent.number())
        .base_block_reward(Capacity::shannons(50_000_000_000))
        .remainder_reward(Capacity::shannons(1_000_128))
        .previous_epoch_hash_rate(U256::one())
        .last_block_hash_in_previous_epoch(h256!("0x1").into())
        .start_number(epoch_start.unwrap_or_else(|| parent.number() - 1000))
        .length(2091)
        .compact_target(DIFF_TWO)
        .build();
    let epoch_hash = h256!("0x123455").into();

    txn.insert_block_epoch_index(&parent.hash(), &epoch_hash)
        .unwrap();
    txn.insert_epoch_ext(&epoch_hash, &epoch_ext).unwrap();

    txn.commit().unwrap();

    (tmp_dir, store, parent.clone())
}

#[test]
fn check_dao_data_calculation() {
    let consensus = Consensus::default();

    let parent_number = 12345;
    let epoch = EpochNumberWithFraction::new(12, 345, 1000);
    let parent_header = HeaderBuilder::default()
        .number(parent_number)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(500_000_000_123_000),
            Capacity::shannons(400_000_000_123),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, None);
    let result = DaoCalculator::new(&consensus, &store.borrow_as_data_loader())
        .dao_field([].iter(), &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_682_998,
            Capacity::shannons(500_079_349_650_985),
            Capacity::shannons(429_314_308_674),
            Capacity::shannons(600_000_000_000)
        )
    );
}

#[test]
fn check_initial_dao_data_calculation() {
    let consensus = Consensus::default();

    let parent_number = 0;
    let parent_header = HeaderBuilder::default()
        .number(parent_number)
        .dao(pack_dao_data(
            10_000_000_000_000_000,
            Capacity::shannons(500_000_000_000_000),
            Capacity::shannons(400_000_000_000),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, Some(0));
    let result = DaoCalculator::new(&consensus, &store.borrow_as_data_loader())
        .dao_field([].iter(), &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_559_680,
            Capacity::shannons(500_079_349_527_985),
            Capacity::shannons(429_314_308_551),
            Capacity::shannons(600_000_000_000)
        )
    );
}

#[test]
fn check_first_epoch_block_dao_data_calculation() {
    let consensus = Consensus::default();

    let parent_number = 12340;
    let epoch = EpochNumberWithFraction::new(12, 340, 1000);
    let parent_header = HeaderBuilder::default()
        .number(parent_number)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(500_000_000_123_000),
            Capacity::shannons(400_000_000_123),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, Some(12340));
    let result = DaoCalculator::new(&consensus, &store.borrow_as_data_loader())
        .dao_field([].iter(), &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_682_998,
            Capacity::shannons(500_079_349_650_985),
            Capacity::shannons(429_314_308_674),
            Capacity::shannons(600_000_000_000)
        )
    );
}

#[test]
fn check_dao_data_calculation_overflows() {
    let consensus = Consensus::default();

    let parent_number = 12345;
    let epoch = EpochNumberWithFraction::new(12, 345, 1000);
    let parent_header = HeaderBuilder::default()
        .number(parent_number)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(18_446_744_073_709_000_000),
            Capacity::shannons(446_744_073_709),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, None);
    let result = DaoCalculator::new(&consensus, &store.borrow_as_data_loader())
        .dao_field([].iter(), &parent_header);
    assert!(result.unwrap_err().to_string().contains("Overflow"));
}

#[test]
fn check_dao_data_calculation_with_transactions() {
    let consensus = Consensus::default();

    let parent_number = 12345;
    let epoch = EpochNumberWithFraction::new(12, 345, 1000);
    let parent_header = HeaderBuilder::default()
        .number(parent_number)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Capacity::shannons(500_000_000_123_000),
            Capacity::shannons(400_000_000_123),
            Capacity::shannons(600_000_000_000),
        ))
        .build();

    let (_tmp_dir, store, parent_header) = prepare_store(&parent_header, None);
    let input_cell_data = Bytes::from("abcde");
    let input_cell = CellOutput::new_builder()
        .capacity(capacity_bytes!(10000))
        .build();
    let output_cell_data = Bytes::from("abcde12345");
    let output_cell = CellOutput::new_builder()
        .capacity(capacity_bytes!(20000))
        .build();

    let tx = TransactionBuilder::default()
        .output(output_cell)
        .output_data(output_cell_data)
        .build();
    let rtx = ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(input_cell, input_cell_data).build(),
        ],
        resolved_dep_groups: vec![],
    };

    let result = DaoCalculator::new(&consensus, &store.borrow_as_data_loader())
        .dao_field([rtx].iter(), &parent_header)
        .unwrap();
    let dao_data = extract_dao_data(result);
    assert_eq!(
        dao_data,
        (
            10_000_586_990_682_998,
            Capacity::shannons(500_079_349_650_985),
            Capacity::shannons(429_314_308_674),
            Capacity::shannons(600_500_000_000)
        )
    );
}

#[test]
fn check_withdraw_calculation() {
    let data = Bytes::from(vec![1; 10]);
    let output = CellOutput::new_builder()
        .capacity(capacity_bytes!(1000000))
        .build();
    let tx = TransactionBuilder::default()
        .output(output.clone())
        .output_data(&data)
        .build();
    let epoch = EpochNumberWithFraction::new(1, 100, 1000);
    let deposit_header = HeaderBuilder::default()
        .number(100)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let deposit_block = BlockBuilder::default()
        .header(deposit_header)
        .transaction(tx)
        .build();

    let epoch = EpochNumberWithFraction::new(1, 200, 1000);
    let withdrawing_header = HeaderBuilder::default()
        .number(200)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_001_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let withdrawing_block = BlockBuilder::default().header(withdrawing_header).build();

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();
    txn.insert_block(&deposit_block).unwrap();
    txn.attach_block(&deposit_block).unwrap();
    txn.insert_block(&withdrawing_block).unwrap();
    txn.attach_block(&withdrawing_block).unwrap();
    txn.commit().unwrap();

    let consensus = Consensus::default();
    let data_loader = store.borrow_as_data_loader();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.calculate_maximum_withdraw(
        &output,
        Capacity::bytes(data.len()).expect("should not overflow"),
        &deposit_block.hash(),
        &withdrawing_block.hash(),
    );
    assert_eq!(result.unwrap(), Capacity::shannons(100_000_000_009_999));
}

#[test]
fn check_withdraw_calculation_overflows() {
    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(18_446_744_073_709_550_000))
        .build();
    let tx = TransactionBuilder::default().output(output.clone()).build();
    let epoch = EpochNumberWithFraction::new(1, 100, 1000);
    let deposit_header = HeaderBuilder::default()
        .number(100)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let deposit_block = BlockBuilder::default()
        .header(deposit_header)
        .transaction(tx)
        .build();

    let epoch = EpochNumberWithFraction::new(1, 200, 1000);
    let withdrawing_header = HeaderBuilder::default()
        .number(200)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_001_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let withdrawing_block = BlockBuilder::default().header(withdrawing_header).build();

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();
    txn.insert_block(&deposit_block).unwrap();
    txn.attach_block(&deposit_block).unwrap();
    txn.insert_block(&withdrawing_block).unwrap();
    txn.attach_block(&withdrawing_block).unwrap();
    txn.commit().unwrap();

    let consensus = Consensus::default();
    let data_loader = store.borrow_as_data_loader();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.calculate_maximum_withdraw(
        &output,
        Capacity::bytes(0).expect("should not overflow"),
        &deposit_block.hash(),
        &withdrawing_block.hash(),
    );
    assert!(result.is_err());
}

#[test]
fn check_withdraw_calculation_counted_capacity_overflows_before_adding_occupied_capacity() {
    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(u64::MAX))
        .build();
    let tx = TransactionBuilder::default().output(output.clone()).build();
    let epoch = EpochNumberWithFraction::new(1, 100, 1000);
    let deposit_header = HeaderBuilder::default()
        .number(100)
        .epoch(epoch)
        .dao(pack_dao_data(
            1,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let deposit_block = BlockBuilder::default()
        .header(deposit_header)
        .transaction(tx)
        .build();

    let epoch = EpochNumberWithFraction::new(1, 200, 1000);
    let withdrawing_header = HeaderBuilder::default()
        .number(200)
        .epoch(epoch)
        .dao(pack_dao_data(
            u64::MAX,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let withdrawing_block = BlockBuilder::default().header(withdrawing_header).build();

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();
    txn.insert_block(&deposit_block).unwrap();
    txn.attach_block(&deposit_block).unwrap();
    txn.insert_block(&withdrawing_block).unwrap();
    txn.attach_block(&withdrawing_block).unwrap();
    txn.commit().unwrap();

    let consensus = Consensus::default();
    let data_loader = store.borrow_as_data_loader();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.calculate_maximum_withdraw(
        &output,
        Capacity::bytes(0).expect("should not overflow"),
        &deposit_block.hash(),
        &withdrawing_block.hash(),
    );
    assert_eq!(result.unwrap_err(), DaoError::Overflow);
}

fn build_dao_withdraw_tx(
    deposit_block: &BlockView,
    withdraw_block: &BlockView,
    deposited_block_number: u64,
) -> ResolvedTransaction {
    let consensus = Consensus::default();

    let dao_type_script = Script::new_builder()
        .code_hash(consensus.dao_type_hash())
        .hash_type(ScriptHashType::Type)
        .build();

    let cell_data = Bytes::from(deposited_block_number.to_le_bytes().to_vec());
    let input_cell = CellOutput::new_builder()
        .capacity(capacity_bytes!(1000000))
        .type_(Some(dao_type_script).pack())
        .build();

    let tx_info = TransactionInfo::new(
        withdraw_block.number(),
        withdraw_block.epoch(),
        withdraw_block.hash(),
        0,
    );
    let cell_meta = CellMetaBuilder::from_cell_output(input_cell, cell_data)
        .transaction_info(tx_info)
        .build();

    let witness = WitnessArgs::new_builder()
        .input_type(Some(Bytes::from(0u64.to_le_bytes().to_vec())))
        .build();
    let witness_bytes: Bytes = witness.as_bytes();

    let tx = TransactionBuilder::default()
        .header_dep(deposit_block.hash())
        .header_dep(withdraw_block.hash())
        .witness(witness_bytes)
        .build();

    ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![cell_meta],
        resolved_dep_groups: vec![],
    }
}

fn setup_store_with_headers(
    deposit_number: u64,
    withdraw_number: u64,
) -> (TempDir, ChainDB, BlockView, BlockView) {
    let epoch = EpochNumberWithFraction::new(1, deposit_number % 1000, 1000);

    let deposit_header = HeaderBuilder::default()
        .number(deposit_number)
        .epoch(epoch)
        .dao(pack_dao_data(
            10_000_000_000_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let deposit_block = BlockBuilder::default().header(deposit_header).build();

    let withdraw_epoch = EpochNumberWithFraction::new(1, withdraw_number % 1000, 1000);
    let withdraw_header = HeaderBuilder::default()
        .number(withdraw_number)
        .epoch(withdraw_epoch)
        .dao(pack_dao_data(
            10_000_000_001_123_456,
            Default::default(),
            Default::default(),
            Default::default(),
        ))
        .build();
    let withdraw_block = BlockBuilder::default().header(withdraw_header).build();

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let txn = store.begin_transaction();
    txn.insert_block(&deposit_block).unwrap();
    txn.attach_block(&deposit_block).unwrap();
    txn.insert_block(&withdraw_block).unwrap();
    txn.attach_block(&withdraw_block).unwrap();
    txn.commit().unwrap();

    (tmp_dir, store, deposit_block, withdraw_block)
}

#[test]
fn check_dao_withdraw_block_number_mismatch() {
    let (_tmp_dir, store, deposit_block, withdraw_block) = setup_store_with_headers(100, 200);

    // Cell data says block 99, but the resolved deposit header is block 100
    let rtx = build_dao_withdraw_tx(&deposit_block, &withdraw_block, 99);

    let consensus = Consensus::default();
    let data_loader = store.borrow_as_data_loader();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.transaction_fee(&rtx);

    assert!(result.is_err());
}

#[test]
fn check_dao_withdraw_block_number_match() {
    let deposit_number = 100u64;
    let (_tmp_dir, store, deposit_block, withdraw_block) =
        setup_store_with_headers(deposit_number, 200);

    // Cell data matches deposit header block number
    let rtx = build_dao_withdraw_tx(&deposit_block, &withdraw_block, deposit_number);

    let consensus = Consensus::default();
    let data_loader = store.borrow_as_data_loader();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.transaction_fee(&rtx);

    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

#[test]
fn check_dao_withdraw_header_dep_index_exceeds_u8() {
    let deposit_number = 100u64;
    let withdraw_number = 200u64;

    let (_tmp_dir, store, deposit_block, withdraw_block) =
        setup_store_with_headers(deposit_number, withdraw_number);

    let consensus = Consensus::default();
    let dao_type_script = Script::new_builder()
        .code_hash(consensus.dao_type_hash())
        .hash_type(ScriptHashType::Type)
        .build();

    // Pad header_deps to 258 entries so index 257 is valid.
    // Position 1: correct deposit block (what C VM resolves via lowest byte).
    // Position 257: withdraw block (wrong — Rust resolves this with full u64).
    let dummy = h256!("0x1").into();
    let mut header_deps = vec![dummy; 258];
    header_deps[1] = deposit_block.hash();
    header_deps[257] = withdraw_block.hash();

    let cell_data = Bytes::from(deposit_number.to_le_bytes().to_vec());
    let input_cell = CellOutput::new_builder()
        .capacity(capacity_bytes!(1000000))
        .type_(Some(dao_type_script).pack())
        .build();
    let tx_info = TransactionInfo::new(
        withdraw_block.number(),
        withdraw_block.epoch(),
        withdraw_block.hash(),
        0,
    );
    let cell_meta = CellMetaBuilder::from_cell_output(input_cell, cell_data)
        .transaction_info(tx_info)
        .build();

    // input_type = 257, lowest byte = 1
    let witness = WitnessArgs::new_builder()
        .input_type(Some(Bytes::from(257u64.to_le_bytes().to_vec())))
        .build();
    let witness_bytes: Bytes = witness.as_bytes();

    let tx = TransactionBuilder::default()
        .set_header_deps(header_deps)
        .witness(witness_bytes)
        .build();

    let rtx = ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![cell_meta],
        resolved_dep_groups: vec![],
    };

    let data_loader = store.borrow_as_data_loader();
    let calculator = DaoCalculator::new(&consensus, &data_loader);
    let result = calculator.transaction_fee(&rtx);

    // Rust resolves index 257 → withdraw block (number 200), but cell data
    // says deposited at block 100. Block number check catches the mismatch.
    assert!(result.is_err(), "expected Err, got {result:?}");
}
