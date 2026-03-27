use {
    clap::ArgMatches,
    solana_gossip::contact_info::ContactInfo,
    solana_hash::Hash,
    solana_keypair::Keypair,
    solana_net_utils::parse_port_range,
    solana_pubkey::Pubkey,
    std::{
        collections::HashSet,
        net::{IpAddr, SocketAddr},
        num::NonZeroUsize,
        path::PathBuf,
        str::FromStr,
    },
};

pub struct LiteRpcConfig {
    pub identity_keypair: Keypair,
    pub ledger_path: PathBuf,
    pub entrypoints: Vec<ContactInfo>,
    pub expected_genesis_hash: Option<Hash>,
    pub expected_shred_version: Option<u16>,
    pub rpc_port: u16,
    pub rpc_bind_address: IpAddr,
    pub private_rpc: bool,
    pub full_rpc_api: bool,
    pub enable_rpc_transaction_history: bool,
    pub geyser_plugin_config_files: Option<Vec<PathBuf>>,
    pub account_paths: Vec<PathBuf>,
    pub snapshot_path: PathBuf,
    pub limit_ledger_size: Option<u64>,
    pub gossip_port: u16,
    pub dynamic_port_range: (u16, u16),
    pub bind_address: IpAddr,
    pub repair_validators: Option<HashSet<Pubkey>>,
    pub gossip_validators: Option<HashSet<Pubkey>>,
    pub replay_forks_threads: NonZeroUsize,
    pub replay_transactions_threads: NonZeroUsize,
    pub tvu_receive_threads: NonZeroUsize,
    pub tvu_sigverify_threads: NonZeroUsize,
}

impl LiteRpcConfig {
    pub fn from_matches(matches: &ArgMatches) -> Result<Self, String> {
        let ledger_path = PathBuf::from(matches.value_of("ledger_path").unwrap());

        let identity_keypair = if let Some(identity_path) = matches.value_of("identity") {
            solana_keypair::read_keypair_file(identity_path)
                .map_err(|e| format!("Failed to read identity keypair: {e}"))?
        } else {
            Keypair::new()
        };

        let entrypoints: Vec<ContactInfo> = matches
            .values_of("entrypoint")
            .map(|values| {
                values
                    .map(|addr_str| {
                        let addr = solana_net_utils::parse_host_port(addr_str)
                            .map_err(|e| format!("Invalid entrypoint '{addr_str}': {e}"))
                            .unwrap();
                        ContactInfo::new_gossip_entry_point(&addr)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let expected_genesis_hash = matches
            .value_of("expected_genesis_hash")
            .map(|s| Hash::from_str(s).map_err(|e| format!("Invalid genesis hash: {e}")))
            .transpose()?;

        let expected_shred_version = matches
            .value_of("expected_shred_version")
            .map(|s| s.parse::<u16>().map_err(|e| format!("Invalid shred version: {e}")))
            .transpose()?;

        let rpc_port = matches
            .value_of("rpc_port")
            .unwrap()
            .parse::<u16>()
            .unwrap();

        let rpc_bind_address: IpAddr = matches
            .value_of("rpc_bind_address")
            .unwrap()
            .parse()
            .map_err(|e| format!("Invalid RPC bind address: {e}"))?;

        let bind_address: IpAddr = matches
            .value_of("bind_address")
            .unwrap()
            .parse()
            .map_err(|e| format!("Invalid bind address: {e}"))?;

        let gossip_port = matches
            .value_of("gossip_port")
            .map(|s| s.parse::<u16>().unwrap())
            .unwrap_or(8001);

        let dynamic_port_range = parse_port_range(matches.value_of("dynamic_port_range").unwrap())
            .ok_or("Invalid dynamic port range")?;

        let account_paths: Vec<PathBuf> = matches
            .values_of("account_paths")
            .map(|vals| vals.map(PathBuf::from).collect())
            .unwrap_or_else(|| vec![ledger_path.join("accounts")]);

        let snapshot_path = matches
            .value_of("snapshots")
            .map(PathBuf::from)
            .unwrap_or_else(|| ledger_path.clone());

        let geyser_plugin_config_files = matches.values_of("geyser_plugin_config").map(|vals| {
            vals.map(PathBuf::from).collect()
        });

        let limit_ledger_size = matches
            .value_of("limit_ledger_size")
            .map(|s| s.parse::<u64>().unwrap());

        let repair_validators: Option<HashSet<Pubkey>> =
            matches.values_of("repair_validators").map(|vals| {
                vals.map(|s| Pubkey::from_str(s).unwrap()).collect()
            });

        let gossip_validators: Option<HashSet<Pubkey>> =
            matches.values_of("gossip_validators").map(|vals| {
                vals.map(|s| Pubkey::from_str(s).unwrap()).collect()
            });

        let replay_forks_threads = NonZeroUsize::new(
            matches
                .value_of("replay_forks_threads")
                .unwrap()
                .parse()
                .unwrap(),
        )
        .ok_or("replay_forks_threads must be > 0")?;

        let replay_transactions_threads = NonZeroUsize::new(
            matches
                .value_of("replay_transactions_threads")
                .unwrap()
                .parse()
                .unwrap(),
        )
        .ok_or("replay_transactions_threads must be > 0")?;

        let tvu_receive_threads = NonZeroUsize::new(
            matches
                .value_of("tvu_receive_threads")
                .unwrap()
                .parse()
                .unwrap(),
        )
        .ok_or("tvu_receive_threads must be > 0")?;

        let tvu_sigverify_threads = NonZeroUsize::new(
            matches
                .value_of("tvu_sigverify_threads")
                .unwrap()
                .parse()
                .unwrap(),
        )
        .ok_or("tvu_sigverify_threads must be > 0")?;

        Ok(Self {
            identity_keypair,
            ledger_path,
            entrypoints,
            expected_genesis_hash,
            expected_shred_version,
            rpc_port,
            rpc_bind_address,
            private_rpc: matches.is_present("private_rpc"),
            full_rpc_api: matches.is_present("full_rpc_api"),
            enable_rpc_transaction_history: matches.is_present("enable_rpc_transaction_history"),
            geyser_plugin_config_files,
            account_paths,
            snapshot_path,
            limit_ledger_size,
            gossip_port,
            dynamic_port_range,
            bind_address,
            repair_validators,
            gossip_validators,
            replay_forks_threads,
            replay_transactions_threads,
            tvu_receive_threads,
            tvu_sigverify_threads,
        })
    }
}
