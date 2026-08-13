//! This crate provides implementation to calculate dao field.

use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::{DaoError, extract_dao_data, pack_dao_data};
use ckb_traits::{CellDataProvider, EpochProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{
        Capacity, CapacityResult, EpochExt, HeaderView, ScriptHashType,
        cell::{CellMeta, CellMetaBuilder, ResolvedTransaction},
    },
    packed::{Byte32, CellOutput, Script, WitnessArgs},
    prelude::*,
};
use std::collections::HashSet;

#[cfg(test)]
mod tests;

/// Dao field calculator
/// `DaoCalculator` is a facade to calculate the dao field.
pub struct DaoCalculator<'a, DL> {
    consensus: &'a Consensus,
    data_loader: &'a DL,
}

/// Secondary issuance allocated for one block.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SecondaryIssuanceBreakdown {
    /// The complete secondary issuance baseline for the block.
    pub total: Capacity,
    /// Compensation for occupied capacity, paid to the block miner.
    pub miner: Capacity,
    /// Secondary issuance reserved for live Nervos DAO deposits.
    pub nervos_dao: Capacity,
    /// Future would-be-burned issuance made available to the treasury.
    pub treasury: Capacity,
}

/// Changes to the counted capacity of live Nervos DAO deposit cells.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DaoDepositCapacityChange {
    /// Counted capacity entering the DAO deposit phase.
    pub added: Capacity,
    /// Counted capacity leaving the DAO deposit phase.
    pub removed: Capacity,
}

/// Splits one block's secondary issuance while preserving the issuance baseline exactly.
///
/// Both the miner and treasury shares are rounded down. Any rounding remainder is retained
/// in the Nervos DAO reserve, so all three shares always sum to `secondary`.
pub fn secondary_issuance_breakdown(
    secondary: Capacity,
    total_issuance: Capacity,
    occupied_capacity: Capacity,
    dao_deposit_capacity: Capacity,
) -> Result<SecondaryIssuanceBreakdown, DaoError> {
    if total_issuance == Capacity::zero() {
        return Err(DaoError::ZeroC);
    }

    let liquid_capacity = total_issuance
        .safe_sub(occupied_capacity)
        .and_then(|capacity| capacity.safe_sub(dao_deposit_capacity))
        .map_err(|_| DaoError::InvalidTreasuryState)?;
    let proportional_share = |capacity: Capacity| -> Result<Capacity, DaoError> {
        let value = u128::from(secondary.as_u64()) * u128::from(capacity.as_u64())
            / u128::from(total_issuance.as_u64());
        Ok(Capacity::shannons(
            u64::try_from(value).map_err(|_| DaoError::Overflow)?,
        ))
    };

    let miner = proportional_share(occupied_capacity)?;
    let treasury = proportional_share(liquid_capacity)?;
    let nervos_dao = secondary
        .safe_sub(miner)?
        .safe_sub(treasury)
        .map_err(DaoError::from)?;

    Ok(SecondaryIssuanceBreakdown {
        total: secondary,
        miner,
        nervos_dao,
        treasury,
    })
}

impl<'a, DL: CellDataProvider + HeaderProvider> DaoCalculator<'a, DL> {
    fn is_dao_deposit_cell(&self, cell_meta: &CellMeta) -> bool {
        let is_dao_type_script = cell_meta
            .cell_output
            .type_()
            .to_opt()
            .map(|type_script| {
                Into::<u8>::into(type_script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
                    && type_script.code_hash() == self.consensus.dao_type_hash()
            })
            .unwrap_or(false);

        is_dao_type_script
            && self
                .data_loader
                .load_cell_data(cell_meta)
                .map(|data| data.as_ref() == [0u8; 8])
                .unwrap_or(false)
    }

    fn counted_capacity(&self, cell_meta: &CellMeta) -> Result<Capacity, DaoError> {
        let occupied = cell_meta.occupied_capacity()?;
        let total: Capacity = cell_meta.cell_output.capacity().into();
        total.safe_sub(occupied).map_err(Into::into)
    }

    /// Calculates how this set of transactions changes live DAO deposit capacity.
    pub fn dao_deposit_capacity_change(
        &self,
        mut rtxs: impl Iterator<Item = &'a ResolvedTransaction> + Clone,
    ) -> Result<DaoDepositCapacityChange, DaoError> {
        let removed = rtxs.clone().try_fold(Capacity::zero(), |total, rtx| {
            rtx.resolved_inputs.iter().try_fold(total, |total, input| {
                if self.is_dao_deposit_cell(input) {
                    total
                        .safe_add(self.counted_capacity(input)?)
                        .map_err(Into::into)
                } else {
                    Ok::<_, DaoError>(total)
                }
            })
        })?;

        let added = rtxs.try_fold(Capacity::zero(), |total, rtx| {
            rtx.transaction
                .outputs_with_data_iter()
                .try_fold(total, |total, (output, data)| {
                    let cell_meta = CellMetaBuilder::from_cell_output(output, data).build();
                    if self.is_dao_deposit_cell(&cell_meta) {
                        total
                            .safe_add(self.counted_capacity(&cell_meta)?)
                            .map_err(Into::into)
                    } else {
                        Ok::<_, DaoError>(total)
                    }
                })
        })?;

        Ok(DaoDepositCapacityChange { added, removed })
    }

    fn treasury_issuance(
        &self,
        mut rtxs: impl Iterator<Item = &'a ResolvedTransaction>,
    ) -> Capacity {
        let Some(config) = self.consensus.treasury() else {
            return Capacity::zero();
        };
        let Some(cellbase) = rtxs.next().filter(|rtx| rtx.transaction.is_cellbase()) else {
            return Capacity::zero();
        };

        cellbase
            .transaction
            .outputs()
            .get(1)
            .filter(|output| output.lock() == config.lock)
            .map(|output| output.capacity().into())
            .unwrap_or_else(Capacity::zero)
    }

    /// Returns the total transactions fee of `rtx`.
    pub fn transaction_fee(&self, rtx: &ResolvedTransaction) -> Result<Capacity, DaoError> {
        let maximum_withdraw = self.transaction_maximum_withdraw(rtx)?;
        rtx.transaction
            .outputs_capacity()
            .and_then(|y| maximum_withdraw.safe_sub(y))
            .map_err(Into::into)
    }

    fn transaction_maximum_withdraw(
        &self,
        rtx: &ResolvedTransaction,
    ) -> Result<Capacity, DaoError> {
        let header_deps: HashSet<Byte32> = rtx.transaction.header_deps_iter().collect();
        rtx.resolved_inputs.iter().enumerate().try_fold(
            Capacity::zero(),
            |capacities, (i, cell_meta)| {
                let capacity: Result<Capacity, DaoError> = {
                    let output = &cell_meta.cell_output;
                    let is_dao_type_script = |type_script: Script| {
                        Into::<u8>::into(type_script.hash_type())
                            == Into::<u8>::into(ScriptHashType::Type)
                            && type_script.code_hash() == self.consensus.dao_type_hash()
                    };
                    let is_dao_output = output
                        .type_()
                        .to_opt()
                        .map(is_dao_type_script)
                        .unwrap_or(false);
                    if is_dao_output {
                        // A withdrawing DAO cell has 8 bytes of cell data storing the
                        // block number of the original deposit.
                        let deposited_block_number =
                            match self.data_loader.load_cell_data(cell_meta) {
                                Some(data) if data.len() == 8 => LittleEndian::read_u64(&data),
                                _ => 0,
                            };
                        if deposited_block_number > 0 {
                            let withdrawing_header_hash = cell_meta
                                .transaction_info
                                .as_ref()
                                .map(|info| &info.block_hash)
                                .filter(|hash| header_deps.contains(hash))
                                .ok_or(DaoError::InvalidOutPoint)?;
                            let deposit_header_hash = rtx
                                .transaction
                                .witnesses()
                                .get(i)
                                .ok_or(DaoError::InvalidOutPoint)
                                .and_then(|witness_data| {
                                    // dao contract stores header deps index as u64 in the input_type field of WitnessArgs
                                    let witness =
                                        WitnessArgs::from_slice(&Into::<Bytes>::into(witness_data))
                                            .map_err(|_| DaoError::InvalidDaoFormat)?;
                                    let header_deps_index_data: Option<Bytes> =
                                        witness.input_type().to_opt().map(|witness| witness.into());
                                    if header_deps_index_data.is_none()
                                        || header_deps_index_data.clone().map(|data| data.len())
                                            != Some(8)
                                    {
                                        return Err(DaoError::InvalidDaoFormat);
                                    }
                                    Ok(LittleEndian::read_u64(&header_deps_index_data.unwrap()))
                                })
                                .and_then(|header_dep_index| {
                                    rtx.transaction
                                        .header_deps()
                                        .get(header_dep_index as usize)
                                        .and_then(|hash| header_deps.get(&hash))
                                        .ok_or(DaoError::InvalidOutPoint)
                                })?;

                            let deposit_header = self
                                .data_loader
                                .get_header(deposit_header_hash)
                                .ok_or(DaoError::InvalidHeader)?;
                            if deposit_header.number() != deposited_block_number {
                                return Err(DaoError::InvalidOutPoint);
                            }
                            self.calculate_maximum_withdraw(
                                output,
                                Capacity::bytes(cell_meta.data_bytes as usize)?,
                                deposit_header_hash,
                                withdrawing_header_hash,
                            )
                        } else {
                            Ok(output.capacity().into())
                        }
                    } else {
                        Ok(output.capacity().into())
                    }
                };
                capacity.and_then(|c| c.safe_add(capacities).map_err(Into::into))
            },
        )
    }

    /// Calculate maximum withdraw capacity of a deposited dao output
    pub fn calculate_maximum_withdraw(
        &self,
        output: &CellOutput,
        output_data_capacity: Capacity,
        deposit_header_hash: &Byte32,
        withdrawing_header_hash: &Byte32,
    ) -> Result<Capacity, DaoError> {
        let deposit_header = self
            .data_loader
            .get_header(deposit_header_hash)
            .ok_or(DaoError::InvalidHeader)?;
        let withdrawing_header = self
            .data_loader
            .get_header(withdrawing_header_hash)
            .ok_or(DaoError::InvalidHeader)?;
        if deposit_header.number() >= withdrawing_header.number() {
            return Err(DaoError::InvalidOutPoint);
        }

        let (deposit_ar, _, _, _) = extract_dao_data(deposit_header.dao());
        let (withdrawing_ar, _, _, _) = extract_dao_data(withdrawing_header.dao());

        let occupied_capacity = output.occupied_capacity(output_data_capacity)?;
        let output_capacity: Capacity = output.capacity().into();
        let counted_capacity = output_capacity.safe_sub(occupied_capacity)?;
        let withdraw_counted_capacity = u128::from(counted_capacity.as_u64())
            * u128::from(withdrawing_ar)
            / u128::from(deposit_ar);
        let withdraw_counted_capacity =
            u64::try_from(withdraw_counted_capacity).map_err(|_| DaoError::Overflow)?;
        let withdraw_capacity =
            Capacity::shannons(withdraw_counted_capacity).safe_add(occupied_capacity)?;

        Ok(withdraw_capacity)
    }

    /// Creates a new `DaoCalculator`.
    pub fn new(consensus: &'a Consensus, data_loader: &'a DL) -> Self {
        DaoCalculator {
            consensus,
            data_loader,
        }
    }
}

impl<'a, DL: CellDataProvider + EpochProvider + HeaderProvider> DaoCalculator<'a, DL> {
    /// Returns the primary block reward for `target` block.
    pub fn primary_block_reward(&self, target: &HeaderView) -> Result<Capacity, DaoError> {
        let target_epoch = self
            .data_loader
            .get_epoch_ext(target)
            .ok_or(DaoError::InvalidHeader)?;

        target_epoch
            .block_reward(target.number())
            .map_err(Into::into)
    }

    /// Returns the secondary block reward for `target` block.
    pub fn secondary_block_reward(&self, target: &HeaderView) -> Result<Capacity, DaoError> {
        if target.number() == 0 {
            return Ok(Capacity::zero());
        }

        let target_parent_hash = target.data().raw().parent_hash();
        let target_parent = self
            .data_loader
            .get_header(&target_parent_hash)
            .ok_or(DaoError::InvalidHeader)?;
        let target_epoch = self
            .data_loader
            .get_epoch_ext(target)
            .ok_or(DaoError::InvalidHeader)?;

        let target_g2 = target_epoch
            .secondary_block_issuance(target.number(), self.consensus.secondary_epoch_reward())?;
        let (_, target_parent_c, _, target_parent_u) = extract_dao_data(target_parent.dao());
        let reward128 = u128::from(target_g2.as_u64()) * u128::from(target_parent_u.as_u64())
            / u128::from(target_parent_c.as_u64());
        let reward = u64::try_from(reward128).map_err(|_| DaoError::Overflow)?;
        Ok(Capacity::shannons(reward))
    }

    /// Calculates the new dao field with specified [`EpochExt`].
    pub fn dao_field_with_current_epoch(
        &self,
        rtxs: impl Iterator<Item = &'a ResolvedTransaction> + Clone,
        parent: &HeaderView,
        current_block_epoch: &EpochExt,
    ) -> Result<Byte32, DaoError> {
        // Freed occupied capacities from consumed inputs
        let freed_occupied_capacities =
            rtxs.clone().try_fold(Capacity::zero(), |capacities, rtx| {
                self.input_occupied_capacities(rtx)
                    .and_then(|c| capacities.safe_add(c))
            })?;
        let added_occupied_capacities = self.added_occupied_capacities(rtxs.clone())?;
        let treasury_issuance = self.treasury_issuance(rtxs.clone());
        let withdrawed_interests = self.withdrawed_interests(rtxs)?;

        let (parent_ar, parent_c, parent_s, parent_u) = extract_dao_data(parent.dao());

        // g contains both primary issuance and secondary issuance,
        // g2 is the secondary issuance for the block, which consists of
        // issuance for the miner, NervosDAO and treasury.
        // When calculating issuance in NervosDAO, we use the real
        // issuance for each block(which will only be issued on chain
        // after the finalization delay), not the capacities generated
        // in the cellbase of current block.
        let current_block_number = parent.number() + 1;
        let current_g2 = current_block_epoch.secondary_block_issuance(
            current_block_number,
            self.consensus.secondary_epoch_reward(),
        )?;
        let current_g = current_block_epoch
            .block_reward(current_block_number)
            .and_then(|c| c.safe_add(current_g2))?;

        let miner_issuance128 = u128::from(current_g2.as_u64()) * u128::from(parent_u.as_u64())
            / u128::from(parent_c.as_u64());
        let miner_issuance =
            Capacity::shannons(u64::try_from(miner_issuance128).map_err(|_| DaoError::Overflow)?);
        let nervosdao_issuance = current_g2.safe_sub(miner_issuance)?;

        let current_c = parent_c.safe_add(current_g)?;
        let current_u = parent_u
            .safe_add(added_occupied_capacities)
            .and_then(|u| u.safe_sub(freed_occupied_capacities))?;
        let current_s = parent_s
            .safe_add(nervosdao_issuance)
            .and_then(|s| s.safe_sub(withdrawed_interests))
            .and_then(|s| s.safe_sub(treasury_issuance))?;

        let ar_increase128 =
            u128::from(parent_ar) * u128::from(current_g2.as_u64()) / u128::from(parent_c.as_u64());
        let ar_increase = u64::try_from(ar_increase128).map_err(|_| DaoError::Overflow)?;
        let current_ar = parent_ar
            .checked_add(ar_increase)
            .ok_or(DaoError::Overflow)?;

        Ok(pack_dao_data(current_ar, current_c, current_s, current_u))
    }

    /// Calculates the new dao field after packaging these transactions. It returns the dao field in [`Byte32`] format. Please see [`extract_dao_data`] if you intend to see the detailed content.
    ///
    /// [`Byte32`]: ../ckb_types/packed/struct.Byte32.html
    /// [`extract_dao_data`]: ../ckb_dao_utils/fn.extract_dao_data.html
    pub fn dao_field(
        &self,
        rtxs: impl Iterator<Item = &'a ResolvedTransaction> + Clone,
        parent: &HeaderView,
    ) -> Result<Byte32, DaoError> {
        let current_block_epoch = self
            .consensus
            .next_epoch_ext(parent, self.data_loader)
            .ok_or(DaoError::InvalidHeader)?
            .epoch();
        self.dao_field_with_current_epoch(rtxs, parent, &current_block_epoch)
    }

    fn added_occupied_capacities(
        &self,
        mut rtxs: impl Iterator<Item = &'a ResolvedTransaction>,
    ) -> CapacityResult<Capacity> {
        // Newly added occupied capacities from outputs
        let added_occupied_capacities = rtxs.try_fold(Capacity::zero(), |capacities, rtx| {
            rtx.transaction
                .outputs_with_data_iter()
                .enumerate()
                .try_fold(Capacity::zero(), |tx_capacities, (_, (output, data))| {
                    Capacity::bytes(data.len())
                        .and_then(|c| output.occupied_capacity(c))
                        .and_then(|c| tx_capacities.safe_add(c))
                })
                .and_then(|c| capacities.safe_add(c))
        })?;

        Ok(added_occupied_capacities)
    }

    fn input_occupied_capacities(&self, rtx: &ResolvedTransaction) -> CapacityResult<Capacity> {
        rtx.resolved_inputs
            .iter()
            .try_fold(Capacity::zero(), |capacities, cell_meta| {
                let current_capacity = modified_occupied_capacity(cell_meta, self.consensus);
                current_capacity.and_then(|c| capacities.safe_add(c))
            })
    }

    fn withdrawed_interests(
        &self,
        mut rtxs: impl Iterator<Item = &'a ResolvedTransaction> + Clone,
    ) -> Result<Capacity, DaoError> {
        let maximum_withdraws = rtxs.clone().try_fold(Capacity::zero(), |capacities, rtx| {
            self.transaction_maximum_withdraw(rtx)
                .and_then(|c| capacities.safe_add(c).map_err(Into::into))
        })?;
        let input_capacities = rtxs.try_fold(Capacity::zero(), |capacities, rtx| {
            let tx_input_capacities = rtx.resolved_inputs.iter().try_fold(
                Capacity::zero(),
                |tx_capacities, cell_meta| {
                    let output_capacity: Capacity = cell_meta.cell_output.capacity().into();
                    tx_capacities.safe_add(output_capacity)
                },
            )?;
            capacities.safe_add(tx_input_capacities)
        })?;
        maximum_withdraws
            .safe_sub(input_capacities)
            .map_err(Into::into)
    }
}

/// return special occupied capacity if cell is satoshi's gift
/// otherwise return cell occupied capacity
pub fn modified_occupied_capacity(
    cell_meta: &CellMeta,
    consensus: &Consensus,
) -> CapacityResult<Capacity> {
    if let Some(tx_info) = &cell_meta.transaction_info
        && tx_info.is_genesis()
        && tx_info.is_cellbase()
        && cell_meta.cell_output.lock().args().raw_data() == consensus.satoshi_pubkey_hash.0[..]
    {
        return Into::<Capacity>::into(cell_meta.cell_output.capacity())
            .safe_mul_ratio(consensus.satoshi_cell_occupied_ratio);
    }
    cell_meta.occupied_capacity()
}
