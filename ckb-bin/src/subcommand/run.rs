use std::path::PathBuf;

use crate::{helper::deadlock_detection, subcommand::check_process};
use ckb_app_config::{ExitCode, RunArgs};
use ckb_async_runtime::{new_global_runtime, Handle};
use ckb_build_info::Version;
use ckb_launcher::Launcher;
use ckb_logger::info;
use ckb_stop_handler::{broadcast_exit_signals, wait_all_ckb_services_exit};

use ckb_types::core::cell::setup_system_cell_cache;
use colored::Colorize;
use daemonize::Daemonize;

pub fn run(args: RunArgs, version: Version, async_handle: Handle) -> Result<(), ExitCode> {
    if let Some(daemon_path) = args.daemon_path.clone() {
        run_app_in_daemon(args, version, daemon_path)
    } else {
        run_app(args, version, async_handle)
    }
}

fn run_app_in_daemon(
    args: RunArgs,
    version: Version,
    daemon_path: PathBuf,
) -> Result<(), ExitCode> {
    eprintln!("starting CKB in daemon mode ...");
    eprintln!("check status : `{}`", "ckb daemon --check".green());
    eprintln!("stop daemon  : `{}`", "ckb daemon --stop".yellow());

    // make sure daemon dir exists
    std::fs::create_dir_all(&daemon_path)?;
    let pid_file = daemon_path.join("ckb-run.pid");

    eprintln!("now daemon path: {}", daemon_path.display());
    if check_process(&pid_file).is_ok() {
        eprintln!("{}", "ckb is already running".red());
        return Ok(());
    }
    eprintln!("no ckb process, starting ...");

    let pwd = std::env::current_dir()?;
    let daemon = Daemonize::new()
        .pid_file(pid_file.clone())
        .chown_pid_file(true)
        .working_directory(pwd);

    eprintln!("now begin to start: {:?}", pid_file);
    match daemon.start() {
        Ok(_) => {
            eprintln!("Success, daemonized ...");
            let (async_handle, _handle_stop_rx, _runtime) = new_global_runtime();
            eprintln!("now ........");
            run_app(args, version, async_handle.clone())
        }
        Err(e) => {
            info!("daemonize error: {}", e);
            Err(ExitCode::Failure)
        }
    }
}

fn run_app(args: RunArgs, version: Version, async_handle: Handle) -> Result<(), ExitCode> {
    deadlock_detection();

    info!("ckb version: {}", version);

    let mut launcher = Launcher::new(args, version, async_handle);

    let block_assembler_config = launcher.sanitize_block_assembler_config()?;
    let miner_enable = block_assembler_config.is_some();

    let (shared, mut pack) = launcher.build_shared(block_assembler_config)?;

    // spawn freezer background process
    let _freezer = shared.spawn_freeze();

    setup_system_cell_cache(
        shared.consensus().genesis_block(),
        shared.snapshot().as_ref(),
    )
    .expect("SYSTEM_CELL cache init once");

    rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("RayonGlobal-{i}"))
        .build_global()
        .expect("Init the global thread pool for rayon failed");

    ckb_memory_tracker::track_current_process(
        launcher.args.config.memory_tracker.interval,
        Some(shared.store().db().inner()),
    );

    launcher.check_assume_valid_target(&shared);

    let chain_controller = launcher.start_chain_service(&shared, pack.take_proposal_table());

    launcher.start_block_filter(&shared);

    let (network_controller, _rpc_server) = launcher.start_network_and_rpc(
        &shared,
        chain_controller.clone(),
        miner_enable,
        pack.take_relay_tx_receiver(),
    );

    let tx_pool_builder = pack.take_tx_pool_builder();
    tx_pool_builder.start(network_controller.clone());

    info!("CKB service started ...");
    ctrlc::set_handler(|| {
        info!("Trapped exit signal, exiting...");
        broadcast_exit_signals();
    })
    .expect("Error setting Ctrl-C handler");

    wait_all_ckb_services_exit();

    Ok(())
}
