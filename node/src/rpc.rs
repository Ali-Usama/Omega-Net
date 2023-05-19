//! A collection of node-specific RPC methods.
//! Substrate provides the `sc-rpc` crate, which defines the core RPC layer
//! used by Substrate nodes. This file extends those RPC definitions with
//! capabilities that are specific to this project's runtime configuration.

#![warn(missing_docs)]

// std
use std::sync::Arc;
// local
use omega_net_runtime::{opaque::Block, AccountId, Balance, Hash, Hashing, Nonce};
// substrate
use sc_client_api::{backend::{Backend, StorageProvider, StateBackend},
					client::BlockchainEvents,
					AuxStore};
use sc_network::NetworkService;
use sc_transaction_pool::{ChainApi, Pool};
use sc_transaction_pool_api::TransactionPool;
pub use sc_rpc_api::DenyUnsafe;
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder;
use sp_blockchain::{ HeaderBackend, HeaderMetadata};
// frontier
use fc_rpc::EthBlockDataCacheTask;
use fc_rpc_core::types::{FeeHistoryCache, FeeHistoryCacheLimit};
// moonbeam
use moonbeam_rpc_debug::{Debug, DebugServer};
use moonbeam_rpc_trace::{Trace, TraceServer};


/// A type representing all RPC extensions.
pub type RpcExtension = jsonrpsee::RpcModule<()>;

/// EVM tracing rpc server config
pub struct TracingConfig {
	/// Debug or Trace
	pub tracing_requesters: crate::frontier_service::RpcRequesters,
	/// Tracing max count
	pub trace_filter_max_count: u32,
}

/// Full client dependencies.
pub struct FullDeps<C, P, A: ChainApi> {
	/// The client instance to use.
	pub client: Arc<C>,
	/// Transaction pool instance.
	pub pool: Arc<P>,
	/// Graph pool instance
	pub graph: Arc<Pool<A>>,
	/// Whether to deny unsafe calls
	pub deny_unsafe: DenyUnsafe,
	/// Node Authority Flag
	pub is_authority: bool,
	/// Network Service
	pub network: Arc<NetworkService<Block, Hash>>,
	/// EthFilterApi pool
	pub filter_pool: Option<fc_rpc_core::types::FilterPool>,
	/// Backend
	pub backend: Arc<fc_db::Backend<Block>>,
	/// Maximum number of logs in the query
	pub max_past_logs: u32,
	/// Fee history cache.
	pub fee_history_cache: FeeHistoryCache,
	/// Maximum fee history cache size.
	pub fee_history_cache_limit: FeeHistoryCacheLimit,
	/// Ethereum data access overrides
	pub overrides: Arc<fc_rpc::OverrideHandle<Block>>,
	/// Cache for Ethereum Block Data
	pub block_data_cache: Arc<EthBlockDataCacheTask<Block>>,
}

/// Instantiate all full RPC extensions.
pub fn create_full<C, P, BE, A>(
	deps: FullDeps<C, P, A>,
	subscription_task_executor: sc_rpc::SubscriptionTaskExecutor,
	tracing_config: Option<TracingConfig>,
) -> Result<RpcExtension, Box<dyn std::error::Error + Send + Sync>>
	where
		BE: Backend<Block> + 'static,
		BE::State: StateBackend<Hashing>,
		C: 'static + Send + Sync + StorageProvider<Block, BE>
		+ BlockchainEvents<Block>
		+ AuxStore
		+ ProvideRuntimeApi<Block>
		+ HeaderBackend<Block>
		+ HeaderMetadata<Block, Error=sp_blockchain::Error>,
		C::Api: substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>
		+ pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance>
		+ BlockBuilder<Block>
		+ fp_rpc::ConvertTransactionRuntimeApi<Block>
		+ fp_rpc::EthereumRuntimeRPCApi<Block>,
		P: TransactionPool<Block=Block> + 'static + Send + Sync,
		A: ChainApi<Block=Block> + 'static,
{
	// frontier
	use fc_rpc::{
		Eth, EthApiServer, Net, NetApiServer, EthFilter, EthFilterApiServer, EthPubSub, EthPubSubApiServer,
		Web3, Web3ApiServer,
	};
	use fp_rpc::NoTransactionConverter;
	// substrate
	use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApiServer};
	use substrate_frame_rpc_system::{System, SystemApiServer};

	let mut module = RpcExtension::new(());
	let FullDeps {
		client,
		pool,
		graph,
		deny_unsafe,
		is_authority,
		network,
		filter_pool,
		backend,
		max_past_logs,
		fee_history_cache,
		fee_history_cache_limit,
		overrides,
		block_data_cache
	} = deps;


	module.merge(System::new(client.clone(), pool.clone(), deny_unsafe).into_rpc())?;
	module.merge(TransactionPayment::new(client.clone()).into_rpc())?;
	module.merge(
		Net::new(
			client.clone(),
			network.clone(),
			// Whether to format the `peer_count` response as Hex(default) or not.
			true,
		).into_rpc())?;
	module.merge(
		Eth::new(
			client.clone(),
			pool.clone(),
			graph,
			<Option<NoTransactionConverter>>::None,
			network.clone(),
			vec![],
			overrides.clone(),
			backend.clone(),
			is_authority,
			block_data_cache.clone(),
			fee_history_cache,
			fee_history_cache_limit,
			10,
		)
			.into_rpc())?;

	if let Some(filter_pool) = filter_pool {
		module.merge(
			EthFilter::new(
				client.clone(),
				backend,
				filter_pool,
				500_usize,
				max_past_logs,
				block_data_cache,
			)
				.into_rpc()
		)?;
	}

	module.merge(
		EthPubSub::new(
			pool,
			client.clone(),
			network.clone(),
			subscription_task_executor,
			overrides,
		)
			.into_rpc()
	)?;

	module.merge(
		Web3::new(
			client.clone()
		)
			.into_rpc()
	)?;

	if let Some(tracing_config) = tracing_config {
		if let Some(trace_filter_requester) = tracing_config.tracing_requesters.trace {
			module.merge(
				Trace::new(client, trace_filter_requester, tracing_config.trace_filter_max_count)
					.into_rpc(),
			)?;
		}

		if let Some(debug_requester) = tracing_config.tracing_requesters.debug {
			module.merge(Debug::new(debug_requester).into_rpc())?;
		}
	}


	Ok(module)
}
