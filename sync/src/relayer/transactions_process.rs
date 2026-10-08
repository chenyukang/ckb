use crate::Status;
use crate::relayer::{MAX_RELAY_TXS_NUM_PER_BATCH, Relayer};
use ckb_logger::error;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core::{Cycle, TransactionView},
    packed,
    prelude::*,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

/// A compliant responder fetches each requested hash at most once (see
/// `GetTransactionsProcess`), so a `RelayTransactions` response must not carry
/// two bodies for the same hash. The caller's filter is evaluated per body and
/// the known marker is applied only after the whole response is collected, so
/// repeated bodies would otherwise each become a separate `submit_remote_tx`
/// tx-pool call. Keep the first body per hash and drop the rest.
pub(crate) fn dedup_transactions(
    txs: Vec<(TransactionView, Cycle)>,
) -> Vec<(TransactionView, Cycle)> {
    let mut seen = HashSet::with_capacity(txs.len());
    txs.into_iter()
        .filter(|(tx, _)| seen.insert(tx.hash()))
        .collect()
}

pub struct TransactionsProcess<'a> {
    message: packed::RelayTransactionsReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a> TransactionsProcess<'a> {
    pub fn new(
        message: packed::RelayTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
    ) -> Self {
        TransactionsProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let shared_state = self.relayer.shared().state();

        // A response is bounded by the relay batch size; reject an oversized
        // body list before doing any per-body work. A compliant responder
        // cannot exceed this because it batches by `MAX_RELAY_TXS_BYTES_PER_BATCH`
        // and every relaid transaction is a real in-pool transaction.
        let response_len = self.message.transactions().len();
        if response_len > MAX_RELAY_TXS_NUM_PER_BATCH {
            return crate::StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                "Transactions count({response_len}) > MAX_RELAY_TXS_NUM_PER_BATCH({MAX_RELAY_TXS_NUM_PER_BATCH})",
            ));
        }

        let txs: Vec<(TransactionView, Cycle)> = {
            // ignore the tx if it's already known or it has never been requested before
            let mut tx_filter = shared_state.tx_filter();
            tx_filter.remove_expired();
            let unknown_tx_hashes = shared_state.unknown_tx_hashes();

            self.message
                .transactions()
                .iter()
                .map(|tx| (tx.transaction().to_entity().into_view(), tx.cycles().into()))
                .filter(|(tx, _)| {
                    !tx_filter.contains(&tx.hash())
                        && unknown_tx_hashes
                            .get_priority(&tx.hash())
                            .map(|priority| priority.requesting_peer() == Some(self.peer))
                            .unwrap_or_default()
                })
                .collect()
        };

        let txs = dedup_transactions(txs);

        if txs.is_empty() {
            return Status::ok();
        }

        let max_block_cycles = self.relayer.shared().consensus().max_block_cycles();
        if txs
            .iter()
            .any(|(_, declared_cycles)| declared_cycles > &max_block_cycles)
        {
            self.nc.ban_peer(
                self.peer,
                DEFAULT_BAN_TIME,
                String::from("relay declared cycles greater than max_block_cycles"),
            );
            return Status::ok();
        }

        shared_state.mark_as_known_txs(txs.iter().map(|(tx, _)| tx.hash()));

        let tx_pool = self.relayer.shared.shared().tx_pool_controller().clone();
        let peer = self.peer;
        self.relayer
            .shared
            .shared()
            .async_handle()
            .spawn(async move {
                for (tx, declared_cycles) in txs {
                    if let Err(e) = tx_pool
                        .submit_remote_tx(tx.clone(), declared_cycles, peer)
                        .await
                    {
                        error!("submit_tx error {}", e);
                    }
                }
            });

        Status::ok()
    }
}
