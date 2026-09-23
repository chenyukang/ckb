use crate::relayer::block_transactions_process::BlockTransactionsProcess;
use crate::relayer::compact_block_process::CompactBlockProcess;
use crate::relayer::tests::helper::{MockProtocolContext, build_chain, gen_block};
use crate::types::PendingCompactBlock;
use crate::{Status, StatusCode};
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_store::ChainStore;
use ckb_tx_pool::{PlugTarget, TxEntry};
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, TransactionBuilder},
    packed::{
        self, BlockTransactions, CellInput, CellOutputBuilder, CompactBlock, Header,
        IndexTransaction, OutPoint,
    },
};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn test_missing_requested_uncle_response_is_rejected() {
    let (_chain, relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let tx1 = TransactionBuilder::default().build();
    let uncle = BlockBuilder::default().build();

    let block = BlockBuilder::default()
        .transaction(tx1)
        .uncle(uncle.as_uncle())
        .build();

    let prefilled = vec![0usize].into_iter().collect();
    let compact_block = CompactBlock::build_from_block(&block, &prefilled);
    let block_hash = compact_block.header().calc_header_hash();

    {
        let mut pending_compact_blocks =
            rt.block_on(relayer.shared.state().pending_compact_blocks());
        pending_compact_blocks.insert(
            block_hash.clone(),
            (
                HashMap::from_iter(vec![(
                    peer_index,
                    PendingCompactBlock {
                        compact_block,
                        missing_transactions: vec![],
                        missing_uncles: vec![0],
                    },
                )]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block_hash)
        .transactions(Vec::<packed::Transaction>::new())
        .uncles(Vec::<packed::UncleBlock>::new())
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::BlockUnclesLengthIsUnmatchedWithPendingCompactBlock.into(),
    );
}

#[test]
fn test_accept_block() {
    let (_chain, relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();
    let other_peer_index: PeerIndex = 101.into();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let tx3 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    let uncle = BlockBuilder::default()
        .proposals(vec![tx3.proposal_short_id()])
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1, tx2.clone()])
        .uncle(uncle.as_uncle())
        .build();
    let prefilled = vec![0usize].into_iter().collect();

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);
    let hash = compact_block.header().calc_header_hash();

    {
        let mut pending_compact_blocks =
            rt.block_on(relayer.shared.state().pending_compact_blocks());
        pending_compact_blocks.insert(
            hash.clone(),
            (
                HashMap::from_iter(vec![
                    (
                        peer_index,
                        PendingCompactBlock {
                            compact_block: compact_block.clone(),
                            missing_transactions: vec![1],
                            missing_uncles: vec![0],
                        },
                    ),
                    (
                        other_peer_index,
                        PendingCompactBlock {
                            compact_block,
                            missing_transactions: vec![1],
                            missing_uncles: vec![],
                        },
                    ),
                ]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()])
        .uncles(vec![uncle.as_uncle().data()])
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    assert_eq!(rt.block_on(process.execute()), Status::ok());

    let pending_compact_blocks = rt.block_on(relayer.shared.state().pending_compact_blocks());
    assert!(pending_compact_blocks.get(&hash).is_none());

    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        relayer
            .shared
            .state()
            .contains_inflight_proposal(&tx3.proposal_short_id())
    );
}

#[test]
fn test_unknown_request() {
    let (_chain, relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1, tx2.clone()])
        .build();

    let prefilled = vec![0usize].into_iter().collect();

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    let foo_peer_index: PeerIndex = 998.into();
    {
        let mut pending_compact_blocks =
            rt.block_on(relayer.shared.state().pending_compact_blocks());
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash(),
            (
                HashMap::from_iter(vec![(
                    foo_peer_index,
                    PendingCompactBlock {
                        compact_block,
                        missing_transactions: vec![1],
                        missing_uncles: vec![],
                    },
                )]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()])
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(rt.block_on(process.execute()), Status::ignored());
}

#[test]
fn test_invalid_transaction_root() {
    let (_chain, relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let prefilled = IndexTransaction::new_builder()
        .index(0u32)
        .transaction(tx1.data())
        .build();

    let header_with_invalid_tx_root = Header::new_builder()
        .raw(
            packed::RawHeader::new_builder()
                .transactions_root(packed::Byte32::zero())
                .build(),
        )
        .build();

    let compact_block = packed::CompactBlock::new_builder()
        .header(header_with_invalid_tx_root)
        .short_ids(vec![tx2.proposal_short_id()])
        .prefilled_transactions(vec![prefilled])
        .build();

    let block_hash = compact_block.header().calc_header_hash();

    {
        let mut pending_compact_blocks =
            rt.block_on(relayer.shared.state().pending_compact_blocks());
        pending_compact_blocks.insert(
            block_hash.clone(),
            (
                HashMap::from_iter(vec![(
                    peer_index,
                    PendingCompactBlock {
                        compact_block,
                        missing_transactions: vec![1],
                        missing_uncles: vec![],
                    },
                )]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block_hash)
        .transactions(vec![tx2.data()])
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock.into(),
    );
}

#[test]
fn test_collision_and_send_missing_indexes() {
    let (_chain, relayer, _) = build_chain(5);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let active_chain = relayer.shared.active_chain();
    let last_block = relayer
        .shared
        .store()
        .get_block(&active_chain.tip_hash())
        .unwrap();
    let last_cellbase = last_block.transactions().first().cloned().unwrap();

    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1000).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    let tx3 = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(last_cellbase.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let fake_hash = tx3
        .hash()
        .as_builder()
        .nth31(0u8)
        .nth30(0u8)
        .nth29(0u8)
        .nth28(0u8)
        .build();
    // Fake tx with the same ProposalShortId but different hash with tx3
    let fake_tx = tx3.clone().fake_hash(fake_hash);

    assert_eq!(tx3.proposal_short_id(), fake_tx.proposal_short_id());
    assert_ne!(tx3.hash(), fake_tx.hash());

    let block = BlockBuilder::default()
        .transactions(vec![tx1, tx2.clone(), fake_tx])
        .build_unchecked();

    let prefilled = vec![0usize].into_iter().collect();

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    {
        let tx_pool = relayer.shared.shared().tx_pool_controller();
        let entry = TxEntry::dummy_resolve(tx3.clone(), 0, Capacity::shannons(0), 0);
        tx_pool
            .plug_entry(vec![entry], PlugTarget::Pending)
            .unwrap();
    }

    let hash = compact_block.header().calc_header_hash();
    {
        let mut pending_compact_blocks =
            rt.block_on(relayer.shared.state().pending_compact_blocks());
        pending_compact_blocks.insert(
            hash.clone(),
            (
                vec![(
                    peer_index,
                    PendingCompactBlock {
                        compact_block,
                        missing_transactions: vec![1],
                        missing_uncles: vec![],
                    },
                )]
                .into_iter()
                .collect(),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    let block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()])
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::CompactBlockMeetsShortIdsCollision.into()
    );

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes([1u32, 2u32])
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_bytes();

    std::thread::sleep(std::time::Duration::from_millis(100));
    // send c indexes messages
    assert!(nc.has_sent(SupportProtocols::RelayV3.protocol_id(), peer_index, data));

    // update cached missing_index
    {
        let pending_compact_blocks = rt.block_on(relayer.shared.state().pending_compact_blocks());
        let (peers_map, _) = pending_compact_blocks.get(&hash).unwrap();
        let peer_pending = peers_map.get(&peer_index).unwrap();
        assert_eq!(peer_pending.missing_transactions, vec![1, 2]);
        assert!(peer_pending.missing_uncles.is_empty());
    }

    // resend BlockTransactions with all the transactions without prefilled
    let new_block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data(), tx3.data()])
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        new_block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock.into(),
    );
}

#[test]
fn test_missing() {
    // test case:
    //  1. Relayer received a compact block with 3 tx
    //  2. Assuming that tx1 is prefilled, tx2 makes a request, and tx3 is in the transaction pool
    //  3. When the relayer receives the return from tx2, tx3 is not in the transaction pool
    //  4. Relayer must make another request for tx2 and tx3

    let (_chain, relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    let tx3 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1, tx2.clone(), tx3])
        .build();

    let prefilled = vec![0usize].into_iter().collect();

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    // tx3 should be in tx_pool already, but it's not.
    // so the reconstruct block will fail
    {
        let mut pending_compact_blocks =
            rt.block_on(relayer.shared.state().pending_compact_blocks());
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash(),
            (
                HashMap::from_iter(vec![(
                    peer_index,
                    PendingCompactBlock {
                        compact_block,
                        missing_transactions: vec![1],
                        missing_uncles: vec![],
                    },
                )]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    let block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()])
        .build();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::CompactBlockRequiresFreshTransactions.into()
    );

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes([1, 2u32])
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();

    std::thread::sleep(std::time::Duration::from_millis(100));
    // send missing indexes messages
    assert!(nc.has_sent(
        SupportProtocols::RelayV3.protocol_id(),
        peer_index,
        message.as_bytes()
    ));
}

// Regression test for the cross-session compact-block reconstruction panic.
//
// Two peers announce different compact block layouts (different short-id
// counts) under the same header hash. The pending record used to keep the
// first peer's compact block while recording each later peer's missing
// indexes, so a response from the second peer was verified against the first
// peer's layout and an out-of-range index panicked the relay task. The
// compact block is now stored together with the indexes of the peer that
// produced them, so every response is verified against its own layout.
#[test]
fn test_peer_response_is_verified_against_its_own_compact_block_layout() {
    let (_chain, relayer, _) = build_chain(5);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let peer_a: PeerIndex = 200.into();
    let peer_b: PeerIndex = 201.into();

    let parent = relayer.shared.active_chain().tip_header();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    let tx3 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap())
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    let block = gen_block(&parent, relayer.shared.shared(), 0, 1, None)
        .as_advanced_builder()
        .transaction(tx2.clone())
        .transaction(tx3.clone())
        .build();

    let prefilled = vec![0usize].into_iter().collect();
    // Peer B announces both non-cellbase transactions as short ids.
    let compact_block_b = CompactBlock::build_from_block(&block, &prefilled);
    assert_eq!(compact_block_b.short_ids().len(), 2);
    // Peer A announces the same header with a shorter layout: only the first
    // short id, so peer A's indexes can not describe peer B's positions.
    let compact_block_a = compact_block_b
        .clone()
        .as_builder()
        .short_ids(vec![tx2.proposal_short_id()])
        .build();
    let block_hash = compact_block_b.header().calc_header_hash();

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);

    // Peer A registers the pending entry first, with the shorter layout.
    let status_a = rt.block_on(
        CompactBlockProcess::new(
            compact_block_a.as_reader(),
            &relayer,
            Arc::<MockProtocolContext>::clone(&nc),
            peer_a,
        )
        .execute(),
    );
    assert_eq!(
        status_a,
        StatusCode::CompactBlockRequiresFreshTransactions.into()
    );

    // Peer B announces the longer layout for the same header.
    let status_b = rt.block_on(
        CompactBlockProcess::new(
            compact_block_b.as_reader(),
            &relayer,
            Arc::<MockProtocolContext>::clone(&nc),
            peer_b,
        )
        .execute(),
    );
    assert_eq!(
        status_b,
        StatusCode::CompactBlockRequiresFreshTransactions.into()
    );

    {
        let pending_compact_blocks = rt.block_on(relayer.shared.state().pending_compact_blocks());
        let (peers_map, _) = pending_compact_blocks.get(&block_hash).unwrap();
        let pending_a = peers_map.get(&peer_a).unwrap();
        assert_eq!(
            pending_a.compact_block.as_slice(),
            compact_block_a.as_slice()
        );
        assert_eq!(pending_a.missing_transactions, vec![1]);
        let pending_b = peers_map.get(&peer_b).unwrap();
        assert_eq!(
            pending_b.compact_block.as_slice(),
            compact_block_b.as_slice()
        );
        assert_eq!(pending_b.missing_transactions, vec![1, 2]);
    }

    // Peer B answers the request with exactly the transactions of its own
    // layout. This used to index peer A's two-position short-id table with
    // index 2 and panic; now it verifies and reconstructs successfully.
    let block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block_hash.clone())
        .transactions(vec![tx2.data(), tx3.data()])
        .build();
    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_b,
    );
    assert_eq!(rt.block_on(process.execute()), Status::ok());

    let pending_compact_blocks = rt.block_on(relayer.shared.state().pending_compact_blocks());
    assert!(pending_compact_blocks.get(&block_hash).is_none());
}
