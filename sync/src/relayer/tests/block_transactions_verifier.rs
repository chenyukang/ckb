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

// An out-of-range index can only come from local state that no longer matches
// the compact block, so it must not panic and must not ban the responding
// peer.
#[test]
fn test_out_of_range_index_is_not_bannable() {
    let block = build_compact_block();

    let status = BlockTransactionsVerifier::verify(&block, &[6], &[]);
    assert_eq!(status, StatusCode::CompactBlockStateMismatch.into());
    assert!(status.should_ban().is_none());
}
