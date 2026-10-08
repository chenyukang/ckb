use crate::relayer::transactions_process::dedup_transactions;
use ckb_types::core::TransactionBuilder;

fn dummy_tx(nonce: u64) -> ckb_types::core::TransactionView {
    // The transaction hash excludes witnesses, so vary a raw field to get a
    // distinct hash for each body.
    TransactionBuilder::default().version(nonce as u32).build()
}

#[test]
fn dedup_keeps_first_body_per_hash() {
    let tx = dummy_tx(0);
    let other = dummy_tx(1);
    let tx_hash = tx.hash();
    let other_hash = other.hash();
    assert_ne!(tx_hash, other_hash);

    let deduped = dedup_transactions(vec![
        (tx.clone(), 1),
        (tx.clone(), 1),
        (other.clone(), 2),
        (tx, 3),
    ]);

    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].0.hash(), tx_hash);
    assert_eq!(deduped[0].1, 1);
    assert_eq!(deduped[1].0.hash(), other_hash);
    assert_eq!(deduped[1].1, 2);
}

#[test]
fn dedup_preserves_distinct_bodies() {
    let bodies: Vec<_> = (0..8).map(|i| (dummy_tx(i), i)).collect();

    let deduped = dedup_transactions(bodies);

    assert_eq!(deduped.len(), 8);
}
