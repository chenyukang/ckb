//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
use crate::component::container::AncestorsScoreSortKey;
use crate::component::edges::Edges;
use crate::component::entry::EvictKey;
use crate::component::links::{Relation, TxLinks, TxLinksMap};
use crate::error::Reject;
use crate::TxEntry;
use ckb_logger::{debug, error, trace, warn};
use ckb_types::core::error::OutPointError;
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::cell::{CellMetaBuilder, CellProvider, CellStatus},
    prelude::*,
};
use ckb_types::{
    core::{cell::CellChecker, TransactionView},
    packed::{Byte32, ProposalShortId},
};
use multi_index_map::MultiIndexMap;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{HashSet, VecDeque};

type ConflictEntry = (TxEntry, Reject);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Pending,
    Gap,
    Proposed,
}

#[derive(MultiIndexMap, Clone)]
pub struct PoolEntry {
    #[multi_index(hashed_unique)]
    pub id: ProposalShortId,
    #[multi_index(ordered_non_unique)]
    pub score: AncestorsScoreSortKey,
    #[multi_index(ordered_non_unique)]
    pub status: Status,
    #[multi_index(ordered_non_unique)]
    pub evict_key: EvictKey,

    pub inner: TxEntry,
    // other sort key
}

impl MultiIndexPoolEntryMap {
    /// sorted by ancestor score from higher to lower
    pub fn score_sorted_iter(&self) -> impl Iterator<Item = &TxEntry> {
        // Note: multi_index don't support reverse order iteration now
        // so we need to collect and reverse
        let entries = self.iter_by_score().collect::<Vec<_>>();
        entries.into_iter().rev().map(move |entry| &entry.inner)
    }
}

pub struct PoolMap {
    /// The pool entries with different kinds of sort strategies
    pub(crate) entries: MultiIndexPoolEntryMap,
    /// All the deps, header_deps, inputs, outputs relationships
    pub(crate) edges: Edges,
    /// All the parent/children relationships
    pub(crate) links: TxLinksMap,
    pub(crate) max_ancestors_count: usize,
}

impl PoolMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        PoolMap {
            entries: MultiIndexPoolEntryMap::default(),
            edges: Edges::default(),
            links: TxLinksMap::new(),
            max_ancestors_count,
        }
    }

    #[cfg(test)]
    pub(crate) fn outputs_len(&self) -> usize {
        self.edges.outputs_len()
    }

    #[cfg(test)]
    pub(crate) fn header_deps_len(&self) -> usize {
        self.edges.header_deps_len()
    }

    #[cfg(test)]
    pub(crate) fn deps_len(&self) -> usize {
        self.edges.deps_len()
    }

    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.edges.inputs_len()
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.get_by_id(id).is_some()
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.entries
            .get_by_id(id)
            .map(|entry| entry.inner.transaction())
    }

    fn record_entry_edges(&mut self, entry: &TxEntry) {
        let tx_short_id = entry.proposal_short_id();
        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();

        for i in inputs {
            self.edges
                .inputs
                .entry(i.to_owned())
                .or_default()
                .insert(tx_short_id.clone());

            if let Some(outputs) = self.edges.outputs.get_mut(&i) {
                outputs.insert(tx_short_id.clone());
            }
        }

        // record dep-txid
        for d in entry.related_dep_out_points() {
            self.edges
                .deps
                .entry(d.to_owned())
                .or_default()
                .insert(tx_short_id.clone());

            if let Some(outputs) = self.edges.outputs.get_mut(d) {
                outputs.insert(tx_short_id.clone());
            }
        }

        // record tx unconsumed output
        for o in outputs {
            self.edges.outputs.insert(o, HashSet::new());
        }

        // record header_deps
        let header_deps = entry.transaction().header_deps();
        if !header_deps.is_empty() {
            self.edges
                .header_deps
                .insert(tx_short_id.clone(), header_deps.into_iter().collect());
        }
    }

    /// Record the links for entry
    fn record_entry_relations(&mut self, entry: &mut TxEntry) -> Result<bool, Reject> {
        // find in pool parents
        let mut parents: HashSet<ProposalShortId> = HashSet::with_capacity(
            entry.transaction().inputs().len() + entry.transaction().cell_deps().len(),
        );
        let short_id = entry.proposal_short_id();

        for input in entry.transaction().inputs() {
            let input_pt = input.previous_output();
            if let Some(deps) = self.edges.deps.get(&input_pt) {
                parents.extend(deps.iter().cloned());
            }

            let parent_hash = &input_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(parent_hash);
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            let id = ProposalShortId::from_tx_hash(&dep_pt.tx_hash());
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }

        let ancestors = self
            .links
            .calc_relation_ids(Cow::Borrowed(&parents), Relation::Parents);

        // update parents references
        for ancestor_id in &ancestors {
            let ancestor = self
                .entries
                .get_by_id(ancestor_id)
                .expect("pool consistent");
            entry.add_entry_weight(&ancestor.inner);
        }

        if entry.ancestors_count > self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            // insert dep-ref map
            self.edges
                .deps
                .entry(dep_pt)
                .or_insert_with(HashSet::new)
                .insert(short_id.clone());
        }

        Ok(true)
    }

    pub fn add_entry(&mut self, mut entry: TxEntry, status: Status) -> bool {
        let tx_short_id = entry.proposal_short_id();
        if self.entries.get_by_id(&tx_short_id).is_some() {
            return false;
        }
        trace!("add_{:?} {}", status, entry.transaction().hash());
        self.record_entry_edges(&entry);
        if status == Status::Proposed && self.record_entry_relations(&mut entry).is_err() {
            return false;
        }
        let score = entry.as_score_key();
        let evict_key = entry.as_evict_key();
        self.entries.insert(PoolEntry {
            id: tx_short_id,
            score,
            status,
            inner: entry,
            evict_key,
        });
        true
    }

    pub fn get_by_id(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.entries.get_by_id(id).map(|entry| entry)
    }

    fn get_descendants(&self, entry: &TxEntry) -> HashSet<ProposalShortId> {
        let mut entries: VecDeque<&TxEntry> = VecDeque::new();
        entries.push_back(entry);

        let mut descendants = HashSet::new();
        while let Some(entry) = entries.pop_front() {
            let outputs = entry.transaction().output_pts();

            for output in outputs {
                if let Some(ids) = self.edges.outputs.get(&output) {
                    for id in ids {
                        if descendants.insert(id.clone()) {
                            if let Some(entry) = self.entries.get_by_id(id) {
                                entries.push_back(&entry.inner);
                            }
                        }
                    }
                }
            }
        }
        descendants
    }

    pub(crate) fn remove_entry_relation(&mut self, entry: &TxEntry) {
        let inputs = entry.transaction().input_pts_iter();
        let tx_short_id = entry.proposal_short_id();
        let outputs = entry.transaction().output_pts();

        // remove inputs
        for i in inputs {
            if let Entry::Occupied(mut occupied) = self.edges.inputs.entry(i) {
                let empty = {
                    let ids = occupied.get_mut();
                    ids.remove(&tx_short_id);
                    ids.is_empty()
                };
                if empty {
                    occupied.remove();
                }
            }
        }

        // remove dep
        for d in entry.related_dep_out_points().cloned() {
            if let Entry::Occupied(mut occupied) = self.edges.deps.entry(d) {
                let empty = {
                    let ids = occupied.get_mut();
                    ids.remove(&tx_short_id);
                    ids.is_empty()
                };
                if empty {
                    occupied.remove();
                }
            }
        }

        for o in outputs {
            self.edges.outputs.remove(&o);
        }

        self.edges.header_deps.remove(&tx_short_id);
    }

    pub fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        let removed = self.entries.remove_by_id(id);

        if let Some(ref entry) = removed {
            self.remove_entry_relation(&entry.inner);
        }
        removed.map(|e| e.inner)
    }

    pub fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removed = Vec::new();
        if let Some(entry) = self.entries.remove_by_id(id) {
            let descendants = self.get_descendants(&entry.inner);
            self.remove_entry_relation(&entry.inner);
            removed.push(entry.inner);
            for id in descendants {
                if let Some(entry) = self.remove_entry(&id) {
                    removed.push(entry);
                }
            }
        }
        removed
    }

    pub fn resolve_conflict_header_dep(&mut self, headers: &HashSet<Byte32>) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();

        // invalid header deps
        let mut ids = Vec::new();
        for (tx_id, deps) in self.edges.header_deps.iter() {
            for hash in deps {
                if headers.contains(hash) {
                    ids.push((hash.clone(), tx_id.clone()));
                    break;
                }
            }
        }

        for (blk_hash, id) in ids {
            let entries = self.remove_entry_and_descendants(&id);
            for entry in entries {
                let reject = Reject::Resolve(OutPointError::InvalidHeader(blk_hash.to_owned()));
                conflicts.push((entry, reject));
            }
        }
        conflicts
    }

    pub fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let inputs = tx.input_pts_iter();
        let mut conflicts = Vec::new();

        for i in inputs {
            if let Some(ids) = self.edges.inputs.remove(&i) {
                for id in ids {
                    let entries = self.remove_entry_and_descendants(&id);
                    for entry in entries {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        conflicts.push((entry, reject));
                    }
                }
            }

            // deps consumed
            if let Some(ids) = self.edges.deps.remove(&i) {
                for id in ids {
                    let entries = self.remove_entry_and_descendants(&id);
                    for entry in entries {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        conflicts.push((entry, reject));
                    }
                }
            }
        }
        conflicts
    }

    // fill proposal txs
    pub fn fill_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
        proposals: &mut HashSet<ProposalShortId>,
        status: &Status,
    ) {
        for entry in self.entries.get_by_status(status) {
            if proposals.len() == limit {
                break;
            }
            if !exclusion.contains(&entry.id) {
                proposals.insert(entry.id.clone());
            }
        }
    }

    pub fn remove_entries_by_filter<P: FnMut(&ProposalShortId, &TxEntry) -> bool>(
        &mut self,
        mut predicate: P,
    ) -> Vec<TxEntry> {
        let mut removed = Vec::new();
        for (_, entry) in self.entries.iter() {
            if predicate(&entry.id, &entry.inner) {
                removed.push(entry.inner.clone());
            }
        }
        for entry in &removed {
            self.remove_entry(&entry.proposal_short_id());
        }

        removed
    }

    pub fn iter(&self) -> impl Iterator<Item = &PoolEntry> {
        self.entries.iter().map(|(_, entry)| entry)
    }

    pub fn iter_by_evict_key(&self) -> impl Iterator<Item = &PoolEntry> {
        self.entries.iter_by_evict_key()
    }

    pub fn next_evict_entry(&self) -> Option<ProposalShortId> {
        self.iter_by_evict_key()
            .into_iter()
            .next()
            .map(|entry| entry.id.clone())
    }

    pub fn clear(&mut self) {
        self.entries = MultiIndexPoolEntryMap::default();
        self.edges.clear();
    }

    pub(crate) fn drain(&mut self) -> Vec<TransactionView> {
        let txs = self
            .entries
            .iter()
            .map(|(_k, entry)| entry.inner.clone().into_transaction())
            .collect::<Vec<_>>();
        self.entries.clear();
        self.edges.clear();
        txs
    }
}

impl CellProvider for MultiIndexPoolEntryMap {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        let tx_hash = out_point.tx_hash();
        if let Some(entry) = self.get_by_id(&ProposalShortId::from_tx_hash(&tx_hash)) {
            match entry
                .inner
                .transaction()
                .output_with_data(out_point.index().unpack())
            {
                Some((output, data)) => {
                    let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                        .out_point(out_point.to_owned())
                        .build();
                    CellStatus::live_cell(cell_meta)
                }
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unknown
        }
    }
}

impl CellChecker for MultiIndexPoolEntryMap {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        let tx_hash = out_point.tx_hash();
        if let Some(entry) = self.get_by_id(&ProposalShortId::from_tx_hash(&tx_hash)) {
            entry
                .inner
                .transaction()
                .output(out_point.index().unpack())
                .map(|_| true)
        } else {
            None
        }
    }
}
