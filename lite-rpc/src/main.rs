#![allow(clippy::arithmetic_side_effects)]
#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
use jemallocator::Jemalloc;

#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use {
    crate::{cli::build_cli, config::LiteRpcConfig, node::LiteNode},
    log::{error, info},
    std::process::exit,
};

mod cli;
mod config;
pub mod latency_tracker;
mod node;

pub fn main() {
    let solana_version = solana_version::version!();
    let cli_app = build_cli(solana_version);
    let matches = cli_app.get_matches();

    let config = match LiteRpcConfig::from_matches(&matches) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            exit(1);
        }
    };

    // Initialize logger
    let _logger_thread = agave_logger::setup_with_default_filter();
    solana_metrics::set_panic_hook("lite-rpc", None);

    info!(
        "agave-lite-rpc {} starting",
        solana_version
    );

    match LiteNode::start(config) {
        Ok(node) => {
            info!("Lite RPC node started successfully");
            node.join();
        }
        Err(e) => {
            error!("Failed to start lite-rpc node: {e:?}");
            exit(1);
        }
    }
}
