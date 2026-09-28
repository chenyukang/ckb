use ckb_merkle_mountain_range::Merge;
use numext_fixed_uint::U256;

use crate::{
    core::BlockNumber,
    packed,
    prelude::*,
    utilities::merkle_mountain_range::{HeaderDigest as _, MergeHeaderDigest},
};

/// Builds a digest over `[start_number, end_number]` carrying `total_difficulty`.
///
/// Epochs are left at their defaults, which is the genesis epoch, so that the
/// merge epoch check passes and the tests below exercise the arithmetic.
fn digest(
    start_number: BlockNumber,
    end_number: BlockNumber,
    total_difficulty: U256,
) -> packed::HeaderDigest {
    packed::HeaderDigest::new_builder()
        .total_difficulty(total_difficulty)
        .start_number(start_number)
        .end_number(end_number)
        .build()
}

#[test]
fn test_merge_still_merges_canonical_digests() {
    let lhs = digest(0, 0, U256::from(1u64));
    let rhs = digest(1, 1, U256::from(2u64));

    let merged = MergeHeaderDigest::merge(&lhs, &rhs).expect("canonical digests should merge");
    let total_difficulty: U256 = merged.total_difficulty().into();
    let start_number: BlockNumber = merged.start_number().into();
    let end_number: BlockNumber = merged.end_number().into();

    assert_eq!(total_difficulty, U256::from(3u64));
    assert_eq!(start_number, 0);
    assert_eq!(end_number, 1);
}

// `U256::MAX` as a total difficulty must fail the merge rather than overflow
// the accumulator.
#[test]
fn test_merge_rejects_total_difficulty_overflow() {
    let max_difficulty = digest(0, 0, U256::max_value());
    let one = digest(1, 1, U256::one());

    assert!(MergeHeaderDigest::merge(&max_difficulty, &one).is_err());
    assert!(MergeHeaderDigest::merge(&one, &max_difficulty).is_err());

    let both = digest(1, 1, U256::max_value());
    assert!(MergeHeaderDigest::merge(&max_difficulty, &both).is_err());
}

// `lhs_end_number + 1` must fail the continuity check rather than wrap, and a
// digest ending at `u64::MAX` must not be treated as adjacent to one starting
// at `u64::MAX`.
#[test]
fn test_merge_rejects_block_number_overflow() {
    let lhs = digest(BlockNumber::MAX, BlockNumber::MAX, U256::one());
    let rhs = digest(BlockNumber::MAX, BlockNumber::MAX, U256::one());

    assert!(MergeHeaderDigest::merge(&lhs, &rhs).is_err());
}

// `end_number - start_number + 1` overflows for a digest claiming the whole
// `u64` range in one epoch.
#[test]
fn test_verify_rejects_block_count_overflow() {
    let whole_range = digest(0, BlockNumber::MAX, U256::one());

    assert!(whole_range.verify().is_err());
}
