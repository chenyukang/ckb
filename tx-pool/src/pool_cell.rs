extern crate rustc_hash;
extern crate slab;
use crate::component::pool_map::PoolMap;
use ckb_types::core::cell::{CellChecker, CellMetaBuilder, CellProvider, CellStatus};
use ckb_types::packed::OutPoint;

pub(crate) struct PoolCell<'a> {
    pub pool_map: &'a PoolMap,
    pub rbf: bool,
}

impl<'a> PoolCell<'a> {
    pub fn new(pool_map: &'a PoolMap, rbf: bool) -> Self {
        PoolCell { pool_map, rbf }
    }

    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if self.pool_map.edges.get_input_ref(out_point).is_some() {
            return CellStatus::Dead;
        }
        if let Some((output, data)) = self.pool_map.get_output_with_data(out_point) {
            let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                .out_point(out_point.to_owned())
                .build();
            CellStatus::live_cell(cell_meta)
        } else {
            CellStatus::Unknown
        }
    }

    fn cell_rbf(&self, out_point: &OutPoint) -> CellStatus {
        if let Some((output, data)) = self.pool_map.get_output_with_data(out_point) {
            let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                .out_point(out_point.to_owned())
                .build();
            {
                //eprintln!("out_point live: {:?} cell_meta: {:?}", out_point, cell_meta);
                CellStatus::live_cell(cell_meta)
            }
        } else {
            CellStatus::Unknown
        }
    }

    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.pool_map.edges.get_input_ref(out_point).is_some() {
            return Some(false);
        }
        if self.pool_map.get_output_with_data(out_point).is_some() {
            return Some(true);
        }
        None
    }

    fn is_live_rbf(&self, out_point: &OutPoint) -> Option<bool> {
        if let Some((_output, _data)) = self.pool_map.get_output_with_data(out_point) {
            Some(true)
        } else {
            None
        }
    }
}

impl<'a> CellProvider for PoolCell<'a> {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        if self.rbf {
            self.cell_rbf(out_point)
        } else {
            self.cell(out_point)
        }
    }
}

impl<'a> CellChecker for PoolCell<'a> {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.rbf {
            self.is_live_rbf(out_point)
        } else {
            self.is_live(out_point)
        }
    }
}
