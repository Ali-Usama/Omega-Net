//! Service and service factory implementation. Specialized wrapper over substrate service, taken from darwinia.

// std
use std::{path::PathBuf, sync::Arc, time::Duration};
use std::str::FromStr;
// crates.io
use futures::{future, StreamExt};
use tokio::sync::Semaphore;
// frontier
use fc_db::Backend as FrontierBackend;
use fc_mapping_sync::{MappingSyncWorker, SyncStrategy};
use fc_rpc::{EthTask, OverrideHandle};
use fc_rpc_core::types::{FeeHistoryCache, FeeHistoryCacheLimit, FilterPool};
//  moonbeam
use moonbeam_rpc_debug::{DebugHandler, DebugRequester};
use moonbeam_rpc_trace::{CacheRequester as TraceFilterCacheRequester, CacheTask};
// substrate
use sc_service::{BasePath, Configuration, TaskManager};
// local
use storage_chain_runtime::{BlockNumber, Hash, Hashing };

#[derive(Clone)]
pub struct RpcRequesters {
	pub debug: Option<DebugRequester>,
	pub trace: Option<TraceFilterCacheRequester>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_frontier_tasks<B, BE, C>(
	task_manager: &TaskManager,
	client: Arc<C>,
	backend: Arc<BE>,
	frontier_backend: Arc<FrontierBackend<B>>,
	filter_pool: Option<FilterPool>,
	overrides: Arc<OverrideHandle<B>>,
	fee_history_cache: FeeHistoryCache,
	fee_history_cache_limit: FeeHistoryCacheLimit,
	eth_rpc_config: EthRpcConfig,
) -> RpcRequesters
where
	C: 'static
		+ sp_api::ProvideRuntimeApi<B>
		+ sp_blockchain::HeaderBackend<B>
		+ sp_blockchain::HeaderMetadata<B, Error = sp_blockchain::Error>
		+ sc_client_api::BlockOf
		+ sc_client_api::BlockchainEvents<B>
		+ sc_client_api::backend::StorageProvider<B, BE>,
	C::Api: sp_block_builder::BlockBuilder<B>
		+ fp_rpc::EthereumRuntimeRPCApi<B>
		+ moonbeam_rpc_primitives_debug::DebugRuntimeApi<B>,
	B: 'static + Send + Sync + sp_runtime::traits::Block<Hash = Hash>,
	B::Header: sp_api::HeaderT<Number = BlockNumber>,
	BE: 'static + sc_client_api::backend::Backend<B>,
	BE::State: sc_client_api::backend::StateBackend<Hashing>,
{
	task_manager.spawn_essential_handle().spawn(
		"frontier-mapping-sync-worker",
		Some("frontier"),
		MappingSyncWorker::new(
			client.import_notification_stream(),
			Duration::new(6, 0),
			client.clone(),
			backend.clone(),
			overrides.clone(),
			frontier_backend.clone(),
			3,
			0,
			SyncStrategy::Parachain,
		)
		.for_each(|()| future::ready(())),
	);

	// Spawn Frontier EthFilterApi maintenance task.
	if let Some(filter_pool) = filter_pool {
		// Each filter is allowed to stay in the pool for 100 blocks.
		const FILTER_RETAIN_THRESHOLD: u64 = 100;
		task_manager.spawn_essential_handle().spawn(
			"frontier-filter-pool",
			Some("frontier"),
			EthTask::filter_pool_task(client.clone(), filter_pool, FILTER_RETAIN_THRESHOLD),
		);
	}

	// Spawn Frontier FeeHistory cache maintenance task.
	task_manager.spawn_essential_handle().spawn(
		"frontier-fee-history",
		Some("frontier"),
		EthTask::fee_history_task(
			client.clone(),
			overrides.clone(),
			fee_history_cache,
			fee_history_cache_limit,
		),
	);

	if eth_rpc_config.tracing_api.contains(&TracingApi::Debug)
		|| eth_rpc_config.tracing_api.contains(&TracingApi::Trace)
	{
		let permit_pool = Arc::new(Semaphore::new(eth_rpc_config.tracing_max_permits as usize));
		let (trace_filter_task, trace_filter_requester) =
			if eth_rpc_config.tracing_api.contains(&TracingApi::Trace) {
				let (trace_filter_task, trace_filter_requester) = CacheTask::create(
					Arc::clone(&client),
					Arc::clone(&backend),
					Duration::from_secs(eth_rpc_config.tracing_cache_duration),
					Arc::clone(&permit_pool),
					Arc::clone(&overrides),
				);
				(Some(trace_filter_task), Some(trace_filter_requester))
			} else {
				(None, None)
			};

		let (debug_task, debug_requester) =
			if eth_rpc_config.tracing_api.contains(&TracingApi::Debug) {
				let (debug_task, debug_requester) = DebugHandler::task(
					Arc::clone(&client),
					Arc::clone(&backend),
					Arc::clone(&frontier_backend),
					Arc::clone(&permit_pool),
					Arc::clone(&overrides),
					eth_rpc_config.tracing_raw_max_memory_usage,
				);
				(Some(debug_task), Some(debug_requester))
			} else {
				(None, None)
			};

		// `trace_filter` cache task. Essential.
		// Proxies rpc requests to it's handler.
		if let Some(trace_filter_task) = trace_filter_task {
			task_manager.spawn_essential_handle().spawn(
				"trace-filter-cache",
				Some("eth-tracing"),
				trace_filter_task,
			);
		}

		// `debug` task if enabled. Essential.
		// Proxies rpc requests to it's handler.
		if let Some(debug_task) = debug_task {
			task_manager.spawn_essential_handle().spawn(
				"tracing_api-debug",
				Some("eth-tracing"),
				debug_task,
			);
		}

		RpcRequesters { debug: debug_requester, trace: trace_filter_requester }
	} else {
		RpcRequesters { debug: None, trace: None }
	}
}

pub(crate) fn db_config_dir(config: &Configuration) -> PathBuf {
	config
		.base_path
		.as_ref()
		.map(|base_path| base_path.config_dir(config.chain_spec.id()))
		.unwrap_or_else(|| {
			BasePath::from_project("", "", "storage-chain")
				.config_dir(config.chain_spec.id())
		})
}


#[derive(Debug, clap::Parser)]
pub struct EthArgs {
	/// Enable EVM tracing functionalities.
	#[arg(long, value_delimiter = ',')]
	pub tracing_api: Vec<TracingApi>,

	/// Number of concurrent tracing tasks. Meant to be shared by both "debug" and "trace" modules.
	#[arg(long, default_value = "10")]
	pub tracing_max_permits: u32,

	/// Maximum number of trace entries a single request of `trace_filter` is allowed to return.
	/// A request asking for more or an unbounded one going over this limit will both return an
	/// error.
	#[arg(long, default_value = "500")]
	pub tracing_max_count: u32,

	// Duration (in seconds) after which the cache of `trace_filter` for a given block will be
	/// discarded.
	#[arg(long, default_value = "300")]
	pub tracing_cache_duration: u64,

	/// Size in bytes of data a raw tracing request is allowed to use.
	/// Bound the size of memory, stack and storage data.
	#[arg(long, default_value = "20000000")]
	pub tracing_raw_max_memory_usage: usize,

	/// Size in bytes of the LRU cache for block data.
	#[arg(long, default_value = "300000000")]
	pub eth_log_block_cache: usize,

	/// Size of the LRU cache for block data and their transaction statuses.
	#[arg(long, default_value = "300000000")]
	pub eth_statuses_cache: usize,

	/// Maximum number of logs in a query.
	#[arg(long, default_value = "10000")]
	pub max_past_logs: u32,

	/// Maximum fee history cache size.
	#[arg(long, default_value = "2048")]
	pub fee_history_limit: u64,
}
impl EthArgs {
	pub fn build_eth_rpc_config(&self) -> EthRpcConfig {
		EthRpcConfig {
			tracing_api: self.tracing_api.clone(),
			tracing_max_permits: self.tracing_max_permits,
			tracing_max_count: self.tracing_max_permits,
			tracing_cache_duration: self.tracing_cache_duration,
			tracing_raw_max_memory_usage: self.tracing_raw_max_memory_usage,
			eth_statuses_cache: self.eth_statuses_cache,
			eth_log_block_cache: self.eth_log_block_cache,
			max_past_logs: self.max_past_logs,
			fee_history_limit: self.fee_history_limit,
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum TracingApi {
	Debug,
	Trace,
}
impl FromStr for TracingApi {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			"debug" => Self::Debug,
			"trace" => Self::Trace,
			_ => return Err(format!("`{}` is not recognized as a supported Ethereum Api", s)),
		})
	}
}

#[derive(Clone, Debug)]
pub struct EthRpcConfig {
	pub tracing_api: Vec<TracingApi>,
	pub tracing_max_permits: u32,
	pub tracing_max_count: u32,
	pub tracing_cache_duration: u64,
	pub tracing_raw_max_memory_usage: usize,
	pub eth_log_block_cache: usize,
	pub eth_statuses_cache: usize,
	pub fee_history_limit: u64,
	pub max_past_logs: u32,
}
