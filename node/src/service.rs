//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

// local
use storage_chain_runtime::{self, opaque::Block, Balance, AccountId, Nonce, Hash};
use crate::frontier_service::{TracingApi, spawn_frontier_tasks, db_config_dir};
// substrate
use sc_consensus::ImportQueue;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
pub use sc_executor::NativeElseWasmExecutor;
use sp_runtime::app_crypto::AppKey;
use sp_core::Pair;
use sc_keystore::LocalKeystore;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager};
use sc_telemetry::{Telemetry, TelemetryWorker, TelemetryWorkerHandle, TelemetryHandle};
use sp_keystore::SyncCryptoStorePtr;
use substrate_prometheus_endpoint::Registry;
use sc_network::NetworkService;
// std
use std::{sync::{Arc, Mutex}, time::Duration, collections::BTreeMap};
// frontier
use fc_rpc_core::types::{FeeHistoryCache, FeeHistoryCacheLimit};
use sc_network::NetworkBlock;
// cumulus
use cumulus_client_consensus_aura::{AuraConsensus, BuildAuraConsensusParams, SlotProportion};
use cumulus_client_consensus_common::ParachainConsensus;
use cumulus_client_service::{
	start_collator, start_full_node, StartCollatorParams,
};
use cumulus_primitives_core::ParaId;
use cumulus_relay_chain_interface::RelayChainInterface;

// Our native executor instance.
pub struct StorageRuntimeExecutor;

impl sc_executor::NativeExecutionDispatch for StorageRuntimeExecutor {
	/// Only enable the benchmarking host functions when we actually want to benchmark.
	#[cfg(feature = "runtime-benchmarks")]
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;
	/// Otherwise we only use the default Substrate host functions.
	#[cfg(not(feature = "runtime-benchmarks"))]
	type ExtendHostFunctions = ();

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		storage_chain_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		storage_chain_runtime::native_version()
	}
}

type FullClient<RuntimeApi, Executor> = sc_service::TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<Executor>>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
type ParachainBlockImport<RuntimeApi, Executor> =
cumulus_client_consensus_common::ParachainBlockImport<
	Block,
	Arc<FullClient<RuntimeApi, Executor>>,
	FullBackend,
>;

/// A set of APIs that the runtimes like this must implement.
pub trait RuntimeApiCollection:
	cumulus_primitives_core::CollectCollationInfo<Block>
		+ fp_rpc::ConvertTransactionRuntimeApi<Block>
		+ fp_rpc::EthereumRuntimeRPCApi<Block>
		+ moonbeam_rpc_primitives_debug::DebugRuntimeApi<Block>
		+ pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance>
		+ sp_api::ApiExt<Block, StateBackend=sc_client_api::StateBackendFor<FullBackend, Block>>
		+ sp_api::Metadata<Block>
		+ sp_block_builder::BlockBuilder<Block>
		+ sp_consensus_aura::AuraApi<Block, <<AuraId as AppKey>::Pair as Pair>::Public>
		+ sp_offchain::OffchainWorkerApi<Block>
		+ sp_session::SessionKeys<Block>
		+ sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
		+ substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>
{}

impl<Api> RuntimeApiCollection for Api where
	Api: cumulus_primitives_core::CollectCollationInfo<Block>
		+ fp_rpc::ConvertTransactionRuntimeApi<Block>
		+ fp_rpc::EthereumRuntimeRPCApi<Block>
		+ moonbeam_rpc_primitives_debug::DebugRuntimeApi<Block>
		+ pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance>
		+ sp_api::ApiExt<Block, StateBackend=sc_client_api::StateBackendFor<FullBackend, Block>>
		+ sp_api::Metadata<Block>
		+ sp_block_builder::BlockBuilder<Block>
		+ sp_consensus_aura::AuraApi<Block, <<AuraId as AppKey>::Pair as Pair>::Public>
		+ sp_offchain::OffchainWorkerApi<Block>
		+ sp_session::SessionKeys<Block>
		+ sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
		+ substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>
{}


pub fn new_partial<RuntimeApi, Executor>(
	config: &Configuration,
) -> Result<
	sc_service::PartialComponents<
		FullClient<RuntimeApi, Executor>,
		FullBackend,
		FullSelectChain,
		sc_consensus::DefaultImportQueue<Block, FullClient<RuntimeApi, Executor>>,
		sc_transaction_pool::FullPool<Block, FullClient<RuntimeApi, Executor>>,
		(
			Arc<fc_db::Backend<Block>>,
			Option<fc_rpc_core::types::FilterPool>,
			(FeeHistoryCache, FeeHistoryCacheLimit),
			ParachainBlockImport<RuntimeApi, Executor>,
			Option<Telemetry>,
			Option<TelemetryWorkerHandle>,
		),
	>,
	ServiceError,
>
	where
		RuntimeApi: 'static + Send + Sync + sp_api::ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>>,
		RuntimeApi::RuntimeApi: RuntimeApiCollection,
		Executor: 'static + sc_executor::NativeExecutionDispatch,

{
	if config.keystore_remote.is_some() {
		return Err(ServiceError::Other("Remote Keystores are not supported.".into()));
	}

	let telemetry = config
		.telemetry_endpoints
		.clone()
		.filter(|x| !x.is_empty())
		.map(|endpoints| -> Result<_, sc_telemetry::Error> {
			let worker = TelemetryWorker::new(16)?;
			let telemetry = worker.handle().new_telemetry(endpoints);
			Ok((worker, telemetry))
		})
		.transpose()?;

	let executor = NativeElseWasmExecutor::<Executor>::new(
		config.wasm_method,
		config.default_heap_pages,
		config.max_runtime_instances,
		config.runtime_cache_size,
	);

	let (client, backend, keystore_container, task_manager) =
		sc_service::new_full_parts::<Block, RuntimeApi, _>(
			config,
			telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
			executor,
		)?;
	let client = Arc::new(client);

	let telemetry_worker_handle = telemetry.as_ref().map(|(worker, _)| worker.handle());
	let telemetry = telemetry.map(|(worker, telemetry)| {
		task_manager.spawn_handle().spawn("telemetry", None, worker.run());
		telemetry
	});

	// let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = sc_transaction_pool::BasicPool::new_full(
		config.transaction_pool.clone(),
		config.role.is_authority().into(),
		config.prometheus_registry(),
		task_manager.spawn_essential_handle(),
		client.clone(),
	);


	let block_import = ParachainBlockImport::new(client.clone(), backend.clone());

	let import_queue = parachain_build_import_queue(
		client.clone(),
		block_import.clone(),
		config,
		telemetry.as_ref().map(|telemetry| telemetry.handle()),
		&task_manager,
	)?;

	// Frontier setting
	let frontier_backend = Arc::new(fc_db::Backend::open(
		Arc::clone(&client),
		&config.database,
		&db_config_dir(config),
	)?);


	let filter_pool = Some(Arc::new(Mutex::new(BTreeMap::new())));
	let fee_history_limit: u64 = 2048;
	let fee_history_cache: FeeHistoryCache = Arc::new(Mutex::new(BTreeMap::new()));
	let fee_history_cache_limit: FeeHistoryCacheLimit = fee_history_limit;
	let fee_history = (fee_history_cache, fee_history_cache_limit);


	Ok(sc_service::PartialComponents {
		backend: backend.clone(),
		client,
		import_queue,
		keystore_container,
		task_manager,
		transaction_pool,
		select_chain: sc_consensus::LongestChain::new(backend),
		other: (frontier_backend, filter_pool, fee_history, block_import, telemetry, telemetry_worker_handle),
	})
}

fn remote_keystore(_url: &String) -> Result<Arc<LocalKeystore>, &'static str> {
	// FIXME: here would the concrete keystore be built,
	//        must return a concrete type (NOT `LocalKeystore`) that
	//        implements `CryptoStore` and `SyncCryptoStore`
	Err("Remote Keystore not supported.")
}

/// Build the import queue for the parachain runtime.
pub fn parachain_build_import_queue<RuntimeApi, Executor>(
	client: Arc<FullClient<RuntimeApi, Executor>>,
	block_import: ParachainBlockImport<RuntimeApi, Executor>,
	config: &Configuration,
	telemetry: Option<TelemetryHandle>,
	task_manager: &TaskManager,
) -> Result<
	sc_consensus::DefaultImportQueue<Block, FullClient<RuntimeApi, Executor>>,
	sc_service::Error,
>
	where
		RuntimeApi: 'static
		+ Send
		+ Sync
		+ sp_api::ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>>,
		RuntimeApi::RuntimeApi: RuntimeApiCollection,
		Executor: 'static + sc_executor::NativeExecutionDispatch,
{
	let slot_duration = cumulus_client_consensus_aura::slot_duration(&*client)?;

	cumulus_client_consensus_aura::import_queue::<
		sp_consensus_aura::sr25519::AuthorityPair,
		_,
		_,
		_,
		_,
		_,
	>(cumulus_client_consensus_aura::ImportQueueParams {
		block_import,
		client,
		create_inherent_data_providers: move |_, _| async move {
			let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

			let slot =
				sp_consensus_aura::inherents::InherentDataProvider::from_timestamp_and_slot_duration(
					*timestamp,
					slot_duration,
				);

			Ok((slot, timestamp))
		},
		registry: config.prometheus_registry(),
		spawner: &task_manager.spawn_essential_handle(),
		telemetry,
	})
		.map_err(Into::into)
}

/// Start a node with the given parachain `Configuration` and relay chain `Configuration`.
///
/// This is the actual implementation that is abstract over the executor and the runtime api.
#[allow(clippy::too_many_arguments)]
#[sc_tracing::logging::prefix_logs_with("Parachain")]
async fn start_node_impl<RuntimeApi, Executor, RB>(
	parachain_config: Configuration,
	polkadot_config: Configuration,
	collator_options: cumulus_client_cli::CollatorOptions,
	para_id: ParaId,
	_rpc_ext_builder: RB,
	hwbench: Option<sc_sysinfo::HwBench>,
	eth_rpc_config: &crate::frontier_service::EthRpcConfig,
) -> sc_service::error::Result<(TaskManager, Arc<FullClient<RuntimeApi, Executor>>)>
	where
		RuntimeApi: 'static + Send + Sync + sp_api::ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>>,
		RuntimeApi::RuntimeApi: RuntimeApiCollection,
		Executor: 'static + sc_executor::NativeExecutionDispatch,
		RB: Fn(
			Arc<sc_service::TFullClient<Block, RuntimeApi, Executor>>,
		) -> Result<jsonrpsee::RpcModule<()>, sc_service::Error>,

{
	let sc_service::PartialComponents {
		client,
		backend,
		mut task_manager,
		import_queue,
		mut keystore_container,
		select_chain: _,
		transaction_pool,
		other: (
			frontier_backend,
			filter_pool,
			fee_history,
			block_import,
			mut telemetry,
			telemetry_worker_handle
		),
	} = new_partial::<RuntimeApi, Executor>(&parachain_config)?;

	let mut parachain_config = cumulus_client_service::prepare_node_config(parachain_config);

	let (relay_chain_interface, collator_key) = cumulus_client_service::build_relay_chain_interface(
		polkadot_config,
		&parachain_config,
		telemetry_worker_handle,
		&mut task_manager,
		collator_options.clone(),
		hwbench.clone(),
	).await
		.map_err(|e| sc_service::Error::Application(Box::new(e) as Box<_>))?;

	let block_announce_validator =
		cumulus_client_network::BlockAnnounceValidator::new(relay_chain_interface.clone(), para_id);


	if let Some(url) = &parachain_config.keystore_remote {
		match remote_keystore(url) {
			Ok(k) => keystore_container.set_remote_keystore(k),
			Err(e) =>
				return Err(ServiceError::Other(format!(
					"Error hooking up remote keystore for {}: {}",
					url, e
				))),
		};
	}

	let import_queue_service = import_queue.service();

	let (network, system_rpc_tx, tx_handler_controller, network_starter) =
		sc_service::build_network(sc_service::BuildNetworkParams {
			config: &parachain_config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue: import_queue,
			block_announce_validator_builder: Some(Box::new(|_| {
				Box::new(block_announce_validator)
			})),
			warp_sync: None,
		})?;

	if parachain_config.offchain_worker.enabled {
		sc_service::build_offchain_workers(
			&parachain_config,
			task_manager.spawn_handle(),
			client.clone(),
			network.clone(),
		);
	}

	let force_authoring = parachain_config.force_authoring;
	let prometheus_registry = parachain_config.prometheus_registry().cloned();
	let collator = parachain_config.role.is_authority();
	let (fee_history_cache, fee_history_cache_limit) = fee_history;

	let overrides = fc_storage::overrides_handle(client.clone());
	let block_data_cache = Arc::new(fc_rpc::EthBlockDataCacheTask::new(
		task_manager.spawn_handle(),
		overrides.clone(),
		50,
		50,
		prometheus_registry.clone(),
	));

	// for ethereum-compatibility rpc.
	parachain_config.rpc_id_provider = Some(Box::new(fc_rpc::EthereumSubIdProvider));
	let tracing_requesters = spawn_frontier_tasks(
		&task_manager,
		client.clone(),
		backend.clone(),
		frontier_backend.clone(),
		filter_pool.clone(),
		overrides.clone(),
		fee_history_cache.clone(),
		fee_history_cache_limit,
		eth_rpc_config.clone(),
	);

	let rpc_extensions_builder = {
		let client = client.clone();
		let pool = transaction_pool.clone();
		let filter_pool = filter_pool.clone();
		let network = network.clone();
		let frontier_backend = frontier_backend.clone();
		let overrides = overrides.clone();
		let fee_history_cache = fee_history_cache.clone();
		let max_past_logs = 10000;
		let eth_rpc_config = eth_rpc_config.clone();

		Box::new(move |deny_unsafe, subscription_task_executor| {
			let deps =
				crate::rpc::FullDeps {
					client: client.clone(),
					pool: pool.clone(),
					graph: pool.pool().clone(),
					deny_unsafe,
					is_authority: collator,
					network: network.clone(),
					filter_pool: filter_pool.clone(),
					backend: frontier_backend.clone(),
					max_past_logs,
					fee_history_cache: fee_history_cache.clone(),
					fee_history_cache_limit,
					overrides: overrides.clone(),
					block_data_cache: block_data_cache.clone(),
				};

			if eth_rpc_config.tracing_api.contains(&TracingApi::Debug)
				|| eth_rpc_config.tracing_api.contains(&TracingApi::Trace)
			{
				crate::rpc::create_full(
					deps,
					subscription_task_executor,
					Some(crate::rpc::TracingConfig {
						tracing_requesters: tracing_requesters.clone(),
						trace_filter_max_count: eth_rpc_config.tracing_max_count,
					}),
				)
					.map_err(Into::into)
			} else {
				crate::rpc::create_full(deps, subscription_task_executor, None).map_err(Into::into)
			}
		})
	};

	sc_service::spawn_tasks(sc_service::SpawnTasksParams {
		network: network.clone(),
		client: client.clone(),
		keystore: keystore_container.sync_keystore(),
		task_manager: &mut task_manager,
		transaction_pool: transaction_pool.clone(),
		rpc_builder: rpc_extensions_builder,
		backend: backend.clone(),
		system_rpc_tx,
		tx_handler_controller,
		config: parachain_config,
		telemetry: telemetry.as_mut(),
	})?;

	if let Some(hwbench) = hwbench {
		sc_sysinfo::print_hwbench(&hwbench);
		// Here you can check whether the hardware meets your chains' requirements. Putting a link
		// in there and swapping out the requirements for your own are probably a good idea. The
		// requirements for a para-chain are dictated by its relay-chain.
		if !frame_benchmarking_cli::SUBSTRATE_REFERENCE_HARDWARE.check_hardware(&hwbench)
			&& collator
		{
			log::warn!(
				"⚠️  The hardware does not meet the minimal requirements for role 'Authority'."
			);
		}

		if let Some(ref mut telemetry) = telemetry {
			let telemetry_handle = telemetry.handle();
			task_manager.spawn_handle().spawn(
				"telemetry_hwbench",
				None,
				sc_sysinfo::initialize_hwbench_telemetry(telemetry_handle, hwbench),
			);
		}
	}

	let announce_block = {
		let network = network.clone();
		Arc::new(move |hash, data| network.announce_block(hash, data))
	};

	let relay_chain_slot_duration = Duration::from_secs(6);


	if collator {
		let parachain_consensus = build_consensus(
			client.clone(),
			block_import,
			prometheus_registry.as_ref(),
			telemetry.as_ref().map(|t| t.handle()),
			&task_manager,
			relay_chain_interface.clone(),
			transaction_pool,
			network,
			keystore_container.sync_keystore(),
			force_authoring,
			para_id
		)?;

		let spawner = task_manager.spawn_handle();
		let params = StartCollatorParams {
			para_id,
			block_status: client.clone(),
			announce_block,
			client: client.clone(),
			task_manager: &mut task_manager,
			relay_chain_interface,
			spawner,
			parachain_consensus,
			import_queue: import_queue_service,
			collator_key: collator_key.expect("Command line arguments do not allow this. qed"),
			relay_chain_slot_duration,
		};

		start_collator(params).await?;
	} else {
		let params = cumulus_client_service::StartFullNodeParams {
			client: client.clone(),
			announce_block,
			task_manager: &mut task_manager,
			para_id,
			relay_chain_interface,
			relay_chain_slot_duration,
			import_queue: import_queue_service,
		};

		start_full_node(params)?;
	}

	network_starter.start_network();
	Ok((task_manager, client))
}


/// Start a parachain node.
pub async fn start_parachain_node<RuntimeApi, Executor>(
	parachain_config: Configuration,
	polkadot_config: Configuration,
	collator_options: cumulus_client_cli::CollatorOptions,
	para_id: ParaId,
	hwbench: Option<sc_sysinfo::HwBench>,
	eth_rpc_config: &crate::frontier_service::EthRpcConfig,
) -> sc_service::error::Result<(
	TaskManager,
	Arc<sc_service::TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<Executor>>>,
)>
	where
		RuntimeApi: sp_api::ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>>
		+ Send
		+ Sync
		+ 'static,
		RuntimeApi::RuntimeApi: RuntimeApiCollection<StateBackend=sc_client_api::StateBackendFor<FullBackend, Block>>,
		RuntimeApi::RuntimeApi: sp_consensus_aura::AuraApi<Block, AuraId>,
		Executor: 'static + sc_executor::NativeExecutionDispatch,
{
	start_node_impl::<RuntimeApi, Executor, _>(
		parachain_config,
		polkadot_config,
		collator_options,
		para_id,
		|_| Ok(jsonrpsee::RpcModule::new(())),
		hwbench,
		eth_rpc_config,
	)
		.await
}

fn build_consensus<RuntimeApi, Executor>(
	client: Arc<FullClient<RuntimeApi, Executor>>,
	block_import: ParachainBlockImport<RuntimeApi, Executor>,
	prometheus_registry: Option<&Registry>,
	telemetry: Option<TelemetryHandle>,
	task_manager: &TaskManager,
	relay_chain_interface: Arc<dyn RelayChainInterface>,
	transaction_pool: Arc<sc_transaction_pool::FullPool<Block, FullClient<RuntimeApi, Executor>>>,
	sync_oracle: Arc<NetworkService<Block, Hash>>,
	keystore: SyncCryptoStorePtr,
	force_authoring: bool,
	para_id: ParaId,
) -> Result<Box<dyn ParachainConsensus<Block>>, sc_service::Error>
	where
	RuntimeApi: sp_api::ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>>
		+ Send
		+ Sync
		+ 'static,
	RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	RuntimeApi::RuntimeApi: sp_consensus_aura::AuraApi<Block, AuraId>,
	Executor: 'static + sc_executor::NativeExecutionDispatch,
{
	let slot_duration = cumulus_client_consensus_aura::slot_duration(&*client)?;

	let proposer_factory = sc_basic_authorship::ProposerFactory::with_proof_recording(
		task_manager.spawn_handle(),
		client.clone(),
		transaction_pool,
		prometheus_registry,
		telemetry.clone(),
	);

	let params = BuildAuraConsensusParams {
		proposer_factory,
		create_inherent_data_providers: move |_, (relay_parent, validation_data)| {
			let relay_chain_interface = relay_chain_interface.clone();
			async move {
				let parachain_inherent =
					cumulus_primitives_parachain_inherent::ParachainInherentData::create_at(
						relay_parent,
						&relay_chain_interface,
						&validation_data,
						para_id,
					)
					.await;
				let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

				let slot =
						sp_consensus_aura::inherents::InherentDataProvider::from_timestamp_and_slot_duration(
							*timestamp,
							slot_duration,
						);

				let parachain_inherent = parachain_inherent.ok_or_else(|| {
					Box::<dyn std::error::Error + Send + Sync>::from(
						"Failed to create parachain inherent",
					)
				})?;
				Ok((slot, timestamp, parachain_inherent))
			}
		},
		block_import,
		para_client: client,
		backoff_authoring_blocks: Option::<()>::None,
		sync_oracle,
		keystore,
		force_authoring,
		slot_duration,
		// We got around 500ms for proposing
		block_proposal_slot_portion: SlotProportion::new(1f32 / 24f32),
		// And a maximum of 750ms if slots are skipped
		max_block_proposal_slot_portion: Some(SlotProportion::new(1f32 / 16f32)),
		telemetry,
	};

	Ok(AuraConsensus::build::<sp_consensus_aura::sr25519::AuthorityPair, _, _, _, _, _, _>(params))
}
