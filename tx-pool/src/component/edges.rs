use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use ckb_logger::debug;


#[derive(Default, Debug, Clone)]
pub(crate) struct Edges {
    /// input-txid map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, ProposalShortId>,
    /// output-op<txid> map represent in-pool tx's outputs
    pub(crate) outputs: HashMap<OutPoint, Option<ProposalShortId>>,
    /// dep-set<txid> map represent in-pool tx's deps
    pub(crate) deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// dep-set<txid-headers> map represent in-pool tx's header deps
    pub(crate) header_deps: HashMap<ProposalShortId, Vec<Byte32>>,
}

impl Edges {
    pub(crate) fn debug(&self) {
        debug!("Edges:  debug: =================");
        debug!("inputs: {:?}", self.inputs);
        debug!("outputs: {:?}", self.outputs);
        debug!("deps: {:?}", self.deps);
        debug!("header_deps: {:?}", self.header_deps);
        debug!("end .....................");
    }

    #[cfg(test)]
    pub(crate) fn outputs_len(&self) -> usize {
        self.outputs.len()
    }

    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.inputs.len()
    }

    #[cfg(test)]
    pub(crate) fn header_deps_len(&self) -> usize {
        self.header_deps.len()
    }

    #[cfg(test)]
    pub(crate) fn deps_len(&self) -> usize {
        self.deps.len()
    }

    pub(crate) fn insert_input(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.inputs.insert(out_point, txid);
    }

    pub(crate) fn remove_input(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.inputs.remove(out_point)
    }

    pub(crate) fn remove_output(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.outputs.remove(out_point).unwrap_or(None)
    }

    pub(crate) fn insert_output(&mut self, out_point: OutPoint) {
        self.outputs.insert(out_point, None);
    }

    pub(crate) fn insert_consumed_output(&mut self, out_point: OutPoint, id: ProposalShortId) {
        self.outputs.insert(out_point, Some(id));
    }

    pub(crate) fn get_input_ref(&self, out_point: &OutPoint) -> Option<&ProposalShortId> {
        self.inputs.get(out_point)
    }

    pub(crate) fn get_deps_ref(&self, out_point: &OutPoint) -> Option<&HashSet<ProposalShortId>> {
        self.deps.get(out_point)
    }

    pub(crate) fn get_output_ref(&self, out_point: &OutPoint) -> Option<&Option<ProposalShortId>> {
        self.outputs.get(out_point)
    }

    pub(crate) fn get_mut_output(
        &mut self,
        out_point: &OutPoint,
    ) -> Option<&mut Option<ProposalShortId>> {
        self.outputs.get_mut(out_point)
    }

    pub(crate) fn remove_deps(&mut self, out_point: &OutPoint) -> Option<HashSet<ProposalShortId>> {
        self.deps.remove(out_point)
    }

    pub(crate) fn insert_deps(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.deps.entry(out_point).or_default().insert(txid);
    }

    pub(crate) fn delete_txid_by_dep(&mut self, out_point: OutPoint, txid: &ProposalShortId) {
        if let Entry::Occupied(mut occupied) = self.deps.entry(out_point) {
            let empty = {
                let ids = occupied.get_mut();
                ids.remove(txid);
                ids.is_empty()
            };
            if empty {
                occupied.remove();
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.inputs.clear();
        self.outputs.clear();
        self.deps.clear();
        self.header_deps.clear();
    }
}
