//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

#![warn(unused_extern_crates)]

use std::sync::Arc;
use transaction_pool::{self, txpool::{Pool as TransactionPool}};
use cennznet_runtime::{GenesisConfig, RuntimeApi};
use cennznet_primitives::Block;
use substrate_service::{
	FactoryFullConfiguration, LightComponents, FullComponents, FullBackend,
	FullClient, LightClient, LightBackend, FullExecutor, LightExecutor, TaskExecutor
};
use consensus::{import_queue, start_aura, Config as AuraConfig, AuraImportQueue, NothingExtra};
use consensus_common::offline_tracker::OfflineTracker;
use primitives::ed25519::Pair;
use client;
use std::time::Duration;
use parking_lot::RwLock;
use grandpa;

pub use substrate_executor::NativeExecutor;
native_executor_instance!(
	pub Executor,
	cennznet_runtime::api::dispatch,
	cennznet_runtime::native_version,
	include_bytes!("../runtime/wasm/target/wasm32-unknown-unknown/release/cennznet_runtime.compact.wasm")
);

const AURA_SLOT_DURATION: u64 = 6;

construct_simple_protocol! {
	/// Demo protocol attachment for substrate.
	pub struct NodeProtocol where Block = Block { }
}

/// Node specific configuration
pub struct NodeConfig<F: substrate_service::ServiceFactory> {
	/// should run as a grandpa authority
	pub grandpa_authority: bool,
	/// should run as a grandpa authority only, don't validate as usual
	pub grandpa_authority_only: bool,
	/// grandpa connection to import block

	// FIXME: rather than putting this on the config, let's have an actual intermediate setup state
	// https://github.com/paritytech/substrate/issues/1134
	pub grandpa_import_setup: Option<(Arc<grandpa::BlockImportForService<F>>, grandpa::LinkHalfForService<F>)>,
}

impl<F> Default for NodeConfig<F> where F: substrate_service::ServiceFactory {
	fn default() -> NodeConfig<F> {
		NodeConfig {
			grandpa_authority: false,
			grandpa_authority_only: false,
			grandpa_import_setup: None,
		}
	}
}

construct_service_factory! {
	struct Factory {
		Block = Block,
		RuntimeApi = RuntimeApi,
		NetworkProtocol = NodeProtocol { |config| Ok(NodeProtocol::new()) },
		RuntimeDispatch = Executor,
		FullTransactionPoolApi = transaction_pool::ChainApi<client::Client<FullBackend<Self>, FullExecutor<Self>, Block, RuntimeApi>, Block>
			{ |config, client| Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client))) },
		LightTransactionPoolApi = transaction_pool::ChainApi<client::Client<LightBackend<Self>, LightExecutor<Self>, Block, RuntimeApi>, Block>
			{ |config, client| Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client))) },
		Genesis = GenesisConfig,
		Configuration = NodeConfig<Self>,
		FullService = FullComponents<Self>
			{ |config: FactoryFullConfiguration<Self>, executor: TaskExecutor|
				FullComponents::<Factory>::new(config, executor) },
		AuthoritySetup = {
			|mut service: Self::FullService, executor: TaskExecutor, key: Arc<Pair>| {
				let (block_import, link_half) = service.config.custom.grandpa_import_setup.take()
					.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

				if service.config.custom.grandpa_authority {
					info!("Running Grandpa session as Authority {}", key.public());
					let (voter, oracle) = grandpa::run_grandpa(
						grandpa::Config {
							gossip_duration: Duration::new(4, 0), // FIXME: make this available through chainspec?
							local_key: Some(key.clone()),
							name: Some(service.config.name.clone())
						},
						link_half,
						grandpa::NetworkBridge::new(service.network()),
					)?;

					executor.spawn(oracle);
					executor.spawn(voter);
				}
				if !service.config.custom.grandpa_authority_only {
					info!("Using authority key {}", key.public());
					let proposer = Arc::new(substrate_service::ProposerFactory {
						client: service.client(),
						transaction_pool: service.transaction_pool(),
						offline: Arc::new(RwLock::new(OfflineTracker::new())),
						force_delay: 0 // FIXME: allow this to be configured https://github.com/paritytech/substrate/issues/1170
					});
					executor.spawn(start_aura(
						AuraConfig {
							local_key: Some(key),
							slot_duration: AURA_SLOT_DURATION,
						},
						service.client(),
						block_import.clone(),
						proposer,
						service.network(),
					));
				}
				Ok(service)
			}
		},
		LightService = LightComponents<Self>
			{ |config, executor| <LightComponents<Factory>>::new(config, executor) },
		FullImportQueue = AuraImportQueue<Self::Block, grandpa::BlockImportForService<Self>, NothingExtra>
			{ |config: &mut FactoryFullConfiguration<Self> , client: Arc<FullClient<Self>>| {
				let (block_import, link_half) = grandpa::block_import::<_, _, _, RuntimeApi, FullClient<Self>>(client.clone(), client)?;
				let block_import = Arc::new(block_import);

				config.custom.grandpa_import_setup = Some((block_import.clone(), link_half));

				Ok(import_queue(
					AuraConfig {
						local_key: None,
						slot_duration: 5
					},
					block_import,
					NothingExtra,
				))
			}},
		LightImportQueue = AuraImportQueue<Self::Block, LightClient<Self>, NothingExtra>
			{ |ref mut config, client| Ok(
				import_queue(AuraConfig {
					local_key: None,
					slot_duration: 5
				},
				client,
				NothingExtra,
			))
			},
	}
}


#[cfg(test)]
mod tests {
	#[cfg(feature = "rhd")]
	fn test_sync() {
		use {service_test, Factory};
		use client::{ImportBlock, BlockOrigin};

		let alice: Arc<ed25519::Pair> = Arc::new(Keyring::Alice.into());
		let bob: Arc<ed25519::Pair> = Arc::new(Keyring::Bob.into());
		let validators = vec![alice.public().0.into(), bob.public().0.into()];
		let keys: Vec<&ed25519::Pair> = vec![&*alice, &*bob];
		let offline = Arc::new(RwLock::new(OfflineTracker::new()));
		let dummy_runtime = ::tokio::runtime::Runtime::new().unwrap();
		let block_factory = |service: &<Factory as service::ServiceFactory>::FullService| {
			let block_id = BlockId::number(service.client().info().unwrap().chain.best_number);
			let parent_header = service.client().header(&block_id).unwrap().unwrap();
			let consensus_net = ConsensusNetwork::new(service.network(), service.client().clone());
			let proposer_factory = consensus::ProposerFactory {
				client: service.client().clone(),
				transaction_pool: service.transaction_pool().clone(),
				network: consensus_net,
				offline: offline.clone(),
				force_delay: 0,
				handle: dummy_runtime.executor(),
			};
			let (proposer, _, _) = proposer_factory.init(&parent_header, &validators, alice.clone()).unwrap();
			let block = proposer.propose().expect("Error making test block");
			ImportBlock {
				origin: BlockOrigin::File,
				justification: Vec::new(),
				internal_justification: Vec::new(),
				finalized: true,
				body: Some(block.extrinsics),
				header: block.header,
				auxiliary: Vec::new(),
			}
		};
		let extrinsic_factory = |service: &<Factory as service::ServiceFactory>::FullService| {
			let payload = (0, Call::Balances(BalancesCall::transfer(RawAddress::Id(bob.public().0.into()), 69.into())), Era::immortal(), service.client().genesis_hash());
			let signature = alice.sign(&payload.encode()).into();
			let id = alice.public().0.into();
			let xt = UncheckedExtrinsic {
				signature: Some((RawAddress::Id(id), signature, payload.0, Era::immortal())),
				function: payload.1,
			}.encode();
			let v: Vec<u8> = Decode::decode(&mut xt.as_slice()).unwrap();
			OpaqueExtrinsic(v)
		};
		service_test::sync::<Factory, _, _>(chain_spec::integration_test_config(), block_factory, extrinsic_factory);
	}

}
