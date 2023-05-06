extern crate test;
use crate::component::entry::TxEntry;
use crate::component::tests::util::DEFAULT_MAX_ANCESTORS_COUNT;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{CellInput, OutPoint},
    prelude::*,
};
use std::hint::black_box;
use test::Bencher;

#[bench]
fn test_container_bench_add(bench: &mut Bencher) {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    use crate::component::container::SortedTxMap;
    let mut map = SortedTxMap::new(1000000);
    let tx1 = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        100,
        Capacity::shannons(100),
        100,
    );
    map.add_entry(black_box(tx1.clone())).unwrap();
    let mut prev_tx = tx1.clone();
    bench.iter(|| {
        let next_tx = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(prev_tx.transaction().hash())
                                .index(0u32.pack())
                                .build(),
                        )
                        .build(),
                )
                .witness(Bytes::new().pack())
                .build(),
            rng.gen_range(0..1000),
            Capacity::shannons(200),
            rng.gen_range(0..1000),
        );
        map.add_entry(black_box(next_tx.clone())).unwrap();
        if map.size() > 10000 {
            map.clear();
        }
        //map.remove_entry(black_box(&next_tx.proposal_short_id()));
        prev_tx = next_tx;
    });
}

#[bench]
fn test_container_bench_add_remove(bench: &mut Bencher) {
    use crate::component::container::SortedTxMap;
    let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    let tx1 = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        100,
        Capacity::shannons(100),
        100,
    );
    let tx2 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(
                        OutPoint::new_builder()
                            .tx_hash(tx1.transaction().hash())
                            .index(0u32.pack())
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new().pack())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx3 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(
                        OutPoint::new_builder()
                            .tx_hash(tx2.transaction().hash())
                            .index(0u32.pack())
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new().pack())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    bench.iter(|| {
        for _ in 0..1000 {
            let tx1_id = tx1.proposal_short_id();
            let tx2_id = tx2.proposal_short_id();
            let tx3_id = tx3.proposal_short_id();
            map.add_entry(black_box(tx1.clone())).unwrap();
            map.add_entry(black_box(tx2.clone())).unwrap();
            map.add_entry(black_box(tx3.clone())).unwrap();
            map.remove_entry(black_box(&tx1_id));
            map.remove_entry(black_box(&tx2_id));
            map.remove_entry(black_box(&tx3_id));
        }
    });
}

#[bench]
fn test_container_bench_sort(bench: &mut Bencher) {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    use crate::component::container::SortedTxMap;
    let mut map = SortedTxMap::new(1000000);
    let tx1 = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        100,
        Capacity::shannons(100),
        100,
    );
    map.add_entry(black_box(tx1.clone())).unwrap();
    let mut prev_tx = tx1.clone();
    for _ in 0..1000 {
        let next_tx = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(prev_tx.transaction().hash())
                                .index(0u32.pack())
                                .build(),
                        )
                        .build(),
                )
                .witness(Bytes::new().pack())
                .build(),
            rng.gen_range(0..1000),
            Capacity::shannons(200),
            rng.gen_range(0..1000),
        );
        map.add_entry(black_box(next_tx.clone())).unwrap();
        prev_tx = next_tx;
    }

    bench.iter(|| {
        //let t = std::time::Instant::now();
        for _ in 0..100 {
            let _entries = black_box(map.score_sorted_iter().collect::<Vec<_>>());
        }
        //eprintln!("{:0.2?}", t.elapsed())
    });
}

