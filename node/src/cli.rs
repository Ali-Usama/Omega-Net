use cumulus_client_cli::RunCmd;
// std
use std::{path::PathBuf};


#[derive(Debug, clap::Parser)]
#[command(
	propagate_version = true,
	args_conflicts_with_subcommands = true,
	subcommand_negates_reqs = true
)]
pub struct Cli {
	#[command(subcommand)]
	pub subcommand: Option<Subcommand>,

	#[command(flatten)]
	pub run: RunCmd,

	/// Disable automatic hardware benchmarks.
	///
	/// By default these benchmarks are automatically ran at startup and measure
	/// the CPU speed, the memory bandwidth and the disk speed.
	///
	/// The results are then printed out in the logs, and also sent as part of
	/// telemetry, if telemetry is enabled.
	#[arg(long)]
	pub no_hardware_benchmarks: bool,

	/// Relay chain arguments
	#[arg(raw = true)]
	pub relay_chain_args: Vec<String>,

	#[command(flatten)]
	pub eth_args: crate::frontier_service::EthArgs,
}

#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
	/// Key management cli utilities
	#[command(subcommand)]
	Key(sc_cli::KeySubcommand),

	/// Build a chain specification.
	BuildSpec(sc_cli::BuildSpecCmd),

	/// Validate blocks.
	CheckBlock(sc_cli::CheckBlockCmd),

	/// Export blocks.
	ExportBlocks(sc_cli::ExportBlocksCmd),

	/// Export the state of a given block into a chain spec.
	ExportState(sc_cli::ExportStateCmd),

	/// Import blocks.
	ImportBlocks(sc_cli::ImportBlocksCmd),

	/// Remove the whole chain.
	PurgeChain(cumulus_client_cli::PurgeChainCmd),

	/// Revert the chain to a previous state.
	Revert(sc_cli::RevertCmd),

	/// Export the genesis state of the parachain.
	ExportGenesisState(cumulus_client_cli::ExportGenesisStateCommand),

	/// Export the genesis wasm of the parachain.
	ExportGenesisWasm(cumulus_client_cli::ExportGenesisWasmCommand),

	/// Db meta columns information.
	FrontierDb(fc_cli::FrontierDbCmd),

	/// Sub-commands concerned with benchmarking.
	/// The pallet benchmarking moved to the `pallet` sub-command.
	#[cfg(feature = "runtime-benchmarks")]
	#[command(subcommand)]
	Benchmark(Box<frame_benchmarking_cli::BenchmarkCmd>),

	/// Try some testing command against a specified runtime state.
	#[cfg(feature = "try-runtime")]
	TryRuntime(try_runtime_cli::TryRuntimeCmd),

	/// Errors since the binary was not build with `--features runtime-benchmarks`.
	#[cfg(not(feature = "runtime-benchmarks"))]
	Benchmark,

	/// Errors since the binary was not build with `--features try-runtime`.
	#[cfg(not(feature = "try-runtime"))]
	TryRuntime,

	/// Db meta columns information.
	ChainInfo(sc_cli::ChainInfoCmd),
}

#[derive(Debug)]
pub struct RelayChainCli {
	/// The actual relay chain cli object.
	pub base: polkadot_cli::RunCmd,

	/// Optional chain id that should be passed to the relay chain.
	pub chain_id: Option<String>,

	/// The base path that should be used by the relay chain.
	pub base_path: Option<PathBuf>,
}
impl RelayChainCli {
	/// Parse the relay chain CLI parameters using the para chain `Configuration`.
	pub fn new<'a>(
		para_config: &sc_service::Configuration,
		relay_chain_args: impl Iterator<Item = &'a String>,
	) -> Self {
		let extension = crate::chain_spec::Extensions::try_get(&*para_config.chain_spec);
		let chain_id = extension.map(|e| e.relay_chain.clone());
		let base_path = para_config.base_path.as_ref().map(|x| x.path().join("polkadot"));
		Self { base_path, chain_id, base: clap::Parser::parse_from(relay_chain_args) }
	}
}
