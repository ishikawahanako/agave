use {
    crate::config::LiteRpcConfig,
    anyhow::{Result, anyhow},
    crossbeam_channel::{bounded, unbounded},
    log::{info, warn},
    solana_accounts_db::accounts_db::AccountsDbConfig,
    solana_client::connection_cache::ConnectionCache,
    solana_core::{
        banking_trace::BankingTracer,
        cluster_slots_service::cluster_slots::ClusterSlots,
        consensus::{tower_storage::FileTowerStorage, Tower},
        repair::repair_service::OutstandingShredRepairs,
        stats_reporter_service::StatsReporterService,
        system_monitor_service::{SystemMonitorService, SystemMonitorStatsReportConfig},
        tvu::{Tvu, TvuConfig, TvuSockets},
    },
    solana_geyser_plugin_manager::geyser_plugin_service::GeyserPluginService,
    solana_gossip::{
        cluster_info::{ClusterInfo, NodeConfig},
        gossip_service::GossipService,
        node::Node,
    },
    solana_keypair::Keypair,
    solana_ledger::{
        blockstore::Blockstore,
        blockstore_metric_report_service::BlockstoreMetricReportService,
        blockstore_options::BlockstoreOptions,
        blockstore_processor::{self, ProcessOptions, TransactionStatusSender},
        entry_notifier_service::EntryNotifierService,
        leader_schedule_cache::LeaderScheduleCache,
    },
    solana_measure::measure::Measure,
    solana_net_utils::multihomed_sockets::BindIpAddrs,
    solana_poh::{
        poh_recorder::PohRecorder,
        poh_controller::PohController,
        poh_service::PohService,
        record_channels::record_channels,
    },
    solana_pubkey::Pubkey,
    solana_rpc::{
        max_slots::MaxSlots,
        optimistically_confirmed_bank_tracker::{
            BankNotificationSenderConfig, OptimisticallyConfirmedBank,
            OptimisticallyConfirmedBankTracker,
        },
        rpc_completed_slots_service::RpcCompletedSlotsService,
        rpc_pubsub_service::PubSubService,
        rpc_service::{JsonRpcService, JsonRpcServiceConfig},
        rpc_subscriptions::RpcSubscriptions,
        transaction_status_service::TransactionStatusService,
    },
    solana_runtime::{
        accounts_background_service::{
            AbsRequestHandlers, AccountsBackgroundService, PrunedBanksRequestHandler,
            SnapshotRequestHandler,
        },
        bank_forks::BankForks,
        commitment::BlockCommitmentCache,
        prioritization_fee_cache::PrioritizationFeeCache,
        snapshot_controller::SnapshotController,
    },
    solana_send_transaction_service::send_transaction_service,
    solana_shred_version::compute_shred_version,
    solana_signer::Signer,
    solana_streamer::socket::SocketAddrSpace,
    solana_validator_exit::Exit,
    std::{
        collections::HashSet,
        net::SocketAddr,
        num::NonZeroUsize,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc, Mutex, RwLock,
        },
        thread::JoinHandle,
    },
};

const MAX_REPLAY_WAKE_UP_SIGNALS: usize = 1;
const MAX_COMPLETED_SLOTS_IN_CHANNEL: usize = 100;

pub struct LiteNode {
    exit: Arc<AtomicBool>,
    validator_exit: Arc<RwLock<Exit>>,
    geyser_plugin_service: Option<GeyserPluginService>,
    json_rpc_service: Option<JsonRpcService>,
    pubsub_service: Option<PubSubService>,
    rpc_completed_slots_service: Option<JoinHandle<()>>,
    optimistically_confirmed_bank_tracker: Option<OptimisticallyConfirmedBankTracker>,
    transaction_status_service: Option<TransactionStatusService>,
    entry_notifier_service: Option<EntryNotifierService>,
    gossip_service: GossipService,
    poh_service: PohService,
    tvu: Tvu,
    blockstore: Arc<Blockstore>,
    #[allow(dead_code)]
    bank_forks: Arc<RwLock<BankForks>>,
    #[allow(dead_code)]
    cluster_info: Arc<ClusterInfo>,
    accounts_background_service: AccountsBackgroundService,
    blockstore_metric_report_service: BlockstoreMetricReportService,
    stats_reporter_service: StatsReporterService,
    system_monitor_service: Option<SystemMonitorService>,
    poh_recorder: Arc<RwLock<PohRecorder>>,
}

impl LiteNode {
    #[allow(clippy::too_many_lines)]
    pub fn start(config: LiteRpcConfig) -> Result<Self> {
        // Phase 1: Setup
        info!("Phase 1: Basic setup");
        let identity_keypair = Arc::new(config.identity_keypair);
        let id = identity_keypair.pubkey();
        info!("Identity: {id}");

        if rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("solRayonGlob{i:02}"))
            .num_threads(num_cpus::get())
            .build_global()
            .is_err()
        {
            warn!("Rayon global thread pool already initialized");
        }

        let exit = Arc::new(AtomicBool::new(false));
        let validator_exit = Arc::new(RwLock::new(Exit::default()));
        {
            let exit = exit.clone();
            validator_exit
                .write()
                .unwrap()
                .register_exit(Box::new(move || exit.store(true, Ordering::Relaxed)));
        }

        // Create Node with network sockets
        let node_config = NodeConfig {
            bind_ip_addrs: BindIpAddrs::new(vec![config.bind_address])
                .map_err(|e| anyhow!("Failed to bind IP: {e}"))?,
            gossip_port: config.gossip_port,
            port_range: config.dynamic_port_range,
            advertised_ip: config.bind_address,
            public_tpu_addr: None,
            public_tpu_forwards_addr: None,
            num_tvu_receive_sockets: config.tvu_receive_threads,
            num_tvu_retransmit_sockets: NonZeroUsize::new(1).unwrap(),
            num_quic_endpoints: NonZeroUsize::new(1).unwrap(),
            vortexor_receiver_addr: None,
        };
        let mut node = Node::new_with_external_ip(&id, node_config);

        // Set up RPC addresses on the node
        let rpc_addr = SocketAddr::new(config.rpc_bind_address, config.rpc_port);
        let rpc_pubsub_addr = SocketAddr::new(
            config.rpc_bind_address,
            config
                .rpc_port
                .checked_add(1)
                .ok_or_else(|| anyhow!("RPC port overflow"))?,
        );
        if !config.private_rpc {
            node.info.set_rpc(rpc_addr).ok();
            node.info.set_rpc_pubsub(rpc_pubsub_addr).ok();
        }

        // Init sigverify
        info!("Initializing sigverify...");
        solana_core::sigverify::init();

        // Phase 2: Geyser Plugin
        info!("Phase 2: Geyser plugin initialization");
        let mut bank_notification_senders = Vec::new();

        let geyser_plugin_service =
            if let Some(ref config_files) = config.geyser_plugin_config_files {
                let (confirmed_bank_sender, confirmed_bank_receiver) = unbounded();
                bank_notification_senders.push(confirmed_bank_sender);
                Some(
                    GeyserPluginService::new_with_receiver(
                        confirmed_bank_receiver,
                        false,
                        config_files,
                        None,
                    )
                    .map_err(|e| anyhow!("Failed to load Geyser plugin: {e:?}"))?,
                )
            } else {
                None
            };

        let (
            accounts_update_notifier,
            transaction_notifier,
            entry_notifier,
            block_metadata_notifier,
            slot_status_notifier,
        ) = if let Some(ref service) = geyser_plugin_service {
            (
                service.get_accounts_update_notifier(),
                service.get_transaction_notifier(),
                service.get_entry_notifier(),
                service.get_block_metadata_notifier(),
                service.get_slot_status_notifier(),
            )
        } else {
            (None, None, None, None, None)
        };

        info!(
            "Geyser: accounts={}, transactions={}, entries={}",
            accounts_update_notifier.is_some(),
            transaction_notifier.is_some(),
            entry_notifier.is_some(),
        );

        // Phase 3: Blockstore + Bank Loading
        info!("Phase 3: Loading blockstore and bank state");
        let ledger_path = &config.ledger_path;

        let genesis_config =
            solana_genesis_utils::open_genesis_config(ledger_path, u64::MAX)
                .map_err(|e| anyhow!("Failed to open genesis config: {e:?}"))?;

        let blockstore_opts = BlockstoreOptions {
            column_options: solana_ledger::blockstore_options::LedgerColumnOptions {
                compression_type: solana_ledger::blockstore_options::BlockstoreCompressionType::None,
                ..Default::default()
            },
            ..Default::default()
        };
        let blockstore = Blockstore::open_with_options(ledger_path, blockstore_opts)
            .map_err(|e| anyhow!("Failed to open blockstore: {e:?}"))?;

        let (ledger_signal_sender, ledger_signal_receiver) = bounded(MAX_REPLAY_WAKE_UP_SIGNALS);
        blockstore.add_new_shred_signal(ledger_signal_sender);
        let blockstore = Arc::new(blockstore);

        // Transaction history services
        let max_complete_transaction_status_slot = Arc::new(AtomicU64::default());
        let enable_rpc_transaction_history = config.enable_rpc_transaction_history;
        let is_plugin_transaction_history_required = transaction_notifier.is_some();

        let (transaction_status_sender, transaction_status_service) =
            if enable_rpc_transaction_history || is_plugin_transaction_history_required {
                let (status_sender, status_receiver) = unbounded();
                let transaction_status_sender = TransactionStatusSender {
                    sender: status_sender,
                    dependency_tracker: None,
                };
                let service = TransactionStatusService::new(
                    status_receiver,
                    max_complete_transaction_status_slot.clone(),
                    enable_rpc_transaction_history,
                    transaction_notifier,
                    blockstore.clone(),
                    false, // enable_extended_tx_metadata_storage
                    None,  // dependency_tracker
                    exit.clone(),
                );
                (Some(transaction_status_sender), Some(service))
            } else {
                (None, None)
            };

        let entry_notifier_service =
            entry_notifier.map(|notifier| EntryNotifierService::new(notifier, exit.clone()));

        // Set up snapshot config
        let snapshot_config = agave_snapshots::snapshot_config::SnapshotConfig {
            full_snapshot_archives_dir: config.snapshot_path.clone(),
            incremental_snapshot_archives_dir: config.snapshot_path.clone(),
            bank_snapshots_dir: config.snapshot_path.join("bank_snapshots"),
            ..agave_snapshots::snapshot_config::SnapshotConfig::default()
        };

        // Disk-based accounts index with aggressive read cache.
        // In-memory index needs ~160GB RAM on mainnet (more than this machine has).
        let accounts_index_path = ledger_path.join("accounts_index");
        std::fs::create_dir_all(&accounts_index_path).ok();
        let mut accounts_db_config = AccountsDbConfig::default();
        accounts_db_config.index = Some(solana_accounts_db::accounts_index::AccountsIndexConfig {
            drives: Some(vec![accounts_index_path]),
            index_limit_mb: solana_accounts_db::accounts_index::IndexLimitMb::Minimal,
            ..Default::default()
        });
        // Increase read cache to reduce disk lookups during replay
        // (max_data_size_lo, max_data_size_hi): cache evicts when above hi, down to lo
        // Larger cache = fewer disk lookups during replay = lower latency
        accounts_db_config.read_cache_limit_bytes = Some((28 * 1024 * 1024 * 1024, 32 * 1024 * 1024 * 1024));

        let process_options = ProcessOptions {
            run_verification: false,
            accounts_db_config,
            ..ProcessOptions::default()
        };

        info!("Loading bank forks from snapshot...");
        let mut timer = Measure::start("load_bank_forks");
        let (bank_forks, leader_schedule_cache, _starting_snapshot_hashes) =
            solana_ledger::bank_forks_utils::load_bank_forks(
                &genesis_config,
                &blockstore,
                config.account_paths.clone(),
                &snapshot_config,
                &process_options,
                transaction_status_sender.as_ref(),
                entry_notifier_service.as_ref().map(|s| s.sender()),
                accounts_update_notifier,
                exit.clone(),
            )
            .map_err(|e| anyhow!("Failed to load bank forks: {e}"))?;
        timer.stop();
        info!("Loading bank forks done. {timer}");

        let pruned_banks_receiver =
            AccountsBackgroundService::setup_bank_drop_callback(bank_forks.clone());

        // Phase 4: Cluster & Gossip setup
        info!("Phase 4: Cluster and gossip setup");
        let root_bank = bank_forks.read().unwrap().root_bank();
        let root_slot = root_bank.slot();
        let hard_forks = root_bank.hard_forks();
        let shred_version = compute_shred_version(&genesis_config.hash(), Some(&hard_forks));
        info!("Shred version: {shred_version}");

        if let Some(expected) = config.expected_shred_version {
            if expected != shred_version {
                return Err(anyhow!(
                    "Shred version mismatch: expected {expected}, got {shred_version}"
                ));
            }
        }

        node.info.set_shred_version(shred_version);
        node.info
            .set_wallclock(solana_time_utils::timestamp());

        let mut cluster_info = ClusterInfo::new(
            node.info.clone(),
            identity_keypair.clone(),
            SocketAddrSpace::Unspecified,
        );
        cluster_info.set_entrypoints(config.entrypoints.clone());
        cluster_info.restore_contact_info(ledger_path, 120_000);
        let cluster_info = Arc::new(cluster_info);

        // Snapshot controller (load-only, no snapshot generation)
        let (snapshot_request_sender, snapshot_request_receiver) = unbounded();
        let snapshot_controller = Arc::new(SnapshotController::new(
            snapshot_request_sender,
            snapshot_config.clone(),
            root_slot,
        ));

        let pending_snapshot_packages =
            Arc::new(Mutex::new(solana_runtime::accounts_background_service::PendingSnapshotPackages::default()));
        let snapshot_request_handler = SnapshotRequestHandler {
            snapshot_controller: snapshot_controller.clone(),
            snapshot_request_receiver,
            pending_snapshot_packages,
        };
        let pruned_banks_request_handler = PrunedBanksRequestHandler {
            pruned_banks_receiver,
        };
        let accounts_background_service = AccountsBackgroundService::new(
            bank_forks.clone(),
            exit.clone(),
            AbsRequestHandlers {
                snapshot_request_handler,
                pruned_banks_request_handler,
            },
        );

        // Phase 5: PoH
        info!("Phase 5: PoH recorder setup");
        let leader_schedule_cache = Arc::new(leader_schedule_cache);
        let (poh_recorder, _entry_receiver) = {
            let bank = &bank_forks.read().unwrap().working_bank();
            PohRecorder::new_with_clear_signal(
                bank.tick_height(),
                bank.last_blockhash(),
                bank.clone(),
                None,
                bank.ticks_per_slot(),
                false, // delay_leader_block_for_pending_fork
                blockstore.clone(),
                blockstore.get_new_shred_signal(0),
                &leader_schedule_cache,
                &genesis_config.poh_config,
                exit.clone(),
            )
        };
        let (_record_sender, record_receiver) =
            record_channels(transaction_status_sender.is_some());
        let poh_recorder = Arc::new(RwLock::new(poh_recorder));
        let (poh_controller, poh_service_message_receiver) = PohController::new();

        let banking_tracer = BankingTracer::new_disabled();

        let poh_service = PohService::new(
            poh_recorder.clone(),
            &genesis_config.poh_config,
            exit.clone(),
            bank_forks.read().unwrap().root_bank().ticks_per_slot(),
            solana_poh::poh_service::DEFAULT_PINNED_CPU_CORE,
            solana_poh::poh_service::DEFAULT_HASHES_PER_BATCH,
            record_receiver,
            poh_service_message_receiver,
        );

        // Phase 6: Process blockstore catch-up
        info!("Phase 6: Processing blockstore catch-up from snapshot");
        let entry_notification_sender_for_catchup =
            entry_notifier_service.as_ref().map(|s| s.sender());
        {
            let mut timer = Measure::start("process_blockstore_from_root");
            blockstore_processor::process_blockstore_from_root(
                &blockstore,
                &bank_forks,
                &leader_schedule_cache,
                &process_options,
                transaction_status_sender.as_ref(),
                entry_notification_sender_for_catchup,
                Some(&*snapshot_controller),
            )
            .map_err(|e| anyhow!("Failed to process blockstore: {e}"))?;
            timer.stop();
            info!("Blockstore catch-up done. {timer}");
        }

        // Phase 7: RPC Server
        info!("Phase 7: RPC server setup");
        let mut block_commitment_cache = BlockCommitmentCache::default();
        {
            let bank_forks_guard = bank_forks.read().unwrap();
            block_commitment_cache.initialize_slots(
                bank_forks_guard.working_bank().slot(),
                bank_forks_guard.root(),
            );
        }
        let block_commitment_cache = Arc::new(RwLock::new(block_commitment_cache));

        let optimistically_confirmed_bank =
            OptimisticallyConfirmedBank::locked_from_bank_forks_root(&bank_forks);

        let max_slots = Arc::new(MaxSlots::default());
        let prioritization_fee_cache = Arc::new(PrioritizationFeeCache::default());

        let connection_cache = Arc::new(ConnectionCache::with_udp(
            "connection_cache_lite_rpc",
            1, // pool_size
        ));

        let (bank_notification_sender, bank_notification_receiver) = unbounded();
        let confirmed_bank_subscribers = if !bank_notification_senders.is_empty() {
            Some(Arc::new(RwLock::new(bank_notification_senders)))
        } else {
            None
        };

        let rpc_config = solana_rpc::rpc::JsonRpcConfig {
            enable_rpc_transaction_history,
            full_api: config.full_rpc_api,
            ..Default::default()
        };

        let json_rpc_service = {
            let config_struct = JsonRpcServiceConfig {
                rpc_addr,
                rpc_config: rpc_config.clone(),
                snapshot_config: Some(snapshot_config.clone()),
                bank_forks: bank_forks.clone(),
                block_commitment_cache: block_commitment_cache.clone(),
                blockstore: blockstore.clone(),
                cluster_info: cluster_info.clone(),
                poh_recorder: Some(poh_recorder.clone()),
                genesis_hash: genesis_config.hash(),
                ledger_path: ledger_path.to_path_buf(),
                validator_exit: validator_exit.clone(),
                exit: exit.clone(),
                override_health_check: Arc::new(AtomicBool::new(false)),
                optimistically_confirmed_bank: optimistically_confirmed_bank.clone(),
                send_transaction_service_config: send_transaction_service::Config::default(),
                max_slots: max_slots.clone(),
                leader_schedule_cache: leader_schedule_cache.clone(),
                max_complete_transaction_status_slot: max_complete_transaction_status_slot.clone(),
                prioritization_fee_cache: prioritization_fee_cache.clone(),
                client_option: solana_client::client_option::ClientOption::ConnectionCache(
                    connection_cache.clone(),
                ),
            };
            Some(
                JsonRpcService::new_with_config(config_struct)
                    .map_err(|e| anyhow!("Failed to start RPC service: {e}"))?,
            )
        };

        let rpc_subscriptions = Arc::new(RpcSubscriptions::new_with_config(
            exit.clone(),
            max_complete_transaction_status_slot.clone(),
            blockstore.clone(),
            bank_forks.clone(),
            block_commitment_cache.clone(),
            optimistically_confirmed_bank.clone(),
            &solana_rpc::rpc_pubsub_service::PubSubConfig::default(),
            None,
        ));

        let pubsub_service = if config.full_rpc_api {
            let (trigger, service) = PubSubService::new(
                solana_rpc::rpc_pubsub_service::PubSubConfig::default(),
                &rpc_subscriptions,
                rpc_pubsub_addr,
            );
            validator_exit
                .write()
                .unwrap()
                .register_exit(Box::new(move || trigger.cancel()));
            Some(service)
        } else {
            None
        };

        // Completed slots service for Geyser slot notifications
        let rpc_completed_slots_service =
            if config.full_rpc_api || geyser_plugin_service.is_some() {
                let (completed_slots_sender, completed_slots_receiver) =
                    bounded(MAX_COMPLETED_SLOTS_IN_CHANNEL);
                blockstore.add_completed_slots_signal(completed_slots_sender);
                Some(RpcCompletedSlotsService::spawn(
                    completed_slots_receiver,
                    rpc_subscriptions.clone(),
                    slot_status_notifier.clone(),
                    exit.clone(),
                ))
            } else {
                None
            };

        let optimistically_confirmed_bank_tracker =
            Some(OptimisticallyConfirmedBankTracker::new(
                bank_notification_receiver,
                exit.clone(),
                bank_forks.clone(),
                optimistically_confirmed_bank,
                rpc_subscriptions.clone(),
                confirmed_bank_subscribers,
                prioritization_fee_cache.clone(),
                None, // dependency_tracker
            ));

        let bank_notification_sender_config = BankNotificationSenderConfig {
            sender: bank_notification_sender,
            should_send_parents: geyser_plugin_service.is_some(),
            dependency_tracker: None,
        };

        // Phase 8: Gossip + Repair setup
        info!("Phase 8: Gossip and repair setup");
        let (stats_reporter_sender, stats_reporter_receiver) = unbounded();
        let stats_reporter_service =
            StatsReporterService::new(stats_reporter_receiver, exit.clone());

        let gossip_service = GossipService::new(
            &cluster_info,
            Some(bank_forks.clone()),
            node.sockets.gossip.clone(),
            config.gossip_validators.clone(),
            false, // should_check_duplicate_instance
            Some(stats_reporter_sender),
            exit.clone(),
        );

        // System monitor
        let system_monitor_service = Some(SystemMonitorService::new(
            exit.clone(),
            SystemMonitorStatsReportConfig {
                report_os_memory_stats: true,
                report_os_network_stats: true,
                report_os_cpu_stats: true,
                report_os_disk_stats: true,
            },
        ));

        let blockstore_metric_report_service =
            BlockstoreMetricReportService::new(blockstore.clone(), exit.clone());

        // Phase 9: TVU Pipeline
        info!("Phase 9: TVU pipeline setup");

        let (replay_vote_sender, _replay_vote_receiver) = unbounded();
        let (retransmit_slots_sender, _retransmit_slots_receiver) = unbounded();
        let (_verified_vote_sender, verified_vote_receiver) = unbounded();
        let (_gossip_verified_vote_hash_sender, gossip_verified_vote_hash_receiver) = unbounded();
        let (_duplicate_confirmed_slot_sender, duplicate_confirmed_slots_receiver) = unbounded();

        let entry_notification_sender = entry_notifier_service
            .as_ref()
            .map(|service| service.sender_cloned());

        // Dummy turbine QUIC endpoint (mainnet-style: no QUIC turbine)
        let (turbine_quic_endpoint_sender, _) = tokio::sync::mpsc::channel(1);
        let (_, turbine_quic_endpoint_receiver) = unbounded();
        // Dummy repair QUIC senders (same as RepairQuicAsyncSenders::new_dummy())
        let repair_request_quic_sender = tokio::sync::mpsc::channel(1).0;
        let ancestor_hashes_request_quic_sender = tokio::sync::mpsc::channel(1).0;
        let (_, repair_response_quic_receiver) = unbounded();
        let (_, ancestor_hashes_response_quic_receiver) = unbounded();

        let outstanding_repair_requests = Arc::<RwLock<OutstandingShredRepairs>>::default();
        let cluster_slots = Arc::new(ClusterSlots::new(
            &bank_forks.read().unwrap().root_bank(),
            &cluster_info,
        ));

        let vote_tracker =
            Arc::new(solana_core::cluster_info_vote_listener::VoteTracker::default());

        let tvu = Tvu::new(
            &id,                           // vote_account (use identity to avoid tower reload issue)
            Arc::new(RwLock::new(vec![])), // authorized_voter_keypairs (empty = no voting)
            &bank_forks,
            &cluster_info,
            TvuSockets {
                repair: node.sockets.repair.try_clone().unwrap(),
                retransmit: node.sockets.retransmit_sockets,
                fetch: node.sockets.tvu,
                ancestor_hashes_requests: node.sockets.ancestor_hashes_requests,
                alpenglow: None,
            },
            blockstore.clone(),
            ledger_signal_receiver,
            Some(rpc_subscriptions.clone()),
            &poh_recorder,
            poh_controller,
            Tower::default(),
            Arc::new(FileTowerStorage::new(ledger_path.to_path_buf())),
            &leader_schedule_cache,
            exit.clone(),
            block_commitment_cache,
            Arc::new(AtomicBool::new(false)), // turbine_disabled=false: we RECEIVE shreds, just don't retransmit
            transaction_status_sender,
            entry_notification_sender,
            vote_tracker,
            retransmit_slots_sender,
            gossip_verified_vote_hash_receiver,
            verified_vote_receiver,
            replay_vote_sender,
            None, // completed_data_sets_sender
            Some(bank_notification_sender_config),
            duplicate_confirmed_slots_receiver,
            TvuConfig {
                max_ledger_shreds: config.limit_ledger_size,
                shred_version,
                repair_validators: config.repair_validators,
                repair_whitelist: Arc::new(RwLock::new(HashSet::new())),
                wait_for_vote_to_start_leader: false,
                replay_forks_threads: config.replay_forks_threads,
                replay_transactions_threads: config.replay_transactions_threads,
                shred_sigverify_threads: config.tvu_sigverify_threads,
                xdp_sender: None,
            },
            &max_slots,
            block_metadata_notifier,
            None, // wait_to_vote_slot
            Some(snapshot_controller),
            None, // log_messages_bytes_limit
            None, // connection_cache for warmup
            &prioritization_fee_cache,
            banking_tracer,
            turbine_quic_endpoint_sender,
            turbine_quic_endpoint_receiver,
            repair_response_quic_receiver,
            repair_request_quic_sender,
            ancestor_hashes_request_quic_sender,
            ancestor_hashes_response_quic_receiver,
            outstanding_repair_requests,
            cluster_slots,
            None, // wen_restart_repair_slots
            slot_status_notifier,
            Arc::new(ConnectionCache::with_udp("vote_cache_lite", 1)),
        )
        .map_err(|e| anyhow!("Failed to start TVU: {e}"))?;

        info!("Lite RPC node fully initialized");
        info!("  RPC: {rpc_addr}");
        info!("  Gossip: {}:{}", config.bind_address, config.gossip_port);
        info!(
            "  Geyser plugins: {}",
            config
                .geyser_plugin_config_files
                .as_ref()
                .map(|f| f.len())
                .unwrap_or(0)
        );

        Ok(Self {
            exit,
            validator_exit,
            geyser_plugin_service,
            json_rpc_service,
            pubsub_service,
            rpc_completed_slots_service,
            optimistically_confirmed_bank_tracker,
            transaction_status_service,
            entry_notifier_service,
            gossip_service,
            poh_service,
            tvu,
            blockstore,
            bank_forks,
            cluster_info,
            accounts_background_service,
            blockstore_metric_report_service,
            stats_reporter_service,
            system_monitor_service,
            poh_recorder,
        })
    }

    pub fn join(self) {
        info!("Waiting for exit signal...");

        // Wait for exit signal
        #[cfg(unix)]
        {
            use signal_hook::consts::{SIGINT, SIGTERM};
            use signal_hook::iterator::Signals;
            let mut signals = Signals::new([SIGINT, SIGTERM]).unwrap();
            for sig in signals.forever() {
                info!("Received signal {sig}, shutting down...");
                break;
            }
        }
        #[cfg(not(unix))]
        {
            loop {
                if self.exit.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }

        info!("Initiating shutdown...");
        self.exit.store(true, Ordering::Relaxed);
        self.validator_exit.write().unwrap().exit();

        // Drop blockstore signals
        self.blockstore.drop_signal();

        info!("Joining PoH service...");
        self.poh_service.join().ok();
        drop(self.poh_recorder);

        info!("Joining RPC services...");
        if let Some(service) = self.json_rpc_service {
            service.join().ok();
        }
        if let Some(service) = self.pubsub_service {
            service.join().ok();
        }
        if let Some(tracker) = self.optimistically_confirmed_bank_tracker {
            tracker.join().ok();
        }
        if let Some(service) = self.transaction_status_service {
            service.join().ok();
        }
        if let Some(service) = self.entry_notifier_service {
            service.join().ok();
        }
        if let Some(handle) = self.rpc_completed_slots_service {
            handle.join().ok();
        }

        info!("Joining gossip service...");
        self.gossip_service.join().ok();

        info!("Joining TVU...");
        self.tvu.join().ok();

        info!("Joining accounts background service...");
        self.accounts_background_service.join().ok();

        self.blockstore_metric_report_service.join().ok();
        self.stats_reporter_service.join().ok();
        if let Some(service) = self.system_monitor_service {
            service.join().ok();
        }

        info!("Joining Geyser plugin service...");
        if let Some(service) = self.geyser_plugin_service {
            service.join().ok();
        }

        info!("Lite RPC node shutdown complete.");
    }
}
