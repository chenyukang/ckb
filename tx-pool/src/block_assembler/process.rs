use crate::service::{BlockAssemblerMessage, TxPoolService};
use std::sync::Arc;
use ckb_logger::debug;

pub(crate) async fn process(service: TxPoolService, message: &BlockAssemblerMessage) {
    match message {
        BlockAssemblerMessage::Pending => {
            if let Some(ref block_assembler) = service.block_assembler {
                debug!("block_assembler update_proposals ...................");
                block_assembler.update_proposals(&service.tx_pool).await;
            }
        }
        BlockAssemblerMessage::Proposed => {
            if let Some(ref block_assembler) = service.block_assembler {
                if let Err(e) = block_assembler.update_transactions(&service.tx_pool).await {
                    debug!("block_assembler update_transactions ...................");
                    ckb_logger::error!("block_assembler update_transactions error {}", e);
                }
            }
        }
        BlockAssemblerMessage::Uncle => {
            if let Some(ref block_assembler) = service.block_assembler {
                block_assembler.update_uncles().await;
            }
        }
        BlockAssemblerMessage::Reset(snapshot) => {
            if let Some(ref block_assembler) = service.block_assembler {
                if let Err(e) = block_assembler.update_blank(Arc::clone(snapshot)).await {
                    ckb_logger::error!("block_assembler update_blank error {}", e);
                }
            }
        }
    }
}
