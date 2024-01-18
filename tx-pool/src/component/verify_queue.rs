//! Top-level VerifyQueue structure.
#![allow(missing_docs)]
extern crate rustc_hash;
extern crate slab;
use ckb_network::PeerIndex;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{tx_pool::Reject, Cycle, TransactionView},
    packed::ProposalShortId,
};
use ckb_util::shrink_to_fit;
use multi_index_map::MultiIndexMap;
use tokio::sync::watch;

const DEFAULT_MAX_VERIFY_TRANSACTIONS: usize = 100;
const SHRINK_THRESHOLD: usize = 100;

/// The verify queue Entry to verify.
#[derive(Debug, Clone, Eq)]
pub struct Entry {
    pub(crate) tx: TransactionView,
    pub(crate) remote: Option<(Cycle, PeerIndex)>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Entry) -> bool {
        self.tx == other.tx
    }
}

#[derive(MultiIndexMap, Clone)]
pub struct VerifyEntry {
    /// The transaction id
    #[multi_index(hashed_unique)]
    pub id: ProposalShortId,
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    /// This field is used to sort the txs in the queue
    /// We may add more other sort keys in the future
    #[multi_index(ordered_non_unique)]
    pub added_time: u64,
    /// other sort key
    pub inner: Entry,
}

/// The verify queue is a priority queue of transactions to verify.
pub struct VerifyQueue {
    /// inner tx entry
    inner: MultiIndexVerifyEntryMap,
    /// when queue is changed, notify the tx-pool to update the txs count
    queue_tx: watch::Sender<usize>,
    /// subscribe this channel to get the txs count in the queue
    queue_rx: watch::Receiver<usize>,
}

impl VerifyQueue {
    /// Create a new VerifyQueue
    pub(crate) fn new() -> Self {
        let (queue_tx, queue_rx) = watch::channel(0_usize);
        VerifyQueue {
            inner: MultiIndexVerifyEntryMap::default(),
            queue_tx,
            queue_rx,
        }
    }

    /// Returns the number of txs in the queue.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the queue contains no txs.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns true if the queue is full.
    pub fn is_full(&self) -> bool {
        self.len() >= DEFAULT_MAX_VERIFY_TRANSACTIONS
    }

    /// Returns true if the queue contains a tx with the specified id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.get_by_id(id).is_some()
    }

    /// Shrink the capacity of the queue as much as possible.
    pub fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }

    /// get a queue_rx to subscribe the txs count in the queue
    pub fn subscribe(&self) -> watch::Receiver<usize> {
        self.queue_rx.clone()
    }

    /// Remove a tx from the queue
    pub fn remove_tx(&mut self, id: &ProposalShortId) -> Option<Entry> {
        self.inner.remove_by_id(id).map(|e| {
            self.shrink_to_fit();
            e.inner
        })
    }

    /// Remove multiple txs from the queue
    pub fn remove_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.inner.remove_by_id(&id);
        }
        self.shrink_to_fit();
    }

    /// Returns the first entry in the queue and remove it
    pub fn pop_first(&mut self) -> Option<Entry> {
        if let Some(short_id) = self.peek() {
            self.remove_tx(&short_id)
        } else {
            None
        }
    }

    /// Returns the first entry in the queue
    pub fn peek(&self) -> Option<ProposalShortId> {
        self.inner
            .iter_by_added_time()
            .next()
            .map(|entry| entry.inner.tx.proposal_short_id())
    }

    /// If the queue did not have this tx present, true is returned.
    /// If the queue did have this tx present, false is returned.
    pub fn add_tx(
        &mut self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        if self.contains_key(&tx.proposal_short_id()) {
            return Ok(false);
        }
        if self.is_full() {
            return Err(Reject::Full(format!(
                "chunk is full, failed to add tx: {:#x}",
                tx.hash()
            )));
        }
        self.inner.insert(VerifyEntry {
            id: tx.proposal_short_id(),
            added_time: unix_time_as_millis(),
            inner: Entry { tx, remote },
        });
        self.queue_tx.send(self.len()).unwrap();
        Ok(true)
    }

    /// Clears the map, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.shrink_to_fit()
    }
}
