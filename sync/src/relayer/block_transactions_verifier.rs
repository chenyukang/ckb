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
            // `pending_compact_blocks` is keyed by block hash and keeps the
            // first compact block, while the missing indexes are recorded per
            // peer. Another peer may have recorded indexes against a longer
            // compact block that shares the same header, so an index can be
            // out of range for the stored compact block here. Reject such a
            // message instead of panicking.
            let Some(short_id) = block_short_ids.get(*index as usize) else {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "transaction index {index} is out of range for the pending compact block ({})",
                    block_short_ids.len(),
                ));
            };
            if let Some(short_id) = short_id {
                missing_short_ids.push(short_id.clone());
            }
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
