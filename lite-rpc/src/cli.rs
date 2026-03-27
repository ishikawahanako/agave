use clap::{App, Arg};

pub fn build_cli<'a>(version: &'a str) -> App<'a, 'a> {
    App::new("agave-lite-rpc")
        .about("Minimal shred-ingesting Geyser + JSON-RPC server for Solana")
        .version(version)
        .arg(
            Arg::with_name("identity")
                .short("i")
                .long("identity")
                .value_name("KEYPAIR")
                .takes_value(true)
                .help("Validator identity keypair"),
        )
        .arg(
            Arg::with_name("ledger_path")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .help("Use DIR as ledger location"),
        )
        .arg(
            Arg::with_name("entrypoint")
                .short("e")
                .long("entrypoint")
                .value_name("HOST:PORT")
                .takes_value(true)
                .multiple(true)
                .help("Cluster entrypoint (repeatable)"),
        )
        .arg(
            Arg::with_name("expected_genesis_hash")
                .long("expected-genesis-hash")
                .value_name("HASH")
                .takes_value(true)
                .help("Require the genesis have this hash"),
        )
        .arg(
            Arg::with_name("expected_shred_version")
                .long("expected-shred-version")
                .value_name("VERSION")
                .takes_value(true)
                .help("Require the shred version be this value"),
        )
        .arg(
            Arg::with_name("rpc_port")
                .long("rpc-port")
                .value_name("PORT")
                .takes_value(true)
                .default_value("8899")
                .help("JSON-RPC port"),
        )
        .arg(
            Arg::with_name("rpc_bind_address")
                .long("rpc-bind-address")
                .value_name("HOST")
                .takes_value(true)
                .default_value("127.0.0.1")
                .help("IP address to bind the RPC port"),
        )
        .arg(
            Arg::with_name("private_rpc")
                .long("private-rpc")
                .takes_value(false)
                .help("Do not advertise the RPC port in gossip"),
        )
        .arg(
            Arg::with_name("full_rpc_api")
                .long("full-rpc-api")
                .takes_value(false)
                .help("Enable full JSON-RPC API plus WebSocket PubSub"),
        )
        .arg(
            Arg::with_name("enable_rpc_transaction_history")
                .long("enable-rpc-transaction-history")
                .takes_value(false)
                .help("Enable historical transaction info over JSON RPC"),
        )
        .arg(
            Arg::with_name("geyser_plugin_config")
                .long("geyser-plugin-config")
                .value_name("FILE")
                .takes_value(true)
                .multiple(true)
                .help("Geyser plugin config file (repeatable)"),
        )
        .arg(
            Arg::with_name("account_paths")
                .long("account-paths")
                .value_name("DIR")
                .takes_value(true)
                .multiple(true)
                .help("Account storage directory (repeatable)"),
        )
        .arg(
            Arg::with_name("snapshots")
                .long("snapshots")
                .value_name("DIR")
                .takes_value(true)
                .help("Snapshot storage directory"),
        )
        .arg(
            Arg::with_name("limit_ledger_size")
                .long("limit-ledger-size")
                .value_name("SHREDS")
                .takes_value(true)
                .default_value("200000000")
                .help("Maximum number of shreds to keep in the ledger"),
        )
        .arg(
            Arg::with_name("gossip_port")
                .long("gossip-port")
                .value_name("PORT")
                .takes_value(true)
                .help("Gossip port number"),
        )
        .arg(
            Arg::with_name("dynamic_port_range")
                .long("dynamic-port-range")
                .value_name("MIN-MAX")
                .takes_value(true)
                .default_value("8000-10000")
                .help("Range to use for dynamically assigned ports"),
        )
        .arg(
            Arg::with_name("bind_address")
                .long("bind-address")
                .value_name("HOST")
                .takes_value(true)
                .default_value("0.0.0.0")
                .help("IP address to bind all listening ports"),
        )
        .arg(
            Arg::with_name("repair_validators")
                .long("repair-validators")
                .value_name("PUBKEY")
                .takes_value(true)
                .multiple(true)
                .help("Only request repairs from these validators (repeatable)"),
        )
        .arg(
            Arg::with_name("gossip_validators")
                .long("gossip-validators")
                .value_name("PUBKEY")
                .takes_value(true)
                .multiple(true)
                .help("Only gossip with these validators (repeatable)"),
        )
        .arg(
            Arg::with_name("log")
                .long("log")
                .value_name("FILE")
                .takes_value(true)
                .help("Log file path"),
        )
        .arg(
            Arg::with_name("replay_forks_threads")
                .long("replay-forks-threads")
                .value_name("N")
                .takes_value(true)
                .default_value("1")
                .help("Number of threads for replay fork processing"),
        )
        .arg(
            Arg::with_name("replay_transactions_threads")
                .long("replay-transactions-threads")
                .value_name("N")
                .takes_value(true)
                .default_value("1")
                .help("Number of threads for replaying transactions"),
        )
        .arg(
            Arg::with_name("tvu_receive_threads")
                .long("tvu-receive-threads")
                .value_name("N")
                .takes_value(true)
                .default_value("2")
                .help("Number of TVU receive threads"),
        )
        .arg(
            Arg::with_name("tvu_sigverify_threads")
                .long("tvu-sigverify-threads")
                .value_name("N")
                .takes_value(true)
                .default_value("2")
                .help("Number of TVU shred signature verification threads"),
        )
}
