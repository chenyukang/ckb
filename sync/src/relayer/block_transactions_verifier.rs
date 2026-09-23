use crate::{Status, StatusCode};
use ckb_types::{core, packed, prelude::*};

pub struct BlockTransactionsVerifier {}

impl BlockTransactionsVerifier {
    pub(crate) fn verify(
        block: &packed::CompactBlock,
        indexes: &[u32],
        transactions: &[core::TransactionView],
    ) -> Status {
        let block_short_ids = block.block_short_ids();
        let mut missing_short_ids: Vec<packed::ProposalShortId> = Vec::with_capacity(indexes.len());
        for index in indexes {
            // The indexes are recorded together with the compact block of the
            // peer that produced them, so they are expected to reference a
            // short id. Keep the guard anyway: never panic on a state that
            // remote messages can influence, and report a mismatch as a
            // non-bannable local state error instead of blaming the
            // responding peer.
            let Some(Some(short_id)) = block_short_ids.get(*index as usize) else {
                return StatusCode::CompactBlockStateMismatch.with_context(format!(
                    "index {index} does not reference a short id, block short ids len({})",
                    block_short_ids.len(),
                ));
            };
            missing_short_ids.push(short_id.clone());
        }

        if missing_short_ids.len() != transactions.len() {
            return StatusCode::BlockTransactionsLengthIsUnmatchedWithPendingCompactBlock
                .with_context(format!(
                    "Expected({}) != actual({})",
                    missing_short_ids.len(),
                    transactions.len(),
                ));
        }

        for (expected_short_id, tx) in missing_short_ids.into_iter().zip(transactions) {
            let short_id = tx.proposal_short_id();
            if expected_short_id != short_id {
                return StatusCode::BlockTransactionsShortIdsAreUnmatchedWithPendingCompactBlock
                    .with_context(format!(
                        "Expected({expected_short_id}) != actual({short_id})",
                    ));
            }
        }

        Status::ok()
    }
}
