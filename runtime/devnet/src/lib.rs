#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

mod weights;
pub mod xcm_config;
pub use fee::WeightToFee;

use codec::{Decode, Encode, MaxEncodedLen};
use cumulus_pallet_parachain_system::RelayNumberStrictlyIncreases;
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, ConstBool, OpaqueMetadata};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{AccountIdLookup, BlakeTwo256, Block as BlockT, Verify},
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult,
};

use sp_std::prelude::*;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

use frame_support::{
	construct_runtime,
	dispatch::DispatchClass,
	parameter_types,
	traits::{AsEnsureOriginWithArg, ConstU32, ConstU64, ConstU8, EitherOfDiverse, Everything},
	weights::{ConstantMultiplier, Weight},
	BoundedVec, PalletId,
};
use frame_system::{
	limits::{BlockLength, BlockWeights},
	EnsureRoot, EnsureSigned,
};
use pallet_nfts::PalletFeatures;
use pallet_xcm::{EnsureXcm, IsVoiceOfBody};
pub use runtime_common::{
	AVERAGE_ON_INITIALIZE_RATIO, DAYS, HOURS, MAXIMUM_BLOCK_WEIGHT, MILLISECS_PER_BLOCK, MINUTES,
	NORMAL_DISPATCH_RATIO, SLOT_DURATION,
};
pub use sp_consensus_aura::sr25519::AuthorityId as AuraId;
pub use sp_runtime::{MultiAddress, Perbill, Permill};
use xcm_config::{RelayLocation, XcmConfig, XcmOriginToTransactDispatchOrigin};

#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

// Polkadot imports
use polkadot_runtime_common::{BlockHashCount, SlowAdjustingFeeUpdate};

pub use runtime_common::{AccountId, Balance, BlockNumber, DealWithFees, Hash, Signature};

use weights::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight};

// XCM Imports
use runtime_common::Nonce;
use xcm::latest::prelude::BodyId;
use xcm_executor::XcmExecutor;

/// The address format for describing accounts.
pub type Address = MultiAddress<AccountId, ()>;

/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;

/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;

/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckEra<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;

/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, RuntimeCall, SignedExtra>;

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
>;

pub mod fee {
	use super::{Balance, ExtrinsicBaseWeight, MILLIMQTY};
	use frame_support::weights::{
		FeePolynomial, Weight, WeightToFeeCoefficient, WeightToFeeCoefficients,
		WeightToFeePolynomial,
	};
	use smallvec::smallvec;
	use sp_runtime::Perbill;

	/// Handles converting a weight scalar to a fee value, based on the scale and granularity of the
	/// node's balance type.
	///
	/// This should typically create a mapping between the following ranges:
	///   - `[0, MAXIMUM_BLOCK_WEIGHT]`
	///   - `[Balance::min, Balance::max]`
	///
	/// Yet, it can be used for any other sort of change to weight-fee. Some examples being:
	///   - Setting it to `0` will essentially disable the weight fee.
	///   - Setting it to `1` will cause the literal `#[weight = x]` values to be charged.
	pub struct WeightToFee;
	impl frame_support::weights::WeightToFee for WeightToFee {
		type Balance = Balance;

		fn weight_to_fee(weight: &Weight) -> Self::Balance {
			let ref_polynomial: FeePolynomial<Balance> = RefTimeToFee::polynomial().into();
			let proof_polynomial: FeePolynomial<Balance> = ProofSizeToFee::polynomial().into();

			// Get fee amount from ref_time based on the RefTime polynomial
			let ref_fee: Balance = ref_polynomial.eval(weight.ref_time());

			// Get fee amount from proof_size based on the ProofSize polynomial
			let proof_fee: Balance = proof_polynomial.eval(weight.proof_size());

			// Take the maximum instead of the sum to charge by the more scarce resource.
			ref_fee.max(proof_fee)
		}
	}

	/// Maps the Ref time component of `Weight` to a fee.
	pub struct RefTimeToFee;
	impl WeightToFeePolynomial for RefTimeToFee {
		type Balance = Balance;
		fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
			// in Rococo, extrinsic base weight (smallest non-zero weight) is mapped to 1 MILLIMQTY:
			// in our template, we map to 1/10 of that, or 1/10 MILLIMQTY
			let p = MILLIMQTY / 10;
			let q = 100 * Balance::from(ExtrinsicBaseWeight::get().ref_time());
			smallvec![WeightToFeeCoefficient {
				degree: 1,
				negative: false,
				coeff_frac: Perbill::from_rational(p % q, q),
				coeff_integer: p / q,
			}]
		}
	}

	/// Maps the proof size component of `Weight` to a fee.
	pub struct ProofSizeToFee;
	impl WeightToFeePolynomial for ProofSizeToFee {
		type Balance = Balance;
		fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
			// Map 10kb proof to 1 CENT.
			let p = MILLIMQTY / 10;
			let q = 10_000;

			smallvec![WeightToFeeCoefficient {
				degree: 1,
				negative: false,
				coeff_frac: Perbill::from_rational(p % q, q),
				coeff_integer: p / q,
			}]
		}
	}
}
/// Opaque types. These are used by the CLI to instantiate machinery that don't need to know
/// the specifics of the runtime. They can then be made to be agnostic over specific formats
/// of data like extrinsics, allowing for them to continue syncing the network through upgrades
/// to even the core data structures.
pub mod opaque {
	use super::*;
	use sp_runtime::{generic, traits::BlakeTwo256};

	pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;
	/// Opaque block header type.
	pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
	/// Opaque block type.
	pub type Block = generic::Block<Header, UncheckedExtrinsic>;
	/// Opaque block identifier type.
	pub type BlockId = generic::BlockId<Block>;
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub aura: Aura,
	}
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("metaquity-dev"),
	impl_name: create_runtime_str!("metaquity"),
	authoring_version: 1,
	spec_version: 100,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 1,
	state_version: 1,
};

pub const MICROMQTY: Balance = 1_000_000_000_000;
pub const MILLIMQTY: Balance = 1_000 * MICROMQTY;
// SBP-M1 review: is 14 decimal places intentional? 18 is specified at https://github.com/paritytech/ss58-registry/blob/main/ss58-registry.json#L882. Suggest setting UNITS/DOLLARS/MQTY to 18 decimal value and then divide accordingly for sub-units for clarity. Consider adding additional metadata in the chain_spec.rs as well - e.g. https://github.com/paritytech/extended-parachain-template/blob/3bec37d7844880d13e0a1f3253d1402500f83789/node/src/chain_spec.rs#L136
pub const MQTY: Balance = 1_000 * MILLIMQTY;

// SBP-M1 review: very small number for ED, especially for a chain with 18 decimals. Update to some fraction of a UNIT - e.g. https://github.com/paritytech/extended-parachain-template/blob/3bec37d7844880d13e0a1f3253d1402500f83789/runtime/mainnet/src/lib.rs#L220
// SBP-M1 review: see https://wiki.polkadot.network/docs/build-protocol-info#existential-deposit for more information
pub const EXISTENTIAL_DEPOSIT: Balance = MILLIMQTY;

pub const fn deposit(items: u32, bytes: u32) -> Balance {
	// SBP-M1 review: implementation may increase likelihood of chain storage bloat by returning a
	// relatively small value for a deposit, based on the number of decimals currently used on the
	// chain. SBP-M1 review: typical implementations include an additional multiplier. See deposit function implementations within runtimes at https://github.com/paritytech/polkadot-sdk/tree/master/polkadot/runtime and https://github.com/paritytech/extended-parachain-template/blob/main/runtime/mainnet/src/lib.rs#L223 as examples.
	(items as Balance * 20 * MQTY + (bytes as Balance) * 100 * MICROMQTY) / 100
}

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;

	// This part is copied from Substrate's `bin/node/runtime/src/lib.rs`.
	//  The `RuntimeBlockLength` and `RuntimeBlockWeights` exist here because the
	// `DeletionWeightLimit` and `DeletionQueueDepth` depend on those to parameterize
	// the lazy contract deletion.
	pub RuntimeBlockLength: BlockLength =
		BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
		.base_block(BlockExecutionWeight::get())
		.for_class(DispatchClass::all(), |weights| {
			weights.base_extrinsic = ExtrinsicBaseWeight::get();
		})
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Operational transactions have some extra reserved space, so that they
			// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
			weights.reserved = Some(
				MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
			);
		})
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
	pub MaxCollectivesProposalWeight: Weight = Perbill::from_percent(50) * RuntimeBlockWeights::get().max_block;
	pub const SS58Prefix: u16 = 666;
}

// Configure FRAME pallets to include in runtime.

// SBP-M1 review: consider matching member order with that of trait
impl frame_system::Config for Runtime {
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	/// The basic call filter to use in dispatchable.
	type BaseCallFilter = Everything;
	/// Block & extrinsics weights: base values and limits.
	type BlockWeights = RuntimeBlockWeights;
	/// The maximum length of a block (in bytes).
	type BlockLength = RuntimeBlockLength;
	/// The ubiquitous origin type.
	type RuntimeOrigin = RuntimeOrigin;
	/// The aggregated dispatch type that is available for extrinsics.
	type RuntimeCall = RuntimeCall;
	/// This stores the number of previous transactions associated with a sender account.
	type Nonce = Nonce;
	/// The type for hashing blocks and tries.
	type Hash = Hash;
	/// The hashing algorithm used.
	type Hashing = BlakeTwo256;
	/// The identifier used to distinguish between accounts.
	type AccountId = AccountId;
	/// The lookup mechanism to get account ID from whatever is passed in dispatchers.
	type Lookup = AccountIdLookup<AccountId, ()>;
	/// The Block type used by the runtime. This is used by `construct_runtime` to retrieve the
	/// extrinsics or other block specific data as needed.
	type Block = Block;
	/// Maximum number of block number to block hash mappings to keep (oldest pruned first).
	type BlockHashCount = BlockHashCount;
	/// The weight of database operations that the runtime can invoke.
	type DbWeight = RocksDbWeight;
	/// Runtime version.
	type Version = Version;
	/// Converts a module to an index of this module in the runtime.
	type PalletInfo = PalletInfo;
	/// The data to be stored in an account.
	type AccountData = pallet_balances::AccountData<Balance>;
	/// What to do if a new account is created.
	type OnNewAccount = ();
	/// What to do if an account is fully reaped from the system.
	type OnKilledAccount = ();
	/// Weight information for the extrinsics of this pallet.
	type SystemWeightInfo = ();
	/// This is used as an identifier of the chain. 42 is the generic substrate prefix.
	type SS58Prefix = SS58Prefix;
	/// The action to take on a Runtime Upgrade
	type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
	/// The maximum number of consumers allowed on a single account.
	// SBP-M1 review: unnecessary qualification
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
	type WeightInfo = ();
}

parameter_types! {
	pub const UncleGenerations: u32 = 0;
}

impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
	type EventHandler = (CollatorSelection,);
}

parameter_types! {
	pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
}

// SBP-M1 review: consider matching member order with that of trait
impl pallet_balances::Config for Runtime {
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
	/// The type for recording an account's balance.
	type Balance = Balance;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type FreezeIdentifier = ();
	type MaxLocks = ConstU32<50>;
	type MaxReserves = ConstU32<50>;
	// SBP-M1 review: add comment noting why this is set to one - i.e.
	// HoldReason::NftFractionalization
	type MaxHolds = ConstU32<1>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	// SBP-M1 review: re-consider value after adjusting units mentioned above.
	pub const AssetDeposit: Balance = 10 * MQTY;
	pub const AssetAccountDeposit: Balance = deposit(1, 16);
	// SBP-M1 review: prefer inlining if type only used once - e.g. ConstU128. Also re-consider value after adjusting units mentioned above.
	pub const ApprovalDeposit: Balance = EXISTENTIAL_DEPOSIT;
	pub const StringLimit: u32 = 50;
	// SBP-M1 review: prefer inlining if type only used once - e.g. ConstU128. Also re-consider value after adjusting units mentioned above.
	pub const MetadataDepositBase: Balance = deposit(1, 68);
	// SBP-M1 review: prefer inlining if type only used once - e.g. ConstU128. Also re-consider value after adjusting units mentioned above.
	pub const MetadataDepositPerByte: Balance = deposit(0, 1);
}

// SBP-M1 review: consider matching member order with that of trait
impl pallet_assets::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	// SBP-M1 review: reuse Balance type rather than explicit u128?
	type Balance = Balance;
	// SBP-M1 review: use separator for consistency - i.e. 1_000
	type RemoveItemsLimit = ConstU32<1000>;
	type AssetId = u32;
	type AssetIdParameter = codec::Compact<u32>;
	type Currency = Balances;
	// SBP-M1 review: consider whether anyone should be able to permissionlessly create an asset -
	// should probably be set to MQTY admin origin only.
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	// SBP-M1 review: may need to be root or MQTY admin origin to allow force_set_metadata for
	// fractionalised assets - see EitherOf<L, R>.
	type ForceOrigin = EnsureRoot<AccountId>;
	type AssetDeposit = AssetDeposit;
	// SBP-M1 review: re-consider this after adjusting the units mentioned above
	type AssetAccountDeposit = AssetAccountDeposit;
	type MetadataDepositBase = MetadataDepositBase;
	type MetadataDepositPerByte = MetadataDepositPerByte;
	type ApprovalDeposit = ApprovalDeposit;
	type StringLimit = StringLimit;
	type Freezer = ();
	type Extra = ();
	type CallbackHandle = ();
	type WeightInfo = pallet_assets::weights::SubstrateWeight<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

parameter_types! {
	/// Relay Chain `TransactionByteFee` / 10
	pub const TransactionByteFee: Balance = 10 * MICROMQTY;
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	// SBP-M1 review: consider a mechanism for dealing with transaction fees - e.g. DealWithFees
	type OnChargeTransaction =
		pallet_transaction_payment::CurrencyAdapter<Balances, DealWithFees<Runtime>>;
	type OperationalFeeMultiplier = ConstU8<5>;
	// SBP-M1 review: consider non-default weight to fee mechanisms - e.g. https://github.com/paritytech/extended-parachain-template/blob/3bec37d7844880d13e0a1f3253d1402500f83789/runtime/mainnet/src/lib.rs#L126
	type WeightToFee = WeightToFee;
	// SBP-M1 review: consider non-default length to fee mechanisms
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	// SBP-M1 review: consider non-default fee multiplier update mechanisms - e.g.
	// SlowAdjustingFeeUpdate
	type FeeMultiplierUpdate = SlowAdjustingFeeUpdate<Self>;
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;

	type WeightInfo = pallet_utility::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
	pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
	pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
}

impl cumulus_pallet_parachain_system::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = ();
	type SelfParaId = parachain_info::Pallet<Runtime>;
	type OutboundXcmpMessageSource = XcmpQueue;
	type DmpMessageHandler = DmpQueue;
	type ReservedDmpWeight = ReservedDmpWeight;
	type XcmpMessageHandler = XcmpQueue;
	type ReservedXcmpWeight = ReservedXcmpWeight;
	type CheckAssociatedRelayNumber = RelayNumberStrictlyIncreases;
}

impl parachain_info::Config for Runtime {}

impl cumulus_pallet_aura_ext::Config for Runtime {}

impl cumulus_pallet_xcmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type ChannelInfo = ParachainSystem;
	type VersionWrapper = ();
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
	type ControllerOrigin = EnsureRoot<AccountId>;
	type ControllerOriginConverter = XcmOriginToTransactDispatchOrigin;
	type PriceForSiblingDelivery = ();
	type WeightInfo = ();
}

impl cumulus_pallet_dmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
}

parameter_types! {
	pub const Period: u32 = 6 * HOURS;
	pub const Offset: u32 = 0;
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	// we don't have stash and controller, thus we don't need the convert as well.
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
	type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
	type SessionManager = CollatorSelection;
	// Essentially just Aura, but lets be pedantic.
	type SessionHandler = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type WeightInfo = ();
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type WeightInfo = pallet_sudo::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
	pub const CouncilMotionDuration: BlockNumber = 7 * DAYS;
	pub const CouncilMaxProposals: u32 = 10;
	pub const CouncilMaxMembers: u32 = 25;
}

type CouncilCollective = pallet_collective::Instance1;
impl pallet_collective::Config<CouncilCollective> for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type Proposal = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type MotionDuration = CouncilMotionDuration;
	type MaxProposals = CouncilMaxProposals;
	type MaxMembers = CouncilMaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type WeightInfo = pallet_collective::weights::SubstrateWeight<Runtime>;
	type SetMembersOrigin = EnsureRoot<AccountId>;
	type MaxProposalWeight = MaxCollectivesProposalWeight;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = deposit(0, 32);
}

impl pallet_multisig::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = ConstU32<100>;
	type WeightInfo = pallet_multisig::weights::SubstrateWeight<Runtime>;
}

// SBP-M1 review: consider matching member order with that of trait
impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type MaxAuthorities = ConstU32<100_000>;
	type DisabledValidators = ();
	type AllowMultipleBlocksPerSlot = ConstBool<false>;
}

parameter_types! {
	pub const PotId: PalletId = PalletId(*b"PotStake");
	pub const MaxCandidates: u32 = 1000;
	pub const MinEligibleCollators: u32 = 5;
	pub const SessionLength: BlockNumber = 6 * HOURS;
	pub const MaxInvulnerables: u32 = 100;
	// StakingAdmin pluralistic body.
	pub const StakingAdminBodyId: BodyId = BodyId::Defense;
}

/// We allow root and the StakingAdmin to execute privileged collator selection operations.
pub type CollatorSelectionUpdateOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	EnsureXcm<IsVoiceOfBody<RelayLocation, StakingAdminBodyId>>,
>;

impl pallet_collator_selection::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type UpdateOrigin = CollatorSelectionUpdateOrigin;
	type PotId = PotId;
	type MaxCandidates = MaxCandidates;
	type MinEligibleCollators = MinEligibleCollators;
	type MaxInvulnerables = MaxInvulnerables;
	// should be a multiple of session or things will get inconsistent
	type KickThreshold = Period;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ValidatorRegistration = Session;
	type WeightInfo = ();
}

parameter_types! {
	// Minimum 100bytes
	// SBP-M1 review: consider using deposit() function for consistency with other pallets deposits (and as per Polkadot runtime).
	// @khssnv: ToDo: adjust
	pub const BasicDeposit: Balance = 10 * MQTY;		//258 bytes on-chain
	// SBP-M1 review: consider using deposit() function for consistency with other pallets deposits (and as per Polkadot runtime).
	// @khssnv: ToDo: adjust
	pub const FieldDeposit: Balance = 2 * MQTY;			//66 bytes on-chain
	// SBP-M1 review: consider using deposit() function for consistency with other pallets deposits (and as per Polkadot runtime).
	// @khssnv: ToDo: adjust
	pub const SubAccountDeposit: Balance = 2 * MQTY;	// 53 bytes on-chain
	pub const MaxAdditionalFields: u32 = 100;
	pub const MaxSubAccounts: u32 = 100;
	pub const MaxRegistrars: u32= 20;
}

impl pallet_identity::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type BasicDeposit = BasicDeposit;
	type FieldDeposit = FieldDeposit;
	type SubAccountDeposit = SubAccountDeposit;
	type MaxSubAccounts = MaxSubAccounts;
	type MaxAdditionalFields = MaxAdditionalFields;
	type MaxRegistrars = MaxRegistrars;
	// SBP-M1 review: consider what happens with slashed funds - e.g. treasury
	type Slashed = ();
	// SBP-M1 review: should be EnsureRoot
	type ForceOrigin = EnsureSigned<Self::AccountId>;
	// SBP-M1 review: should be EnsureRoot or MQTY admin origin to maintain registrar integrity
	type RegistrarOrigin = EnsureSigned<Self::AccountId>;
	type WeightInfo = ();
}

// SBP-M1 review: pallet_uniques is not used, so these parameters can be removed, with values moved
// to Nfts* parameter types
parameter_types! {
	// SBP-M1 review: UNITS is 1, resulting in deposit of zero. This needs to be fixed.
	// SBP-M1 review: only used once, move implementation to usage to remove this parameter
	pub const UniquesCollectionDeposit: Balance = MQTY / 10;
	// SBP-M1 review: UNITS is 1, resulting in deposit of zero. This needs to be fixed.
	// SBP-M1 review: only used once, move implementation to usage to remove this parameter
	pub const UniquesItemDeposit: Balance = MQTY / 1_000;
	// SBP-M1 review: only used once, move implementation to usage to remove this parameter
	// SBP-M1 review: provide justification as to how 129 is determined. I do see that it is configured this way on Asset Hub on Polkadot/Kusama though.
	pub const UniquesMetadataDepositsBase: Balance = deposit(1, 129);
	// SBP-M1 review: only used once, move implementation to usage to remove this parameter
	// SBP-M1 review: provide justification as to how 129 is determined. Asset Hub on Polkadot/Kusama has this configured as deposit(1, 0).
	pub const UniquesAttributeDepositsBase: Balance = deposit(1, 129);
	// SBP-M1 review: only used once, move implementation to usage to remove this parameter
	pub const UniquesDepositPerByte: Balance = deposit(0, 1);
}

parameter_types! {
	pub NftsPalletFeatures: PalletFeatures = PalletFeatures::all_enabled();
	// SBP-M1 review: only used once, inline value via ConstU32 to remove this parameter
	pub const NftsMaxDeadlineDuration: BlockNumber = 12 * 30 * DAYS;

	// SBP-M1 review: typo
	// reuse the unique deopsits
	// SBP-M1 review: move impls from above for each of the following to eliminate Uniques* parameter types above
	pub const NftsCollectionDeposit: Balance = UniquesCollectionDeposit::get();
	pub const NftsItemDeposit: Balance = UniquesItemDeposit::get();
	pub const NftsMetadataDepositsBase: Balance = UniquesMetadataDepositsBase::get();
	pub const NftsAttributeDepositsBase: Balance = UniquesAttributeDepositsBase::get();
	pub const NftsDepositPerByte: Balance = UniquesDepositPerByte::get();
}

// SBP-M1 review: consider matching member order with that of trait
impl pallet_nfts::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CollectionId = u32;
	type ItemId = u32;
	type Currency = Balances;
	// SBP-M1 review: consider whether any user with access to public chain should be able to
	// permissionlessly create collections, which is currently the case here. The use-case/UI
	// screenshots imply that asset verification is required, so assume the onchain creation of
	// collections should only be carried out by MQTY admin (e.g. configure CreateOrigin as MQTY
	// admin) and then assets (NFTs) minted by the collection admin once verified. SBP-M1 review:
	// unnecessary qualification (frame_system::)
	type CreateOrigin = AsEnsureOriginWithArg<frame_system::EnsureSigned<Self::AccountId>>;
	// SBP-M1 review: unnecessary qualification (frame_system::)
	type ForceOrigin = frame_system::EnsureRoot<Self::AccountId>;
	type Locker = ();
	type CollectionDeposit = NftsCollectionDeposit;
	type ItemDeposit = NftsItemDeposit;
	type MetadataDepositBase = NftsMetadataDepositsBase;
	type AttributeDepositBase = NftsAttributeDepositsBase;
	type DepositPerByte = NftsDepositPerByte;
	type StringLimit = ConstU32<50>;
	type KeyLimit = ConstU32<50>;
	type ValueLimit = ConstU32<50>;
	type ApprovalsLimit = ConstU32<10>;
	type ItemAttributesApprovalsLimit = ConstU32<2>;
	type MaxTips = ConstU32<10>;
	type MaxDeadlineDuration = NftsMaxDeadlineDuration;
	type MaxAttributesPerCall = ConstU32<2>;
	type Features = NftsPalletFeatures;
	/// Off-chain = signature On-chain - therefore no conversion needed.
	/// It needs to be From<MultiSignature> for benchmarking.
	type OffchainSignature = Signature;
	type OffchainPublic = <Signature as Verify>::Signer;
	type WeightInfo = ();
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
}

/// A reason for placing a hold on funds.
#[derive(
	Copy,
	Clone,
	Eq,
	PartialEq,
	Ord,
	PartialOrd,
	Encode,
	Decode,
	MaxEncodedLen,
	Debug,
	scale_info::TypeInfo,
)]
pub enum HoldReason {
	/// Used by the NFT Fractionalization Pallet.
	NftFractionalization,
}

parameter_types! {
	pub const NftFractionalizationPalletId: PalletId = PalletId(*b"fraction");
	// SBP-M1 review: consider BoundedVec::unchecked_from() or use .expect("reason") rather than .unwrap(). I see the Asset Hub on Kusama is configured this way though.
	pub NewAssetSymbol: BoundedVec<u8, StringLimit> = (*b"FRAC").to_vec().try_into().unwrap();
	// SBP-M1 review: consider something more informative like 'Fractionalized Asset'. May not matter though, as it will probably require an assets::force_set_metadata to customise the fractionalized asset metadata after the NFT has been fractionalized.
	pub NewAssetName: BoundedVec<u8, StringLimit> = (*b"Frac").to_vec().try_into().unwrap();
}

// SBP-M1 review: consider matching member order with that of trait
impl pallet_nft_fractionalization::Config for Runtime {
	// SBP-M1 review: whilst it resolves to the same type, consider using <Self as
	// pallet_assets::Config>::Balance as it would better align with AssetId and Assets type
	// definitions below. I see the Asset Hub on Kusama is configured this way though.
	type AssetBalance = <Self as pallet_balances::Config>::Balance;
	type AssetId = <Self as pallet_assets::Config>::AssetId;
	type Assets = Assets;
	type Currency = Balances;
	// SBP-M1 review: uses AssetDeposit rather than NftsCollectionDeposit and cannot determine
	// whether this is intentional. I see the Asset Hub on Kusama is configured this way though.
	// Suggest adding NftFractionalizationDeposit alias to AssetDeposit or NftsCollectionDeposit
	// with a comment as to why it is being used in your runtime for clarity.
	type Deposit = AssetDeposit;
	type NewAssetName = NewAssetName;
	type NewAssetSymbol = NewAssetSymbol;
	type NftCollectionId = <Self as pallet_nfts::Config>::CollectionId;
	type NftId = <Self as pallet_nfts::Config>::ItemId;
	type Nfts = Nfts;
	type PalletId = NftFractionalizationPalletId;
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
	type StringLimit = StringLimit;
	type WeightInfo = pallet_nft_fractionalization::weights::SubstrateWeight<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

// Create the runtime by composing the FRAME pallets that were previously configured.
construct_runtime!(
	pub struct Runtime {
		// System support stuff.
		// SBP-M1 review: explicit pallet indices preferred - e.g. https://github.com/paritytech/extended-parachain-template/blob/3bec37d7844880d13e0a1f3253d1402500f83789/runtime/mainnet/src/lib.rs#L564
		System: frame_system = 0,
		ParachainSystem: cumulus_pallet_parachain_system = 1,
		Timestamp: pallet_timestamp = 2,
		ParachainInfo: parachain_info = 3,

		// Utility
		Utility: pallet_utility = 4,
		Multisig: pallet_multisig = 5,

		// Monetary stuff.
		Balances: pallet_balances = 10,
		TransactionPayment: pallet_transaction_payment = 11,
		Assets: pallet_assets = 12,

		// Governance
		Sudo: pallet_sudo = 15,
		Council: pallet_collective::<Instance1> = 16,

		// Collator support. The order of these 4 are important and shall not change.
		Authorship: pallet_authorship = 20,
		CollatorSelection: pallet_collator_selection = 21,
		Session: pallet_session = 22,
		Aura: pallet_aura = 23,
		AuraExt: cumulus_pallet_aura_ext = 24,

		// XCM helpers.
		XcmpQueue: cumulus_pallet_xcmp_queue = 30,
		PolkadotXcm: pallet_xcm = 31,
		CumulusXcm: cumulus_pallet_xcm = 32,
		DmpQueue: cumulus_pallet_dmp_queue = 33,

		// Less simple identity module.
		Identity: pallet_identity = 40,

		// NFT modules.
		Nfts: pallet_nfts = 50,
		NftFractionalization: pallet_nft_fractionalization = 51,
	}
);

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	frame_benchmarking::define_benchmarks!(
		[frame_system, SystemBench::<Runtime>]
		[pallet_balances, Balances]
		[pallet_assets, Assets]
		[pallet_session, SessionBench::<Runtime>]
		[pallet_timestamp, Timestamp]
		[pallet_collator_selection, CollatorSelection]
		[pallet_multisig, Multisig]
		[cumulus_pallet_xcmp_queue, XcmpQueue]
		// SBP-M1 review: add missing pallets: benchmarks should be re-run on reference hardware based on how they are configured/used by your runtime
	);
}

impl_runtime_apis! {
	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(Aura::slot_duration())
		}

		fn authorities() -> Vec<AuraId> {
			Aura::authorities().into_inner()
		}
	}

	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}
		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> sp_std::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
		for Runtime
	{
		fn query_call_info(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_call_info(call, len)
		}
		fn query_call_fee_details(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_call_fee_details(call, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
		fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
			ParachainSystem::collect_collation_info(header)
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect,
		) -> Weight {
			Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;
			use frame_system_benchmarking::Pallet as SystemBench;
			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			(list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch};

			use frame_system_benchmarking::Pallet as SystemBench;
			impl frame_system_benchmarking::Config for Runtime {}

			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
			impl cumulus_pallet_session_benchmarking::Config for Runtime {}

			use frame_support::traits::WhitelistedStorageKeys;
			let whitelist = AllPalletsWithSystem::whitelisted_storage_keys();

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);

			if batches.is_empty() { return Err("Benchmark not found for this pallet.".into()) }
			Ok(batches)
		}
	}
}

struct CheckInherents;

impl cumulus_pallet_parachain_system::CheckInherents<Block> for CheckInherents {
	fn check_inherents(
		block: &Block,
		relay_state_proof: &cumulus_pallet_parachain_system::RelayChainStateProof,
	) -> sp_inherents::CheckInherentsResult {
		let relay_chain_slot = relay_state_proof
			.read_slot()
			.expect("Could not read the relay chain slot from the proof");

		let inherent_data =
			cumulus_primitives_timestamp::InherentDataProvider::from_relay_chain_slot_and_duration(
				relay_chain_slot,
				sp_std::time::Duration::from_secs(6),
			)
			.create_inherent_data()
			.expect("Could not create the timestamp inherent data");

		inherent_data.check_extrinsics(block)
	}
}

cumulus_pallet_parachain_system::register_validate_block! {
	Runtime = Runtime,
	BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
	CheckInherents = CheckInherents,
}
