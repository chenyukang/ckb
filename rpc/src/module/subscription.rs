use async_trait::async_trait;
use ckb_jsonrpc_types::Topic;
use ckb_notify::NotifyController;

use jsonrpc_core::{MetaIoHandler, Metadata, Params};
use jsonrpc_utils::pub_sub::{add_pub_sub, PubSub};
// use jsonrpc_pubsub::{
//     typed::{Sink, Subscriber},
//     SubscriptionId,
// };
use jsonrpc_utils::{pub_sub::PublishMsg, rpc};

use futures_util::{stream::BoxStream, Stream};
use jsonrpc_utils::{axum_utils::handle_jsonrpc, pub_sub::Session};
use tokio::sync::broadcast::Sender;

use core::result::Result;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    RwLock,
};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

const SUBSCRIBER_NAME: &str = "TcpSubscription";

// #[derive(Clone)]
// pub struct SubscriptionSession {
//     pub(crate) subscription_ids: Arc<RwLock<HashSet<SubscriptionId>>>,
//     pub(crate) session: Arc<Session>,
// }

// impl SubscriptionSession {
//     pub fn new(session: Session) -> Self {
//         Self {
//             subscription_ids: Arc::new(RwLock::new(HashSet::new())),
//             session: Arc::new(session),
//         }
//     }
// }

// impl Metadata for SubscriptionSession {}

/// RPC Module Subscription that CKB node will push new messages to subscribers.
///
/// RPC subscriptions require a full duplex connection. CKB offers such connections in the form of
/// TCP (enable with rpc.tcp_listen_address configuration option) and WebSocket (enable with
/// rpc.ws_listen_address).
///
/// ## Examples
///
/// TCP RPC subscription:
///
/// ```text
/// telnet localhost 18114
/// > {"id": 2, "jsonrpc": "2.0", "method": "subscribe", "params": ["new_tip_header"]}
/// < {"jsonrpc":"2.0","result":"0x0","id":2}
/// < {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
///"subscription":0}}
/// < {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
///"subscription":0}}
/// < ...
/// > {"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": ["0x0"]}
/// < {"jsonrpc":"2.0","result":true,"id":2}
/// ```
///
/// WebSocket RPC subscription:
///
/// ```javascript
/// let socket = new WebSocket("ws://localhost:28114")
///
/// socket.onmessage = function(event) {
///   console.log(`Data received from server: ${event.data}`);
/// }
///
/// socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "subscribe", "params": ["new_tip_header"]}`)
///
/// socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": ["0x0"]}`)
/// ```
#[allow(clippy::needless_return)]
#[rpc]
#[async_trait]
pub trait SubscriptionRpc {
    /// Context to implement the subscription RPC.
    /// type Metadata;

    /// Subscribes to a topic.
    ///
    /// ## Params
    ///
    /// * `topic` - Subscription topic (enum: new_tip_header | new_tip_block | new_transaction | proposed_transaction | rejected_transaction)
    ///
    /// ## Returns
    ///
    /// This RPC returns the subscription ID as the result. CKB node will push messages in the subscribed
    /// topics to the current RPC connection. The subscript ID is also attached as
    /// `params.subscription` in the push messages.
    ///
    /// Example push message:
    ///
    /// ```json+skip
    /// {
    ///   "jsonrpc": "2.0",
    ///   "method": "subscribe",
    ///   "params": {
    ///     "result": { ... },
    ///     "subscription": "0x2a"
    ///   }
    /// }
    /// ```
    ///
    /// ## Topics
    ///
    /// ### `new_tip_header`
    ///
    /// Whenever there's a block that is appended to the canonical chain, the CKB node will publish the
    /// block header to subscribers.
    ///
    /// The type of the `params.result` in the push message is [`HeaderView`](../../ckb_jsonrpc_types/struct.HeaderView.html).
    ///
    /// ### `new_tip_block`
    ///
    /// Whenever there's a block that is appended to the canonical chain, the CKB node will publish the
    /// whole block to subscribers.
    ///
    /// The type of the `params.result` in the push message is [`BlockView`](../../ckb_jsonrpc_types/struct.BlockView.html).
    ///
    /// ### `new_transaction`
    ///
    /// Subscribers will get notified when a new transaction is submitted to the pool.
    ///
    /// The type of the `params.result` in the push message is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html).
    ///
    /// ### `proposed_transaction`
    ///
    /// Subscribers will get notified when an in-pool transaction is proposed by chain.
    ///
    /// The type of the `params.result` in the push message is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html).
    ///
    /// ### `rejected_transaction`
    ///
    /// Subscribers will get notified when a pending transaction is rejected by tx-pool.
    ///
    /// The type of the `params.result` in the push message is an array contain:
    ///
    /// The type of the `params.result` in the push message is a two-elements array, where
    ///
    /// -   the first item type is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html), and
    /// -   the second item type is [`PoolTransactionReject`](../../ckb_jsonrpc_types/struct.PoolTransactionReject.html).
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "subscribe",
    ///   "params": [
    ///     "new_tip_header"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x2a"
    /// }
    /// ```
    //#[pubsub(subscription = "subscribe", subscribe, name = "subscribe")]
    type S: Stream<Item = PublishMsg<String>> + Send + 'static;
    #[rpc(pub_sub(notify = "subscribe", unsubscribe = "unsubscribe"))]
    fn subscribe(&self, topic: Topic);

    /*
    /// Unsubscribes from a subscribed topic.
    ///
    /// ## Params
    ///
    /// * `id` - Subscription ID
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "unsubscribe",
    ///   "params": [
    ///     "0x2a"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": true
    /// }
    /// ```
    /// #[pubsub(subscription = "subscribe", unsubscribe, name = "unsubscribe")]
    /// fn unsubscribe(&self, meta: Option<Self::Metadata>, id: SubscriptionId) -> Result<bool>;
     */
}

type SubscriptionId = u64;
type Subscribers = HashSet<SubscriptionId>;
#[derive(Clone)]
pub struct SubscriptionRpcImpl {
    pub(crate) new_tip_header_sender: Sender<PublishMsg<String>>,
    pub(crate) new_tip_block_sender: Sender<PublishMsg<String>>,
    pub(crate) new_transaction_sender: Sender<PublishMsg<String>>,
    pub(crate) io_handler: Arc<MetaIoHandler<Option<Session>>>,
}

impl SubscriptionRpcImpl {
    fn subscribe(&mut self, topic: Topic) -> String {
        let tx = self.new_tip_block_sender.clone();
        add_pub_sub(
            &mut self.io_handler,
            "subscribe",
            "subscription",
            "unsubscribe",
            move |_params: Params| {
                Ok(BroadcastStream::new(tx.subscribe()).map(|result| {
                    result.unwrap_or_else(|_| {
                        PublishMsg::error(&jsonrpc_core::Error {
                            code: jsonrpc_core::ErrorCode::ServerError(-32000),
                            message: "lagged".into(),
                            data: None,
                        })
                    })
                }))
            },
        );
        "ok".to_owned()
    }
}

impl SubscriptionRpcImpl {
    pub fn new(
        notify_controller: NotifyController,
        handle: Handle,
        io_handle: Arc<MetaIoHandler<Option<Session>>>,
    ) -> Self {
        let mut new_block_receiver =
            handle.block_on(notify_controller.subscribe_new_block(SUBSCRIBER_NAME.to_string()));
        let mut new_transaction_receiver = handle
            .block_on(notify_controller.subscribe_new_transaction(SUBSCRIBER_NAME.to_string()));
        let mut proposed_transaction_receiver = handle.block_on(
            notify_controller.subscribe_proposed_transaction(SUBSCRIBER_NAME.to_string()),
        );
        let mut reject_transaction_receiver = handle
            .block_on(notify_controller.subscribe_reject_transaction(SUBSCRIBER_NAME.to_string()));

        let (new_tip_header_sender, _) = broadcast::channel(10);
        let (new_tip_block_sender, _) = broadcast::channel(10);
        let (new_transaction_sender, _) = broadcast::channel(10);
        let subscription_rpc_impl = SubscriptionRpcImpl {
            new_tip_header_sender,
            new_tip_block_sender,
            new_transaction_sender,
            io_handler: Arc::clone(&io_handle),
        };
        handle.spawn({
            async move {
            loop {
                tokio::select! {
                    Some(block) = new_block_receiver.recv() => {
                        let block: ckb_jsonrpc_types::BlockView  = block.into();
                        let json_string = serde_json::to_string(&block).expect("serialization should be ok");
                        drop(new_tip_header_sender.send(PublishMsg::result(&json_string)));
                    },
                    Some(tx_entry) = new_transaction_receiver.recv() => {
                        let entry: ckb_jsonrpc_types::PoolTransactionEntry = tx_entry.into();
                        let json_string = serde_json::to_string(&entry).expect("serialization should be ok");
                        drop(new_transaction_sender.send(PublishMsg::result(&json_string)));
                    },
                    Some(tx_entry) = proposed_transaction_receiver.recv() => {
                        let entry: ckb_jsonrpc_types::PoolTransactionEntry = tx_entry.into();
                        let json_string = serde_json::to_string(&entry).expect("serialization should be ok");
                        drop(new_transaction_sender.send(PublishMsg::result(&json_string)));
                    },
                    Some((tx_entry, reject)) = reject_transaction_receiver.recv() => {
                        let entry: ckb_jsonrpc_types::PoolTransactionEntry = tx_entry.into();
                        let reject: ckb_jsonrpc_types::PoolTransactionReject = reject.into();
                        let json_string = serde_json::to_string(&(entry, reject)).expect("serialization should be ok");
                        drop(new_transaction_sender.send(PublishMsg::result(&json_string)));
                    }
                    else => break,
                }
            }
        }});

        subscription_rpc_impl
    }
}
