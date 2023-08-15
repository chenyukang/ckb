use crate::generic::GetProposalTxIds;
use crate::util::cell::gen_spendable;
use crate::util::transaction::always_success_transaction;
use crate::{Node, Spec};
use ckb_jsonrpc_types::Status;
use ckb_types::packed::CellInput;
use ckb_types::packed::OutPoint;
use ckb_types::prelude::*;

pub struct AvoidDuplicatedProposalsWithUncles;

impl Spec for AvoidDuplicatedProposalsWithUncles {
    // Case: This is not a validation rule, but just an improvement for miner
    //       filling proposals: Don't re-propose the transactions which
    //       has already been proposed within the uncles.
    //    1. Submit `tx` into mempool, and `uncle` which proposed `tx` as an candidate uncle
    //    2. Get block template, expect empty proposals cause we already proposed `tx` within `uncle`

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let cells = gen_spendable(node, 1);
        let tx = always_success_transaction(node, &cells[0]);
        let uncle = {
            let block = node.new_block(None, None, None);
            let uncle = block
                .as_advanced_builder()
                .timestamp((block.timestamp() + 1).pack())
                .set_proposals(vec![tx.proposal_short_id()])
                .build();
            node.submit_block(&block);
            uncle
        };
        node.submit_block(&uncle);
        node.submit_transaction(&tx);

        let block = node.new_block_with_blocking(|template| template.uncles.is_empty());
        assert_eq!(
            vec![uncle.hash()],
            block
                .uncles()
                .into_iter()
                .map(|u| u.hash())
                .collect::<Vec<_>>()
        );
        assert!(
            block.get_proposal_tx_ids().is_empty(),
            "expect empty proposals, actual: {:?}",
            block.get_proposal_tx_ids()
        );
    }
}

pub struct AvoidReproposeGap;
impl Spec for AvoidReproposeGap {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0.clone()];

        eprintln!("tx0 now: {:?}", tx0.hash());
        while txs.len() < 10 {
            let parent = txs.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            txs.push(child);
        }
        for tx in txs.iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            if ret.is_err() {
                eprintln!("send tx: {:?} error: {:?}", tx.hash(), ret);
            }
            assert!(ret.is_ok());
            eprintln!("send tx: {:?}", tx.hash());
        }

        node0.mine_with_blocking(|template| template.proposals.len() != txs.len());
        let res = node0.rpc_client().get_raw_tx_pool(Some(true));
        eprintln!("tx_pool: {:?}", res);

        let ret = node0.rpc_client().get_transaction(txs[2].hash());
        assert!(
            matches!(ret.tx_status.status, Status::Pending),
            "tx1 should be pending"
        );
        node0.mine(1);
        let ret = node0.rpc_client().get_transaction(txs[2].hash());
        assert!(
            matches!(ret.tx_status.status, Status::Proposed),
            "tx1 should be proposed"
        );
    }
}
