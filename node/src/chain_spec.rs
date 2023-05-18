use storage_chain_runtime::{*, opaque::*, currency::*};
use cumulus_primitives_core::ParaId;
use sc_chain_spec::{ChainSpecExtension, ChainSpecGroup};
use sc_service::{ChainType, Properties};
use hex_literal::hex;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{ Pair, Public, H160, U256};
use std::{collections::BTreeMap, default::Default};
use serde::{Serialize, Deserialize};

// The URL for the telemetry server.
// const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";

/// Specialized `ChainSpec` for the storage-chain runtime
pub type ChainSpec = sc_service::GenericChainSpec<GenesisConfig, Extensions>;

/// The default XCM version to set in genesis config.
const SAFE_XCM_VERSION: u32 = xcm::prelude::XCM_VERSION;

const PROTOCOL_ID: &str = "stor";

/// Helper function to generate a crypto pair from seed
fn get_from_secret<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.unwrap_or_else(|_| panic!("Invalid string '{}'", seed))
		.public()
}

/// This function's return type must always match the session keys of the chain in tuple format.
fn get_collator_keys_from_seed(seed: &str) -> AuraId {
	get_from_secret::<AuraId>(seed)
}

/// The extensions for the [`ChainSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ChainSpecGroup, ChainSpecExtension)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
	/// The relay chain of the Parachain.
	pub relay_chain: String,
	/// The id of the Parachain.
	pub para_id: u32,
}

impl Extensions {
	/// Try to get the extension from the given `ChainSpec`.
	pub fn try_get(chain_spec: &dyn sc_service::ChainSpec) -> Option<&Self> {
		sc_chain_spec::get_extension(chain_spec.extensions())
	}
}


fn session_keys(
	aura: AuraId,
) -> SessionKeys {
	SessionKeys { aura }
}

pub fn chainspec_properties() -> Properties {
	let mut properties = Properties::new();
	properties.insert("tokenDecimals".into(), 18.into());
	properties.insert("tokenSymbol".into(), "STOR".into());
	properties
}


const ALITH: &str = "0xf24FF3a9CF04c71Dbc94D0b566f7A27B94566cac";
const BALTATHAR: &str = "0x3Cd0A705a2DC65e5b1E1205896BaA2be8A07c6e0";
const CHARLETH: &str = "0x798d4Ba9baf0064Ec19eB4F0a1a45785ae9D6DFc";
const DOROTHY: &str = "0x773539d4Ac0e786233D90A233654ccEE26a613D9";
// const ETHAN: &str = "0xFf64d3F6efE2317EE2807d223a0Bdc4c0c49dfDB";


pub fn development_config() -> Result<ChainSpec, String> {

	Ok(ChainSpec::from_genesis(
		// Name
		"Development",
		// ID
		"dev",
		ChainType::Development,
		move || {
			testnet_genesis(
				// Initial PoA authorities
				vec![
					// Bind the `Alice` to `Alith` to make `--alice` available for testnet.
					(array_bytes::hex_n_into_unchecked(ALITH),
					 get_collator_keys_from_seed("Alice"),
					 ),
				],
				// Sudo account
				AccountId::from(hex!("f24FF3a9CF04c71Dbc94D0b566f7A27B94566cac")),
				// Pre-funded accounts
				vec![
					AccountId::from(hex!("f24FF3a9CF04c71Dbc94D0b566f7A27B94566cac")),
					AccountId::from(hex!("7ed8c8a0C4d1FeA01275fE13F0Ef23bce5CBF8C3")),
					AccountId::from(hex!("3263236Cbc327B5519E373CC591318e56e7c5081")),
				],
				2000.into(),
			)
		},
		// Bootnodes
		vec![],
		// Telemetry
		None,
		// Protocol ID
		Some(PROTOCOL_ID),
		None,
		// Properties
		Some(chainspec_properties()),
		// Extensions
		Extensions {
			relay_chain: "rococo-local".into(),
			para_id: 2000
		},
	))
}

pub fn local_testnet_config() -> Result<ChainSpec, String> {

	Ok(ChainSpec::from_genesis(
		// Name
		"Local Testnet",
		// ID
		"local_testnet",
		ChainType::Local,
		move || {
			testnet_genesis(
				// Initial PoA authorities
				vec![
					(array_bytes::hex_n_into_unchecked(ALITH),
					 get_collator_keys_from_seed("Alice"),
					 ),
					(array_bytes::hex_n_into_unchecked(BALTATHAR),
					 get_collator_keys_from_seed("Bob"),
					 ),
				],
				// Sudo account
				AccountId::from(hex!("55D5E776997198679A8774507CaA4b0F7841767e")),
				// Pre-funded accounts
				vec![
					array_bytes::hex_n_into_unchecked(ALITH),
					array_bytes::hex_n_into_unchecked(BALTATHAR),
					array_bytes::hex_n_into_unchecked(CHARLETH),
					array_bytes::hex_n_into_unchecked(DOROTHY),
				],
				2000.into(),
			)
		},
		// Bootnodes
		vec![],
		// Telemetry
		None,
		// Protocol ID
		Some(PROTOCOL_ID),
		// Properties
		None,
		Some(chainspec_properties()),
		// Extensions
		Extensions {
			relay_chain: "rococo-local".into(),
			para_id: 2000,
		},
	))
}

/// Configure initial storage state for FRAME modules.
fn testnet_genesis(
	collators: Vec<(AccountId, AuraId)>,
	root_key: AccountId,
	endowed_accounts: Vec<AccountId>,
	id: ParaId
) -> GenesisConfig {
	const ENDOWMENT: Balance = 2_500_000_000 * STOR;
	GenesisConfig {
		// System config
		system: SystemConfig {
			// Add Wasm runtime to storage.
			code: WASM_BINARY.unwrap().to_vec(),
		},
		parachain_info: ParachainInfoConfig { parachain_id: id },
		parachain_system: Default::default(),

		// Monetary config
		balances: BalancesConfig {
			// Configure endowed accounts with initial balance of 1 << 60.
			balances: endowed_accounts.iter().cloned().map(|k| (k.clone(), ENDOWMENT / collators.len() as u128)).collect(),
		},
		transaction_payment: Default::default(),

		// consensus config
		aura: Default::default(),
		aura_ext: Default::default(),
		collator_selection: CollatorSelectionConfig {
			invulnerables: collators.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: EXISTENTIAL_DEPOSIT * 16,
			..Default::default()
		},
		session: SessionConfig {
			keys: collators
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),           // account id
						acc,                   // validator id
						session_keys(          // session keys
							aura,
						),
					)
				})
				.collect::<Vec<_>>(),
		},

		// Governance config
		treasury: Default::default(),

		// XCM config
		polkadot_xcm: PolkadotXcmConfig {
			safe_xcm_version: Some(SAFE_XCM_VERSION),
		},

		// Sudo config (will be removed in future updates)
		sudo: SudoConfig {
			// Assign network admin rights.
			key: Some(root_key),
		},

		// EVM config
		evm: EVMConfig {
			accounts: {
				let mut accounts = BTreeMap::new();
				accounts.insert(
					H160::from_slice(&hex!("6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b")),
					GenesisAccount {
						nonce: U256::zero(),
						balance: Default::default(),
						code: vec![],
						storage: BTreeMap::new(),
					},
				);
				accounts
			}
		},
		ethereum: Default::default(),
		base_fee: Default::default(),
	}
}
