use super::helper::new_index_transaction;
use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::{Status, StatusCode};
use ckb_types::packed::{CompactBlock, CompactBlockBuilder};
use ckb_types::prelude::*;

// block_short_ids: vec![None, Some(1), None, Some(3), Some(4), None]
fn build_compact_block() -> CompactBlock {
    let prefilled_iter = vec![0, 2, 5]
        .into_iter()
        .map(new_index_transaction)
        .collect::<Vec<_>>();

    let short_ids = vec![1, 3, 4]
        .into_iter()
        .map(new_index_transaction)
        .map(|tx| tx.transaction().proposal_short_id())
        .collect::<Vec<_>>();

    CompactBlockBuilder::default()
        .short_ids(short_ids)
        .prefilled_transactions(prefilled_iter)
        .build()
}

#[test]
fn test_invalid() {
    let block = build_compact_block();
    let indexes = vec![1, 3, 4];

    // Invalid len
    let block_txs: Vec<_> = vec![1, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();

    assert_eq!(
        BlockTransactionsVerifier::verify(&block, &indexes, block_txs.as_slice()),
        StatusCode::BlockTransactionsLengthIsUnmatchedWithPendingCompactBlock.into(),
    );

    // Unordered txs
    let block_txs: Vec<_> = vec![1, 4, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();
    assert_eq!(
        BlockTransactionsVerifier::verify(&block, &indexes, &block_txs),
        StatusCode::BlockTransactionsShortIdsAreUnmatchedWithPendingCompactBlock.into(),
    );
}

#[test]
fn test_ok() {
    let block = build_compact_block();

    let indexes = vec![1, 3, 4];
    let block_txs: Vec<_> = vec![1, 3, 4]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();

    assert_eq!(
        BlockTransactionsVerifier::verify(&block, &indexes, &block_txs),
        Status::ok()
    );
}

// Regression test for the cross-peer poisoning of `pending_compact_blocks`:
// the map is keyed by block hash and keeps the first compact block, while the
// missing indexes are recorded per peer. A second peer can therefore record
// indexes computed against a longer compact block that shares the same header.
// The verifier must reject the out-of-range indexes instead of panicking.
#[test]
fn test_out_of_range_indexes() {
    // 2 transactions in total: index 0 is prefilled, index 1 is a short id.
    let stored_compact_block = CompactBlockBuilder::default()
        .short_ids(vec![
            new_index_transaction(1).transaction().proposal_short_id(),
        ])
        .prefilled_transactions(vec![new_index_transaction(0)])
        .build();

    // The other peer recorded [1, 2, 3] against its own, longer compact block.
    let block_txs: Vec<_> = vec![1, 2, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();

    assert_eq!(
        BlockTransactionsVerifier::verify(&stored_compact_block, &[1, 2, 3], &block_txs),
        StatusCode::ProtocolMessageIsMalformed.into(),
    );
}
