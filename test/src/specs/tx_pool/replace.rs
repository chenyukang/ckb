use crate::{
    rpc::RpcClient,
    util::{
        cell::gen_spendable,
        transaction::{
            always_success_transaction, always_success_transactions, get_tx_pool_conflicts,
        },
    },
    utils::wait_until,
    Node, Spec,
};
use ckb_jsonrpc_types::Status;
use ckb_logger::info;
use ckb_types::{
    core::{capacity_bytes, cell::CellMetaBuilder, Capacity, DepType, TransactionView},
    packed::{Byte32, CellDep, CellDepBuilder, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
    H256,
};

pub struct RbfEnable;
impl Spec for RbfEnable {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        let tx1 = node0.new_transaction(tx_hash_0);

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx1 = tx1.as_advanced_builder().set_outputs(vec![output]).build();

        node0.rpc_client().send_transaction(tx1.data().into());
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 2);

        assert_eq!(ret.min_replace_fee, None);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(100);
        config.tx_pool.min_fee_rate = ckb_types::core::FeeRate(100);
    }
}

pub struct RbfBasic;
impl Spec for RbfBasic {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(99).pack())
            .build();

        let tx1 = tx1.as_advanced_builder().set_outputs(vec![output]).build();
        // assume tx1's replace fee is ok
        node0.rpc_client().send_transaction(tx1.data().into());
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 2);
        // min_replace_fee is 363
        // fee is 100000000
        assert_eq!(ret.fee.unwrap().to_string(), "0x5f5e100");
        // replace fee is 100000363
        assert_eq!(ret.min_replace_fee.unwrap().to_string(), "0x5f5e26b");

        // Set tx2 fee to a higher value, tx1 capacity is 99, set tx2 capacity to 95 for +4 fee.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(95).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_ok(), "tx2 should replace with old tx");
        assert_eq!(get_tx_pool_conflicts(node0), vec![tx1.hash().unpack()]);

        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx2.hash(), 2);
        // fee is 500000000
        assert!(ret.fee.unwrap().to_string() == "0x1dcd6500");
        // replace fee is 500000363
        assert!(ret.min_replace_fee.unwrap().to_string() == "0x1dcd666b");

        node0.mine_with_blocking(|template| template.proposals.len() != 2);
        node0.mine_with_blocking(|template| template.number.value() != 14);
        node0.mine_with_blocking(|template| template.transactions.len() != 2);

        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .transactions()
            .iter()
            .map(TransactionView::hash)
            .collect();

        // RBF (Replace-By-Fees) is enabled
        assert!(!commit_txs_hash.contains(&tx1.hash()));
        assert!(commit_txs_hash.contains(&tx2.hash()));

        // when tx2 should be committed
        let ret = node0.rpc_client().get_transaction(tx2.hash());
        assert!(
            matches!(ret.tx_status.status, Status::Committed),
            "tx2 should be committed"
        );

        // verbosity = 1
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 1);
        assert!(ret.transaction.is_none());
        assert!(matches!(ret.tx_status.status, Status::Rejected));
        assert!(ret.tx_status.reason.unwrap().contains("RBFRejected"));

        // verbosity = 2
        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx2.hash(), 2);
        assert!(ret.transaction.is_some());
        assert!(matches!(ret.tx_status.status, Status::Committed));

        let ret = node0
            .rpc_client()
            .get_transaction_with_verbosity(tx1.hash(), 2);
        assert!(ret.transaction.is_none());
        assert!(matches!(ret.tx_status.status, Status::Rejected));
        assert!(ret.tx_status.reason.unwrap().contains("RBFRejected"));
        assert_eq!(get_tx_pool_conflicts(node0), vec![tx1.hash().unpack()]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = ckb_types::core::FeeRate(1000);
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfSameInput;
impl Spec for RbfSameInput {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);

        let tx2 = tx2_temp.as_advanced_builder().build();

        node0.rpc_client().send_transaction(tx1.data().into());
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert_eq!(get_tx_pool_conflicts(node0), vec![]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfOnlyForResolveDead;
impl Spec for RbfOnlyForResolveDead {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);

        let tx_hash_0 = node0.generate_transaction();

        let tx1 = node0.new_transaction(tx_hash_0);

        // This is an unknown input
        let tx_hash_1 = Byte32::zero();
        let tx2 = tx1
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(tx_hash_1, 0))
                    .build()
            }])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        let message = res.err().unwrap().to_string();
        assert!(message.contains("TransactionFailedToResolve: Resolve failed Unknown"));
        assert_eq!(get_tx_pool_conflicts(node0), vec![]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfSameInputwithLessFee;

// RBF Rule #3, #4
impl Spec for RbfSameInputwithLessFee {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0);

        let output1 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();

        let tx1 = tx1.as_advanced_builder().set_outputs(vec![output1]).build();

        // Set tx2 fee to a lower value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(90).pack())
            .build();

        let tx2 = tx2_temp
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        node0.rpc_client().send_transaction(tx1.data().into());
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        let message = res.err().unwrap().to_string();
        assert!(message.contains(
            "Tx's current fee is 1000000000, expect it to >= 2000000363 to replace old txs"
        ));
        assert_eq!(get_tx_pool_conflicts(node0), vec![tx2.hash().unpack()]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfTooManyDescendants;

// RBF Rule #5
impl Spec for RbfTooManyDescendants {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let tx0_temp = tx0.clone();
        let mut txs = vec![tx0];
        let max_count = 101;
        while txs.len() <= max_count {
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
        assert_eq!(txs.len(), max_count + 1);
        // send tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = tx0_temp
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("Tx conflict with too many txs"));
        assert_eq!(get_tx_pool_conflicts(node0), vec![tx2.hash().unpack()]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfContainNewTx;

// RBF Rule #2
impl Spec for RbfContainNewTx {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
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
        assert_eq!(txs.len(), max_count + 1);
        // send tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[1].hash(), 0))
                        .build()
                },
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[4].hash(), 0))
                        .build()
                },
            ])
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("new Tx contains unconfirmed inputs"));
        assert_eq!(get_tx_pool_conflicts(node0), vec![tx2.hash().unpack()]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfContainInvalidInput;

// RBF Rule #2
impl Spec for RbfContainInvalidInput {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
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
        assert_eq!(txs.len(), max_count + 1);
        // send Tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx2 = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[1].hash(), 0))
                        .build()
                },
                {
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(txs[3].hash(), 0))
                        .build()
                },
            ])
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("new Tx contains inputs in descendants of to be replaced Tx"));
        assert_eq!(get_tx_pool_conflicts(node0), vec![tx2.hash().unpack()]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfChildPayForParent;

// RBF Rule #2
impl Spec for RbfChildPayForParent {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;

        let output5 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(50).pack())
            .build();

        while txs.len() <= max_count {
            let parent = txs.last().unwrap();
            // we set tx5's fee to higher, so tx5 will pay for tx1
            let output = if txs.len() == max_count - 1 {
                output5.clone()
            } else {
                parent.output(0).unwrap()
            };
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![output])
                .build();
            txs.push(child);
        }
        assert_eq!(txs.len(), max_count + 1);
        // send Tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value, but not enough to pay for tx4
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let new_tx = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(txs[1].hash(), 0))
                    .build()
            }])
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(new_tx.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("RBF rejected: Tx's current fee is 3000000000, expect it to >= 5000000363 to replace old txs"));
        assert_eq!(get_tx_pool_conflicts(node0), vec![new_tx.hash().unpack()]);

        // let's try a new transaction with new higher fee
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(45).pack())
            .build();
        let new_tx_ok = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(txs[1].hash(), 0))
                    .build()
            }])
            .set_outputs(vec![output2])
            .build();
        let res = node0
            .rpc_client()
            .send_transaction_result(new_tx_ok.data().into());
        assert!(res.is_ok());

        // replaced txs are in conflicts pool
        // tx2 tx3 tx4 is replaced, old `new_tx` is still in conflicts pool
        let mut expected: Vec<ckb_types::H256> = txs[2..=max_count - 1]
            .iter()
            .map(|tx| tx.hash().unpack())
            .collect::<Vec<_>>();
        expected.push(new_tx.hash().unpack());
        expected.sort_unstable();
        let conflicts = get_tx_pool_conflicts(node0);
        assert_eq!(conflicts, expected);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfContainInvalidCells;

// RBF Rule, contains cell from conflicts txs
impl Spec for RbfContainInvalidCells {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        let cells = gen_spendable(node0, 3);
        let txs = always_success_transactions(node0, &cells);
        for tx in txs.iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let clone_tx = txs[2].clone();

        let cell = CellDep::new_builder()
            .out_point(OutPoint::new(txs[1].hash(), 0))
            .build();

        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();
        let tx2 = clone_tx
            .as_advanced_builder()
            .set_inputs(vec![{
                CellInput::new_builder()
                    .previous_output(OutPoint::new(txs[1].hash(), 0))
                    .build()
            }])
            .set_cell_deps(vec![cell])
            .set_outputs(vec![output2])
            .build();

        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
        // script verification failed because of invalid cell dep, will not in conflicts pool
        assert_eq!(get_tx_pool_conflicts(node0), vec![]);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfRejectReplaceProposed;

// RBF Rule #6
// We removed rule #6, even tx in `Gap` and `Proposed` status can be replaced.
impl Spec for RbfRejectReplaceProposed {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
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
        assert_eq!(txs.len(), max_count + 1);
        // send Tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let proposed = node0.mine_with_blocking(|template| template.proposals.len() != max_count);
        let ret = node0.rpc_client().get_transaction(txs[2].hash());
        assert!(
            matches!(ret.tx_status.status, Status::Pending),
            "tx1 should be pending"
        );

        node0.mine_with_blocking(|template| template.number.value() != (proposed + 1));

        let rpc_client0 = node0.rpc_client();
        let ret = wait_until(20, || {
            let res = rpc_client0.get_transaction(txs[2].hash());
            res.tx_status.status == Status::Proposed
        });
        assert!(ret, "tx1 should be proposed");

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx1_hash = txs[2].hash();
        let tx2 = clone_tx
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        // begin to RBF
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_ok());

        let old_tx_status = node0.rpc_client().get_transaction(tx1_hash).tx_status;
        assert_eq!(old_tx_status.status, Status::Rejected);
        assert!(old_tx_status.reason.unwrap().contains("RBFRejected"));

        let tx2_status = node0.rpc_client().get_transaction(tx2.hash()).tx_status;
        assert_eq!(tx2_status.status, Status::Pending);

        let window_count = node0.consensus().tx_proposal_window().closest();
        node0.mine(window_count);
        // since old tx is already in BlockAssembler,
        // tx1 will be committed, even it is not in tx_pool and with `Rejected` status now
        let ret = wait_until(20, || {
            let res = rpc_client0.get_transaction(txs[2].hash());
            res.tx_status.status == Status::Committed
        });
        assert!(ret, "tx1 should be committed");
        let tx1_status = node0.rpc_client().get_transaction(txs[2].hash()).tx_status;
        assert_eq!(tx1_status.status, Status::Committed);

        // tx2 will be marked as `Rejected` because callback of `remove_committed_txs` from tx1
        let tx2_status = node0.rpc_client().get_transaction(tx2.hash()).tx_status;
        assert_eq!(tx2_status.status, Status::Rejected);

        // the same tx2 can not be sent again
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");

        // resolve tx2 failed with `unknown` when resolve inputs used by tx1
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("TransactionFailedToResolve: Resolve failed Unknown"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfReplaceProposedSuccess;

// RBF Rule #6
// We removed rule #6, this spec testing that we can replace tx in `Gap` and `Proposed` successfully.
impl Spec for RbfReplaceProposedSuccess {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        // build txs chain
        let tx0 = node0.new_transaction_spend_tip_cellbase();
        let mut txs = vec![tx0];
        let max_count = 5;
        while txs.len() <= max_count {
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
        assert_eq!(txs.len(), max_count + 1);
        // send Tx chain
        for tx in txs[..=max_count - 1].iter() {
            let ret = node0.rpc_client().send_transaction_result(tx.data().into());
            assert!(ret.is_ok());
        }

        let proposed = node0.mine_with_blocking(|template| template.proposals.len() != max_count);
        let ret = node0.rpc_client().get_transaction(txs[2].hash());
        assert!(
            matches!(ret.tx_status.status, Status::Pending),
            "tx1 should be pending"
        );

        node0.mine_with_blocking(|template| template.number.value() != (proposed + 1));

        let rpc_client0 = node0.rpc_client();
        let ret = wait_until(20, || {
            let res = rpc_client0.get_transaction(txs[2].hash());
            res.tx_status.status == Status::Proposed
        });
        assert!(ret, "tx1 should be proposed");

        let clone_tx = txs[2].clone();
        // Set tx2 fee to a higher value
        let output2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(70).pack())
            .build();

        let tx1_hash = txs[2].hash();
        let tx2 = clone_tx
            .as_advanced_builder()
            .set_outputs(vec![output2])
            .build();

        // begin to RBF
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_ok());

        let old_tx_status = node0.rpc_client().get_transaction(tx1_hash).tx_status;
        assert_eq!(old_tx_status.status, Status::Rejected);
        assert!(old_tx_status.reason.unwrap().contains("RBFRejected"));

        let tx2_status = node0.rpc_client().get_transaction(tx2.hash()).tx_status;
        assert_eq!(tx2_status.status, Status::Pending);

        // submit a blank block
        let example = node0.new_block(None, None, None);
        let blank_block = example
            .as_advanced_builder()
            .set_proposals(vec![])
            .set_transactions(vec![example.transaction(0).unwrap()])
            .build();
        node0.submit_block(&blank_block);

        wait_until(10, move || node0.get_tip_block() == blank_block);

        let window_count = node0.consensus().tx_proposal_window().closest();
        node0.mine(window_count);

        let ret = wait_until(20, || {
            let res = rpc_client0.get_transaction(tx2.hash());
            res.tx_status.status == Status::Proposed
        });
        assert!(ret, "tx2 should be proposed");
        let tx1_status = node0.rpc_client().get_transaction(txs[2].hash()).tx_status;
        assert_eq!(tx1_status.status, Status::Rejected);

        let window_count = node0.consensus().tx_proposal_window().closest();
        node0.mine(window_count);
        // since old tx is already in BlockAssembler,
        // tx1 will be committed, even it is not in tx_pool and with `Rejected` status now
        let ret = wait_until(20, || {
            let res = rpc_client0.get_transaction(tx2.hash());
            res.tx_status.status == Status::Committed
        });
        assert!(ret, "tx2 should be committed");

        // the same tx2 can not be sent again
        let res = node0
            .rpc_client()
            .send_transaction_result(tx2.data().into());
        assert!(res.is_err(), "tx2 should be rejected");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfConcurrency;
impl Spec for RbfConcurrency {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 4 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());

        let mut conflicts = vec![tx1];
        // tx1 capacity is 100, set other txs to higher fee
        let fees = vec![
            capacity_bytes!(83),
            capacity_bytes!(82),
            capacity_bytes!(81),
            capacity_bytes!(80),
        ];
        for fee in fees.iter() {
            let tx2_temp = node0.new_transaction(tx_hash_0.clone());
            let output = CellOutputBuilder::default().capacity(fee.pack()).build();

            let tx2 = tx2_temp
                .as_advanced_builder()
                .set_outputs(vec![output])
                .build();
            conflicts.push(tx2);
        }

        // make 5 threads to set_transaction concurrently
        let mut handles = vec![];
        for tx in &conflicts {
            let cur_tx = tx.clone();
            let rpc_address = node0.rpc_listen();
            let handle = std::thread::spawn(move || {
                let rpc_client = RpcClient::new(&rpc_address);
                let _ = rpc_client.send_transaction_result(cur_tx.data().into());
            });
            handles.push(handle);
        }
        for handle in handles {
            let _ = handle.join();
        }

        let status: Vec<_> = conflicts
            .iter()
            .map(|tx| {
                let res = node0.rpc_client().get_transaction(tx.hash());
                res.tx_status.status
            })
            .collect();

        // the last tx should be in Pending(with the highest fee), others should be in Rejected
        assert_eq!(status[4], Status::Pending);
        for s in status.iter().take(4) {
            assert_eq!(*s, Status::Rejected);
        }
        let mut expected_conflicts: Vec<H256> = conflicts
            .iter()
            .take(4)
            .map(|tx| tx.hash().unpack())
            .collect();
        expected_conflicts.sort_unstable();
        assert_eq!(get_tx_pool_conflicts(node0), expected_conflicts);
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}

pub struct RbfCellDepsCheck;
impl Spec for RbfCellDepsCheck {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        let initial_inputs = gen_spendable(node0, 2);
        let input_a = &initial_inputs[0];
        let input_c = &initial_inputs[1];

        // Commit transaction root
        let tx_a = {
            let tx_a = always_success_transaction(node0, input_a);
            node0.submit_transaction(&tx_a);
            tx_a
        };

        let mut prev = tx_a.clone();
        // Create transaction chain
        for _i in 0..2 {
            let input =
                CellMetaBuilder::from_cell_output(prev.output(0).unwrap(), Default::default())
                    .out_point(OutPoint::new(prev.hash(), 0))
                    .build();
            let cur = always_success_transaction(node0, &input);
            let _ = node0.rpc_client().send_transaction(cur.data().into());
            prev = cur.clone();
        }

        // Create a child transaction with celldep
        let tx = always_success_transaction(node0, input_c);
        let cell_dep_to_last = CellDepBuilder::default()
            .dep_type(DepType::Code.into())
            .out_point(OutPoint::new(prev.hash(), 0))
            .build();
        let tx_c = tx
            .as_advanced_builder()
            .cell_dep(cell_dep_to_last.clone())
            .build();
        let res = node0
            .rpc_client()
            .send_transaction_result(tx_c.data().into());
        assert!(res.is_ok());

        // Create a new transaction for cell dep with high fee
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(80).pack())
            .build();
        let new_tx = tx_a
            .as_advanced_builder()
            .set_outputs(vec![output])
            .cell_dep(cell_dep_to_last)
            .build();

        let res = node0.submit_transaction_with_result(&new_tx);
        assert!(res
            .err()
            .unwrap()
            .to_string()
            .contains("new Tx contains cell deps from conflicts"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_rbf_rate = ckb_types::core::FeeRate(1500);
    }
}
