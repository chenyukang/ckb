use ckb_logger::debug;
use ckb_types::packed::ProposalShortId;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

#[derive(Default, Debug, Clone)]
pub struct TxLinks {
    pub parents: HashSet<ProposalShortId>,
    pub children: HashSet<ProposalShortId>,
}

#[derive(Clone, Copy)]
pub enum Relation {
    Parents,
    Children,
}

impl TxLinks {
    fn get_direct_ids(&self, relation: Relation) -> &HashSet<ProposalShortId> {
        match relation {
            Relation::Parents => &self.parents,
            Relation::Children => &self.children,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TxLinksMap {
    pub inner: HashMap<ProposalShortId, TxLinks>,
}

impl TxLinksMap {
    pub fn new() -> Self {
        TxLinksMap {
            inner: Default::default(),
        }
    }

    fn calc_relative_ids(
        &self,
        short_id: &ProposalShortId,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let direct = self
            .inner
            .get(short_id)
            .map(|link| link.get_direct_ids(relation))
            .cloned()
            .unwrap_or_default();

        self.calc_relation_ids(Cow::Owned(direct), relation)
    }

    pub fn calc_relation_ids(
        &self,
        stage: Cow<HashSet<ProposalShortId>>,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let mut stage = stage.into_owned();
        let mut relation_ids = HashSet::with_capacity(stage.len());

        while let Some(id) = stage.iter().next().cloned() {
            debug!("calc_relation_ids: id: {:?}", id);
            relation_ids.insert(id.clone());
            stage.remove(&id);

            //recursively
            for id in self
                .inner
                .get(&id)
                .map(|link| link.get_direct_ids(relation))
                .cloned()
                .unwrap_or_default()
            {
                if !relation_ids.contains(&id) {
                    stage.insert(id);
                }
            }
        }
        relation_ids
    }

    pub fn calc_ancestors(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.calc_relative_ids(short_id, Relation::Parents)
    }

    pub fn calc_descendants(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.calc_relative_ids(short_id, Relation::Children)
    }

    pub fn get_children(&self, short_id: &ProposalShortId) -> Option<&HashSet<ProposalShortId>> {
        self.inner.get(short_id).map(|link| &link.children)
    }

    pub fn get_parents(&self, short_id: &ProposalShortId) -> Option<&HashSet<ProposalShortId>> {
        self.inner.get(short_id).map(|link| &link.parents)
    }

    pub fn remove(&mut self, short_id: &ProposalShortId) -> Option<TxLinks> {
        self.inner.remove(short_id)
    }

    pub fn remove_child(
        &mut self,
        short_id: &ProposalShortId,
        child: &ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.children.remove(child))
    }

    pub fn remove_parent(
        &mut self,
        short_id: &ProposalShortId,
        parent: &ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.parents.remove(parent))
    }

    pub fn add_child(
        &mut self,
        short_id: &ProposalShortId,
        child: ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.children.insert(child))
    }

    pub fn add_parent(
        &mut self,
        short_id: &ProposalShortId,
        parent: ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.parents.insert(parent))
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }
}
