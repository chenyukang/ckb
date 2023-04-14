use ckb_jsonrpc_types::{ExtraLoggerConfig, MainLoggerConfig};
use ckb_logger_service::Logger;
use jsonrpsee::core::RpcResult;
use jsonrpsee::proc_macros::rpc;
use jsonrpsee_types::error::ErrorCode::InternalError;
use jsonrpsee_types::error::ErrorObject;
use std::time;

/// RPC Module Debug for internal RPC methods.
///
/// **This module is for CKB developers and will not guarantee compatibility.** The methods here
/// will be changed or removed without advanced notification.
#[rpc(server)]
#[doc(hidden)]
pub trait DebugRpc {
    /// Dumps jemalloc memory profiling information into a file.
    ///
    /// The file is stored in the server running the CKB node.
    ///
    /// The RPC returns the path to the dumped file on success or returns an error on failure.
    #[method(name = "jemalloc_profiling_dump")]
    fn jemalloc_profiling_dump(&self) -> RpcResult<String>;
    /// Changes main logger config options while CKB is running.
    #[method(name = "update_main_logger")]
    fn update_main_logger(&self, config: MainLoggerConfig) -> RpcResult<()>;
    /// Sets logger config options for extra loggers.
    ///
    /// CKB nodes allow setting up extra loggers. These loggers will have their own log files and
    /// they only append logs to their log files.
    ///
    /// ## Params
    ///
    /// * `name` - Extra logger name
    /// * `config_opt` - Adds a new logger or update an existing logger when this is not null.
    /// Removes the logger when this is null.
    #[method(name = "set_extra_logger")]
    fn set_extra_logger(
        &self,
        name: String,
        config_opt: Option<ExtraLoggerConfig>,
    ) -> RpcResult<()>;
}

pub(crate) struct DebugRpcImpl {}

impl DebugRpcServer for DebugRpcImpl {
    fn jemalloc_profiling_dump(&self) -> RpcResult<String> {
        let timestamp = time::SystemTime::now()
            .duration_since(time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let filename = format!("ckb-jeprof.{timestamp}.heap");
        match ckb_memory_tracker::jemalloc_profiling_dump(&filename) {
            Ok(()) => Ok(filename),
            Err(err) => Err(ErrorObject {
                code: InternalError,
                message: err,
                data: None,
            }),
        }
    }

    fn update_main_logger(&self, config: MainLoggerConfig) -> RpcResult<()> {
        let MainLoggerConfig {
            filter,
            to_stdout,
            to_file,
            color,
        } = config;
        if filter.is_none() && to_stdout.is_none() && to_file.is_none() && color.is_none() {
            return Ok(());
        }
        Logger::update_main_logger(filter, to_stdout, to_file, color).map_err(|err| ErrorObject {
            code: InternalError,
            message: err,
            data: None,
        })
    }

    fn set_extra_logger(
        &self,
        name: String,
        config_opt: Option<ExtraLoggerConfig>,
    ) -> RpcResult<()> {
        if let Err(err) = Logger::check_extra_logger_name(&name) {
            return Err(ErrorObject {
                code: InternalError,
                message: err,
                data: None,
            });
        }
        if let Some(config) = config_opt {
            Logger::update_extra_logger(name, config.filter)
        } else {
            Logger::remove_extra_logger(name)
        }
        .map_err(|err| ErrorObject {
            code: InternalError,
            message: err,
            data: None,
        })
    }
}
