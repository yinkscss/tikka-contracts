#![no_std]
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractimpl, contracttype, token,
    xdr::ToXdr,
    Address, Bytes, BytesN, Env, IntoVal, String, Symbol, Val, Vec,
};

mod admin;
mod draw;
mod events;
mod helpers;
mod randomness;
mod tickets;

use raffle_shared::{
    CancelReason, FailureReason, FairnessData, RaffleConfig, RaffleStatus, RandomnessSource,
    RandomnessType, Ticket,
};

use self::randomness::{
    build_vrf_proof_message, OracleSeedWinnerSelection, WinnerSelectionStrategy,
};

use crate::events::{
    ContractPaused, ContractUnpaused, DrawTriggered, EmergencyWithdrawn, FeesWithdrawn,
    OracleAddressUpdated, PrizeClaimed, PrizeDeposited, PrizeRefunded, ProtocolFeeUpdated,
    RaffleCancelled, RaffleCreated, RaffleFailed, RaffleFinalized, RaffleStatusChanged,
    RandomnessFallbackTriggered, RandomnessReceived, RandomnessRequested, SwapDeadlineUpdated,
    TicketNftMinted, TicketPurchased, TicketRefunded, TicketSalesPaused, TicketSalesResumed,
    TokensRescued, WinnerDrawn,
};

const ORACLE_TIMEOUT_LEDGERS: u32 = 200;
const RANDOMNESS_MIN_DELAY_LEDGERS: u32 = 10;
pub const MAX_DESCRIPTION_LENGTH: u32 = 1000;
pub const MAX_TICKETS_LIMIT: u32 = 100_000;
pub const MAX_PRIZES: u32 = 100;
pub const MIN_TICKET_PRICE: i128 = 10_000;
pub const MAX_PRIZE_AMOUNT: i128 = 1_000_000_000_000_000_000_000;
pub const DEFAULT_CLAIM_LOCKUP_SECONDS: u64 = 3_600;
pub const MAX_CLAIM_LOCKUP_SECONDS: u64 = 604_800;
pub const DEFAULT_SWAP_DEADLINE_SECONDS: u64 = 300;
pub const MAX_SWAP_DEADLINE_SECONDS: u64 = 3_600;
pub const EMERGENCY_WITHDRAW_DELAY_SECONDS: u64 = 90 * 24 * 3600;
pub const MAX_PROTOCOL_FEE_BP: u32 = 2_000;

#[contract]
pub struct Contract;

#[contracttype]
#[derive(Clone)]
pub struct Raffle {
    pub creator: Address,
    pub description: String,
    pub end_time: u64,
    pub no_deadline: bool,
    pub max_tickets: u32,
    pub max_tickets_per_tx: u32,
    pub min_tickets: u32,
    pub allow_multiple: bool,
    pub ticket_price: i128,
    pub payment_token: Address,
    /// The token used for prize deposit and claims.
    /// Defaults to `payment_token` when not explicitly set by the creator.
    pub prize_token: Address,
    pub prize_amount: i128,
    pub prizes: Vec<u32>,
    pub tickets_sold: u32,
    pub status: RaffleStatus,
    pub prize_deposited: bool,
    pub winners: Vec<Address>,
    pub claimed_winners: Vec<bool>,
    pub randomness_source: RandomnessSource,
    pub oracle_address: Option<Address>,
    pub protocol_fee_bp: u32,
    pub treasury_address: Option<Address>,
    pub swap_router: Option<Address>,
    pub tikka_token: Option<Address>,
    pub finalized_at: Option<u64>,
    pub claim_lockup_seconds: u64,
    pub swap_deadline_seconds: u64,
    pub ticket_sales_paused: bool,
    /// The percentage of max_tickets covered by the early bird discount (0 to disable).
    pub early_bird_ticket_percentage: u32,
    /// The discount amount specified in basis points.
    pub early_bird_discount_bp: u32,
}

#[contracttype]
#[derive(Clone)]
pub struct FairnessMetadata {
    pub seed: u64,
    pub randomness_source: RandomnessSource,
    pub winning_ticket_indices: Vec<u32>,
    pub draw_timestamp: u64,
    pub draw_sequence: u32,
}

#[soroban_sdk::contracttype]
#[derive(Clone)]
pub enum DataKey {
    Raffle,
    TicketCount(Address),
    Ticket(u32),
    TicketRefunded(u32),
    Factory,
    ReentrancyGuard,
    Paused,
    Admin,
    RandomnessSeed,
    RandomnessRequested,
    RandomnessRequestLedger,
    RandomnessRequestId,
    FinishTime,
    AccumulatedFees,
    CommitEntry(u32),
    DrawingLock,
    TicketBuyers,
    /// Per-owner ticket ID index: owner Address → Vec<u32> of ticket IDs.
    /// Appended to on every successful ticket purchase, allowing O(1) owner
    /// lookups without scanning the full ticket space.
    OwnerTickets(Address),
    /// Admin-cancel timelock unlock timestamp (unix seconds).
    PendingAdminCancel,
}

#[contracttype]
#[derive(Clone)]
pub struct CommitRevealEntry {
    pub committer: Address,
    pub hash: BytesN<32>,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    RaffleNotFound = 1,
    RaffleInactive = 2,
    TicketsSoldOut = 3,
    InsufficientFunds = 4,
    NotAuthorized = 5,
    OracleNotSet = 6,
    RandomnessAlreadyRequested = 7,
    NoRandomnessRequest = 8,
    FallbackTooEarly = 9,
    PrizeNotDeposited = 11,
    PrizeAlreadyClaimed = 12,
    PrizeAlreadyDeposited = 13,
    NotWinner = 14,
    ClaimTooEarly = 15,
    InvalidParameters = 21,
    InvalidQuantity = 22,
    InvalidStatus = 23,
    ContractPaused = 24,
    InvalidStateTransition = 25,
    RaffleExpired = 26,
    InsufficientTickets = 31,
    MultipleTicketsNotAllowed = 32,
    NoTicketsSold = 33,
    TicketNotFound = 34,
    RaffleEnded = 35,
    ArithmeticOverflow = 41,
    AlreadyInitialized = 42,
    NotInitialized = 43,
    Reentrancy = 44,
    TokenTransferFailed = 45,
    NoActiveTickets = 46,
    DeadlinePassed = 47,
    SlippageExceeded = 48,
    InvalidIndex = 49,
    MorePrizesThanTickets = 50,
    ZeroPrize = 51,
    InvalidTokenAddress = 52,
    TooManyPrizes = 53,
    EmergencyTooEarly = 54,
    InvalidTicketRange = 55,
    InsufficientAccumulatedFees = 56,
    PrizeConfigurationLocked = 57,
    ExceedsMaxTicketsPerTx = 58,
    DrawingAlreadyInProgress = 59,
    InvalidStatusForDrawingTransition = 60,
    DrawingAlreadyComplete = 61,
    InvalidEndTime = 62,
    InvalidAdminAddress = 63,
    RandomnessTooEarly = 64,
}

fn read_raffle(env: &Env) -> Result<Raffle, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Raffle)
        .ok_or(Error::NotInitialized)
}

fn write_raffle(env: &Env, raffle: &Raffle) {
    env.storage().instance().set(&DataKey::Raffle, raffle);
}

raffle_shared::impl_require_admin!(Error, Error::NotAuthorized);

fn get_ticket_owner(env: &Env, ticket_id: u32) -> Option<Address> {
    env.storage()
        .persistent()
        .get::<_, Ticket>(&DataKey::Ticket(ticket_id))
        .map(|t| t.owner)
}

fn acquire_guard(env: &Env) -> Result<(), Error> {
    if env.storage().instance().has(&DataKey::ReentrancyGuard) {
        return Err(Error::Reentrancy);
    }
    env.storage()
        .instance()
        .set(&DataKey::ReentrancyGuard, &true);
    Ok(())
}

// Helper to enforce slippage and deadline guards for token swaps
// Uses the raffle's configurable swap_deadline_seconds to calculate the deadline
#[allow(dead_code)]
fn enforce_swap_guard(
    env: &Env,
    raffle: &Raffle,
    amount_out: i128,
    min_amount_out: i128,
) -> Result<(), Error> {
    // Calculate deadline based on current timestamp and raffle's configured deadline window
    let deadline = env.ledger().timestamp() + raffle.swap_deadline_seconds;

    // Check deadline
    if env.ledger().timestamp() > deadline {
        return Err(Error::DeadlinePassed);
    }
    // Check slippage (amount_out must be >= min_amount_out)
    if amount_out < min_amount_out {
        return Err(Error::SlippageExceeded);
    }
    Ok(())
}

fn release_guard(env: &Env) {
    env.storage().instance().remove(&DataKey::ReentrancyGuard);
}

struct Guard<'a> {
    env: &'a Env,
}

impl<'a> Guard<'a> {
    fn new(env: &'a Env) -> Result<Self, Error> {
        acquire_guard(env)?;
        Ok(Guard { env })
    }
}

impl<'a> Drop for Guard<'a> {
    fn drop(&mut self) {
        release_guard(self.env);
    }
}

// Helper function to request randomness (used in both buy_tickets and finalize_raffle)
fn request_randomness(env: &Env) -> Result<u64, Error> {
    let already: bool = env
        .storage()
        .instance()
        .get(&DataKey::RandomnessRequested)
        .unwrap_or(false);
    if already {
        return Err(Error::RandomnessAlreadyRequested);
    }

    // Generate unique request ID
    let request_id_xdr = (
        env.ledger().timestamp(),
        env.ledger().sequence(),
        env.current_contract_address().to_xdr(env),
    )
        .to_xdr(env);
    let request_id_hash: BytesN<32> = env.crypto().sha256(&request_id_xdr).into();
    let arr = request_id_hash.to_array();
    let mut id_bytes = [0u8; 8];
    id_bytes.copy_from_slice(&arr[..8]);
    let request_id = u64::from_be_bytes(id_bytes);

    env.storage()
        .instance()
        .set(&DataKey::RandomnessRequested, &true);
    env.storage()
        .instance()
        .set(&DataKey::RandomnessRequestLedger, &env.ledger().sequence());
    env.storage()
        .instance()
        .set(&DataKey::RandomnessRequestId, &request_id);

    Ok(request_id)
}

/// State machine for drawing entry:
/// - PendingPrize -> Active is the initial funded state.
/// - Active -> Drawing is the only valid transition that begins winner selection.
/// - Active -> Drawing is also used when buy_tickets fills the last ticket and the raffle
///   should enter the draw window.
/// - Drawing -> Finalized is the normal completion path after the oracle or fallback seed
///   produces winners.
/// - Drawing -> Cancelled/Failed is the error or refund path when the drawing flow is aborted.
///
/// Soroban contract calls are atomic per call frame, but the same ledger can still observe
/// overlapping state transitions via re-entrant or concurrent calls into the contract. The
/// DrawingLock is therefore the exclusive guard that makes the transition single-owner even
/// when two entry points race in the same ledger or during re-entry.
///
/// This helper is the single source of truth for entering Drawing and for setting the
/// DrawingLock. The lock prevents any second caller from entering Drawing while the first
/// draw flow is in progress, and it is cleared only after the callback or rollback path
/// finishes so the contract never stays permanently pinned in a half-drawn state.
fn transition_to_drawing(env: &Env, raffle: &mut Raffle, timestamp: u64) -> Result<(), Error> {
    // SECURITY: fast-path guard — if DrawingLock is true, another Drawing transition is
    // already in progress; reject without reading further state
    let drawing_lock: bool = env
        .storage()
        .instance()
        .get(&DataKey::DrawingLock)
        .unwrap_or(false);
    if drawing_lock {
        return Err(Error::DrawingAlreadyInProgress);
    }

    if raffle.status != RaffleStatus::Active {
        if raffle.status == RaffleStatus::Drawing {
            return Err(Error::DrawingAlreadyInProgress);
        }
        return Err(Error::InvalidStatusForDrawingTransition);
    }

    let old_status = raffle.status.clone();
    raffle.status = RaffleStatus::Drawing;
    write_raffle(env, raffle);
    RaffleStatusChanged {
        old_status,
        new_status: RaffleStatus::Drawing,
        timestamp,
    }
    .publish(env);

    // SECURITY: set the DrawingLock in the same contract call as the status transition
    env.storage().instance().set(&DataKey::DrawingLock, &true);
    Ok(())
}

raffle_shared::impl_require_not_paused!(Error, Error::ContractPaused, require_not_paused);

fn validate_token_address(env: &Env, token_address: &Address) -> Result<(), Error> {
    let token_client = token::Client::new(env, token_address);
    let _ = token_client
        .try_decimals()
        .map_err(|_| Error::InvalidTokenAddress)?;
    Ok(())
}

fn build_internal_seed_u64(env: &Env) -> u64 {
    let xdr = (
        env.ledger().timestamp(),
        env.ledger().sequence(),
        env.current_contract_address(),
    )
        .to_xdr(env);
    let hash: BytesN<32> = env.crypto().sha256(&xdr).into();
    let arr = hash.to_array();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&arr[..8]);
    u64::from_be_bytes(bytes)
}

fn calculate_tier_prize(raffle: &Raffle, tier_index: u32) -> Result<i128, Error> {
    let last_tier_index = raffle.prizes.len() - 1;

    if tier_index == last_tier_index {
        let mut allocated_before_last = 0i128;
        for i in 0..last_tier_index {
            let prize_bp = raffle.prizes.get(i).ok_or(Error::InvalidIndex)?;
            let amount = raffle
                .prize_amount
                .checked_mul(prize_bp as i128)
                .ok_or(Error::ArithmeticOverflow)?
                / 10000;
            allocated_before_last = allocated_before_last
                .checked_add(amount)
                .ok_or(Error::ArithmeticOverflow)?;
        }

        return raffle
            .prize_amount
            .checked_sub(allocated_before_last)
            .ok_or(Error::ArithmeticOverflow);
    }

    let prize_bp = raffle.prizes.get(tier_index).ok_or(Error::InvalidIndex)?;
    raffle
        .prize_amount
        .checked_mul(prize_bp as i128)
        .ok_or(Error::ArithmeticOverflow)
        .map(|amount| amount / 10000)
}

#[contractimpl]
impl Contract {
    pub fn init(
        env: Env,
        factory: Address,
        admin: Address,
        creator: Address,
        config: RaffleConfig,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Raffle) {
            return Err(Error::AlreadyInitialized);
        }

        if config.description.len() > MAX_DESCRIPTION_LENGTH {
            return Err(Error::InvalidParameters);
        }

        let now = env.ledger().timestamp();
        if config.no_deadline && config.end_time != 0 {
            return Err(Error::InvalidParameters);
        }
        if !config.no_deadline && config.end_time <= now {
            return Err(Error::InvalidParameters);
        }
        // Explicit check: end_time must be either 0 (no deadline) or in the future
        if config.end_time != 0 && config.end_time <= now {
            return Err(Error::InvalidEndTime);
        }
        if config.max_tickets == 0 || config.max_tickets > MAX_TICKETS_LIMIT {
            return Err(Error::InvalidParameters);
        }
        if config.max_tickets < config.min_tickets {
            return Err(Error::InvalidTicketRange);
        }
        if config.max_tickets_per_tx == 0 || config.max_tickets_per_tx > config.max_tickets {
            return Err(Error::InvalidParameters);
        }

        if config.ticket_price < MIN_TICKET_PRICE {
            return Err(Error::InvalidParameters);
        }
        if config.prize_amount < config.ticket_price {
            return Err(Error::InvalidParameters);
        }
        if config.prize_amount > MAX_PRIZE_AMOUNT {
            return Err(Error::InvalidParameters);
        }
        if config.prizes.is_empty() {
            return Err(Error::InvalidParameters);
        }
        if config.prizes.len() > MAX_PRIZES {
            return Err(Error::TooManyPrizes);
        }
        let mut total_prizes_bp = 0u32;
        for prize_bp in config.prizes.iter() {
            total_prizes_bp += prize_bp;
        }
        if total_prizes_bp != 10000 {
            return Err(Error::InvalidParameters);
        }

        if config.protocol_fee_bp > 10000 {
            return Err(Error::InvalidParameters);
        }

        if config.randomness_source == RandomnessSource::External {
            match config.oracle_address {
                None => return Err(Error::InvalidParameters),
                Some(ref addr) if *addr == env.current_contract_address() => {
                    return Err(Error::InvalidParameters);
                }
                Some(_) => {}
            }
        }

        if config.randomness_source != RandomnessSource::External && config.oracle_address.is_some()
        {
            return Err(Error::InvalidParameters);
        }

        if config.metadata_hash == BytesN::from_array(&env, &[0u8; 32]) {
            return Err(Error::InvalidParameters);
        }

        // Validate that the payment_token is a valid token contract
        validate_token_address(&env, &config.payment_token)?;

        // Prize token matches payment token (no separate prize_token on RaffleConfig).
        let prize_token = config.payment_token.clone();

        // Resolve default values for fields that use 0 as "use default"
        let config = config.resolve_defaults();

        // #259: claim_lockup_seconds must be within [0, MAX_CLAIM_LOCKUP_SECONDS].
        if config.claim_lockup_seconds > MAX_CLAIM_LOCKUP_SECONDS {
            return Err(Error::InvalidParameters);
        }

        // Swap deadline must be within [0, MAX_SWAP_DEADLINE_SECONDS].
        if config.swap_deadline_seconds > MAX_SWAP_DEADLINE_SECONDS {
            return Err(Error::InvalidParameters);
        }

        // Validate early bird parameters
        if config.early_bird_ticket_percentage > 100 {
            return Err(Error::InvalidParameters);
        }
        if config.early_bird_ticket_percentage > 0 && config.early_bird_discount_bp > 10000 {
            return Err(Error::InvalidParameters);
        }

        let raffle = Raffle {
            creator: creator.clone(),
            description: config.description.clone(),
            end_time: config.end_time,
            no_deadline: config.no_deadline,
            max_tickets: config.max_tickets,
            max_tickets_per_tx: config.max_tickets_per_tx,
            min_tickets: config.min_tickets,
            allow_multiple: config.allow_multiple,
            ticket_price: config.ticket_price,
            payment_token: config.payment_token.clone(),
            prize_token: prize_token.clone(),
            prize_amount: config.prize_amount,
            prizes: config.prizes.clone(),
            tickets_sold: 0,
            status: RaffleStatus::PendingPrize,
            prize_deposited: false,
            winners: Vec::new(&env),
            claimed_winners: Vec::new(&env),
            randomness_source: config.randomness_source.clone(),
            oracle_address: config.oracle_address,
            protocol_fee_bp: config.protocol_fee_bp,
            treasury_address: config.treasury_address,
            swap_router: config.swap_router,
            tikka_token: config.tikka_token,
            finalized_at: None,
            claim_lockup_seconds: config.claim_lockup_seconds,
            swap_deadline_seconds: config.swap_deadline_seconds,
            ticket_sales_paused: false,
            early_bird_ticket_percentage: config.early_bird_ticket_percentage,
            early_bird_discount_bp: config.early_bird_discount_bp,
        };
        write_raffle(&env, &raffle);
        env.storage().instance().set(&DataKey::Factory, &factory);
        env.storage().instance().set(&DataKey::Admin, &admin);

        RaffleCreated {
            raffle_id: env.current_contract_address(),
            creator,
            end_time: config.end_time,
            max_tickets: config.max_tickets,
            ticket_price: config.ticket_price,
            payment_token: config.payment_token,
            prize_amount: config.prize_amount,
            prizes: config.prizes,
            description: config.description,
            randomness_source: config.randomness_source,
            metadata_hash: config.metadata_hash,
        }
        .publish(&env);

        Ok(())
    }

    pub fn deposit_prize(env: Env) -> Result<(), Error> {
        require_not_paused(&env)?;
        let mut raffle = read_raffle(&env)?;
        raffle.creator.require_auth();

        if raffle.prize_deposited {
            return Err(Error::PrizeAlreadyDeposited);
        }

        let _old_status = raffle.status.clone();
        raffle.prize_deposited = true;
        write_raffle(&env, &raffle);
        let old_status = raffle.status.clone();

        // Move tokens first. If the transfer fails we want the contract state
        // (prize_deposited flag, raffle.status) to remain untouched.
        let token_client = token::Client::new(&env, &raffle.prize_token);
        let contract_address = env.current_contract_address();

        let _ = token_client
            .try_transfer(&raffle.creator, &contract_address, &raffle.prize_amount)
            .map_err(|_| Error::TokenTransferFailed)?;

        // Transfer succeeded — flip the prize_deposited flag and transition the
        // raffle into Active so ticket sales can begin. This is the explicit
        // status transition #225 asks for: previously the raffle was created
        // directly in Active and `deposit_prize` only flipped a boolean, which
        // left off-chain indexers without a clear signal that the raffle had
        // become buyable.
        raffle.prize_deposited = true;
        raffle.status = RaffleStatus::Active;
        write_raffle(&env, &raffle);

        let timestamp = env.ledger().timestamp();

        PrizeDeposited {
            creator: raffle.creator.clone(),
            amount: raffle.prize_amount,
            token: raffle.payment_token.clone(),
            timestamp,
        }
        .publish(&env);

        RaffleStatusChanged {
            old_status,
            new_status: RaffleStatus::Active,
            timestamp,
        }
        .publish(&env);

        Ok(())
    }

    pub fn buy_tickets(env: Env, buyer: Address, quantity: u32) -> Result<u32, Error> {
        // SECURITY: Fast path guard for DrawingLock!
        let drawing_lock: bool = env
            .storage()
            .instance()
            .get(&DataKey::DrawingLock)
            .unwrap_or(false);
        if drawing_lock {
            return Err(Error::DrawingAlreadyInProgress);
        }
        if quantity == 0 {
            return Err(Error::InvalidQuantity);
        }
        let mut raffle = read_raffle(&env)?;
        if quantity > raffle.max_tickets_per_tx {
            return Err(Error::ExceedsMaxTicketsPerTx);
        }
        buyer.require_auth();
        require_not_paused(&env)?;

        if raffle.status != RaffleStatus::Active {
            return Err(Error::RaffleInactive);
        }
        if raffle.ticket_sales_paused {
            return Err(Error::ContractPaused);
        }
        if !raffle.prize_deposited {
            return Err(Error::InvalidStateTransition);
        }
        if !raffle.no_deadline && env.ledger().timestamp() > raffle.end_time {
            return Err(Error::RaffleExpired);
        }

        // SECURITY: Snapshot initial state for optimistic concurrency control
        let snapshot_sold = raffle.tickets_sold;
        let current_count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::TicketCount(buyer.clone()))
            .unwrap_or(0);

        if snapshot_sold + quantity > raffle.max_tickets {
            return Err(Error::TicketsSoldOut);
        }

        let current_count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::TicketCount(buyer.clone()))
            .unwrap_or(0);
        if !raffle.allow_multiple && (current_count > 0 || quantity > 1) {
            return Err(Error::MultipleTicketsNotAllowed);
        }

        let timestamp = env.ledger().timestamp();
        let effective_price = if raffle.early_bird_ticket_percentage > 0 {
            let early_bird_cap = raffle.max_tickets * raffle.early_bird_ticket_percentage / 100;
            if raffle.tickets_sold < early_bird_cap {
                raffle
                    .ticket_price
                    .checked_mul((10000 - raffle.early_bird_discount_bp) as i128)
                    .ok_or(Error::ArithmeticOverflow)?
                    / 10000
            } else {
                raffle.ticket_price
            }
        } else {
            raffle.ticket_price
        };
        let total_price = effective_price
            .checked_mul(quantity as i128)
            .ok_or(Error::InvalidParameters)?;

        let protocol_fee = total_price
            .checked_mul(raffle.protocol_fee_bp as i128)
            .ok_or(Error::ArithmeticOverflow)?
            / 10000;
        let _net_amount = total_price - protocol_fee;

        // SECURITY: Re-read persisted state and verify no concurrent changes
        let persisted_raffle = read_raffle(&env)?;
        let persisted_sold = persisted_raffle.tickets_sold;
        let persisted_count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::TicketCount(buyer.clone()))
            .unwrap_or(0);

        if persisted_sold != snapshot_sold || persisted_count != current_count {
            return Err(Error::InvalidStateTransition);
        }

        // Final availability check against persisted values
        if persisted_sold + quantity > persisted_raffle.max_tickets {
            return Err(Error::TicketsSoldOut);
        }

        // Track unique buyer addresses for later storage cleanup
        if current_count == 0 {
            let mut buyers: Vec<Address> = env
                .storage()
                .persistent()
                .get(&DataKey::TicketBuyers)
                .unwrap_or_else(|| Vec::new(&env));
            buyers.push_back(buyer.clone());
            env.storage()
                .persistent()
                .set(&DataKey::TicketBuyers, &buyers);
        }

        // Now commit all changes atomically
        let mut ticket_ids = Vec::new(&env);
        for i in 0..quantity {
            let ticket_id = snapshot_sold + i + 1;
            let ticket = Ticket {
                id: ticket_id,
                owner: buyer.clone(),
                purchase_time: timestamp,
                ticket_number: ticket_id,
            };
            env.storage()
                .persistent()
                .set(&DataKey::Ticket(ticket_id), &ticket);
            ticket_ids.push_back(ticket_id);
        }

        // Maintain the per-owner ticket ID index so get_my_tickets is O(1).
        let mut owner_tickets: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::OwnerTickets(buyer.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        for i in 0..ticket_ids.len() {
            if let Some(tid) = ticket_ids.get(i) {
                owner_tickets.push_back(tid);
            }
        }
        env.storage()
            .persistent()
            .set(&DataKey::OwnerTickets(buyer.clone()), &owner_tickets);

        // Update ticket count and raffle sold
        env.storage().persistent().set(
            &DataKey::TicketCount(buyer.clone()),
            &(current_count + quantity),
        );
        raffle.tickets_sold = snapshot_sold + quantity;

        if raffle.tickets_sold >= raffle.max_tickets {
            transition_to_drawing(&env, &mut raffle, timestamp)?;
            // SECURITY: Atomically request randomness after transitioning to Drawing
            if raffle.randomness_source == RandomnessSource::External {
                let request_id = request_randomness(&env)?;
                DrawTriggered {
                    caller: buyer.clone(),
                    total_tickets_sold: raffle.tickets_sold,
                    timestamp,
                }
                .publish(&env);

                RandomnessRequested {
                    oracle: raffle
                        .oracle_address
                        .clone()
                        .unwrap_or(env.current_contract_address()),
                    request_id,
                    timestamp,
                }
                .publish(&env);
            }
        }

        write_raffle(&env, &raffle);

        if let Some(factory_address) = env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::Factory)
        {
            let record_volume_args: Vec<Val> =
                (raffle.payment_token.clone(), total_price).into_val(&env);

            env.authorize_as_current_contract(Vec::from_array(
                &env,
                [InvokerContractAuthEntry::Contract(SubContractInvocation {
                    context: ContractContext {
                        contract: factory_address.clone(),
                        fn_name: Symbol::new(&env, "record_volume"),
                        args: record_volume_args.clone(),
                    },
                    sub_invocations: Vec::new(&env),
                })],
            ));
            env.invoke_contract::<()>(
                &factory_address,
                &Symbol::new(&env, "record_volume"),
                record_volume_args,
            );
            env.invoke_contract::<()>(
                &factory_address,
                &Symbol::new(&env, "track_participant"),
                (buyer.clone(),).into_val(&env),
            );
        }

        let token_client = token::Client::new(&env, &raffle.payment_token);
        let _ = token_client
            .try_transfer(&buyer, env.current_contract_address(), &total_price)
            .map_err(|_| Error::TokenTransferFailed)?;

        if protocol_fee > 0 {
            if let Some(treasury) = &raffle.treasury_address {
                token_client.transfer(&env.current_contract_address(), treasury, &protocol_fee);
            }
            let prev_fees: i128 = env
                .storage()
                .instance()
                .get(&DataKey::AccumulatedFees)
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::AccumulatedFees, &(prev_fees + protocol_fee));
        }

        TicketPurchased {
            buyer: buyer.clone(),
            ticket_ids: ticket_ids.clone(),
            quantity,
            ticket_price: raffle.ticket_price,
            effective_ticket_price: effective_price,
            total_paid: total_price,
            protocol_fee,
            timestamp,
        }
        .publish(&env);

        // NFT minting: issue an on-chain NFT receipt for each ticket purchased.
        // This is best-effort — a failing NFT contract panics the whole call, so
        // the NFT contract is assumed to be trusted and correctly implemented.
        // Optional NFT minting is not wired on the current Raffle state model.

        Ok(raffle.tickets_sold)
    }

    pub fn submit_commit(env: Env, ticket_id: u32, hash: BytesN<32>) -> Result<(), Error> {
        self::tickets::submit_commit(env, ticket_id, hash)
    }

    pub fn finalize_raffle(env: Env) -> Result<(), Error> {
        let mut raffle = read_raffle(&env)?;
        raffle.creator.require_auth();

        if raffle.status != RaffleStatus::Active && raffle.status != RaffleStatus::Drawing {
            return Err(Error::InvalidStatus);
        }

        let now = env.ledger().timestamp();
        let time_ended = !raffle.no_deadline && now >= raffle.end_time;
        let tickets_full = raffle.tickets_sold >= raffle.max_tickets;

        if raffle.status == RaffleStatus::Active && !time_ended && !tickets_full {
            return Err(Error::InvalidStateTransition);
        }

        // #169: zero tickets sold is always a failure regardless of min_tickets,
        // ensuring the creator can recover their deposited prize via refund_prize.
        if raffle.tickets_sold == 0 || raffle.tickets_sold < raffle.min_tickets {
            let _old_status = raffle.status.clone();
            raffle.status = RaffleStatus::Failed;
            write_raffle(&env, &raffle);

            let failure_reason = if raffle.tickets_sold == 0 {
                FailureReason::ZeroTicketsSold
            } else {
                FailureReason::MinTicketsNotMet
            };

            RaffleFailed {
                creator: raffle.creator.clone(),
                reason: failure_reason,
                tickets_sold: raffle.tickets_sold,
                timestamp: now,
            }
            .publish(&env);
            return Ok(());
        }

        let caller = raffle.creator.clone();
        let pre_drawing_status = raffle.status.clone();

        if raffle.status != RaffleStatus::Drawing {
            transition_to_drawing(&env, &mut raffle, now)?;
        }

        if raffle.randomness_source == RandomnessSource::External {
            match request_randomness(&env) {
                Ok(request_id) => {
                    DrawTriggered {
                        caller: caller.clone(),
                        total_tickets_sold: raffle.tickets_sold,
                        timestamp: now,
                    }
                    .publish(&env);

                    RandomnessRequested {
                        oracle: raffle
                            .oracle_address
                            .clone()
                            .unwrap_or(env.current_contract_address()),
                        request_id,
                        timestamp: now,
                    }
                    .publish(&env);
                    return Ok(());
                }
                Err(err) => {
                    raffle.status = pre_drawing_status;
                    write_raffle(&env, &raffle);
                    env.storage().instance().set(&DataKey::DrawingLock, &false);
                    return Err(err);
                }
            }
        }

        DrawTriggered {
            caller: caller.clone(),
            total_tickets_sold: raffle.tickets_sold,
            timestamp: now,
        }
        .publish(&env);

        if raffle.randomness_source == RandomnessSource::CommitReveal {
            // Collect entropy from all commit entries stored by ticket ID.
            //
            // We iterate over ticket IDs 1..=tickets_sold and read the
            // CommitEntry for each one.  Keying by ticket ID (rather than by
            // current owner address) is what makes the fix for #311: a
            // participant who committed and then transferred their ticket
            // still has their CommitEntry present under the original ticket
            // ID, so their entropy is never silently discarded.
            let mut combined = Bytes::new(&env);
            let mut commits_found: u32 = 0;
            for ticket_id in 1..=raffle.tickets_sold {
                if let Some(entry) = env
                    .storage()
                    .persistent()
                    .get::<_, CommitRevealEntry>(&DataKey::CommitEntry(ticket_id))
                {
                    combined.extend_from_array(&entry.hash.to_array());
                    commits_found += 1;
                }
            }

            // If no commits were submitted at all fall through to the
            // internal PRNG so the raffle can still be finalised.
            if commits_found > 0 {
                let hash: BytesN<32> = env.crypto().sha256(&combined).into();
                let arr = hash.to_array();
                let mut seed_bytes = [0u8; 8];
                seed_bytes.copy_from_slice(&arr[..8]);
                let seed = u64::from_be_bytes(seed_bytes);
                return helpers::do_finalize_with_seed(&env, raffle, seed, RandomnessType::Prng);
            }
        }

        let seed = build_internal_seed_u64(&env);
        helpers::do_finalize_with_seed(&env, raffle, seed, RandomnessType::Prng)
    }

    pub fn provide_randomness(
        env: Env,
        random_seed: u64,
        public_key: BytesN<32>,
        proof: BytesN<64>,
        request_id: u64,
    ) -> Result<Address, Error> {
        self::draw::provide_randomness(env, random_seed, public_key, proof, request_id)
    }

    pub fn trigger_randomness_fallback(
        env: Env,
        caller: Address,
        do_refund: bool,
    ) -> Result<(), Error> {
        // # SECURITY: fallback is only valid while a draw is in progress.
        // If DrawingLock is already false, the draw has completed or never started.
        let drawing_lock: bool = env
            .storage()
            .instance()
            .get(&DataKey::DrawingLock)
            .unwrap_or(false);
        if !drawing_lock {
            return Err(Error::DrawingAlreadyComplete);
        }

        caller.require_auth();
        let mut raffle = read_raffle(&env)?;

        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAuthorized)?;
        if caller != raffle.creator && caller != admin {
            return Err(Error::NotAuthorized);
        }

        if raffle.status != RaffleStatus::Drawing {
            return Err(Error::InvalidStateTransition);
        }

        let request_pending: bool = env
            .storage()
            .instance()
            .get(&DataKey::RandomnessRequested)
            .unwrap_or(false);
        if !request_pending {
            return Err(Error::NoRandomnessRequest);
        }

        let request_ledger: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RandomnessRequestLedger)
            .unwrap_or(0);
        if env.ledger().sequence() < request_ledger + ORACLE_TIMEOUT_LEDGERS {
            return Err(Error::FallbackTooEarly);
        }

        if do_refund {
            raffle.status = RaffleStatus::Cancelled;
            write_raffle(&env, &raffle);

            // Clear pending randomness and DrawingLock when cancelling
            env.storage()
                .instance()
                .remove(&DataKey::RandomnessRequested);
            env.storage()
                .instance()
                .remove(&DataKey::RandomnessRequestId);
            env.storage()
                .instance()
                .remove(&DataKey::RandomnessRequestLedger);
            env.storage().instance().set(&DataKey::DrawingLock, &false);

            RaffleCancelled {
                creator: raffle.creator.clone(),
                reason: CancelReason::OracleTimeout,
                tickets_sold: raffle.tickets_sold,
                prize_refunded: raffle.prize_deposited,
                timestamp: env.ledger().timestamp(),
            }
            .publish(&env);
            return Ok(());
        }

        let seed = build_internal_seed_u64(&env);

        RandomnessFallbackTriggered {
            triggered_by: caller,
            seed_used: seed,
            request_ledger,
            fallback_ledger: env.ledger().sequence(),
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        self::helpers::do_finalize_with_seed(&env, raffle, seed, RandomnessType::Fallback)
    }

    pub fn claim_prize(env: Env, winner: Address, tier_index: u32) -> Result<i128, Error> {
        winner.require_auth();
        let _guard = Guard::new(&env)?;
        let mut raffle = read_raffle(&env)?;

        if raffle.status != RaffleStatus::Finalized {
            return Err(Error::InvalidStatus);
        }

        // #259: enforce the configurable lockup delay.
        if let Some(finalized_at) = raffle.finalized_at {
            if env.ledger().timestamp() < finalized_at + raffle.claim_lockup_seconds {
                return Err(Error::ClaimTooEarly);
            }
        }

        if tier_index >= raffle.winners.len() {
            return Err(Error::InvalidParameters);
        }

        if raffle.winners.get(tier_index).ok_or(Error::InvalidIndex)? != winner {
            return Err(Error::NotWinner);
        }

        if raffle
            .claimed_winners
            .get(tier_index)
            .ok_or(Error::InvalidIndex)?
        {
            return Err(Error::PrizeAlreadyClaimed);
        }

        let prize_bp = raffle.prizes.get(tier_index).ok_or(Error::InvalidIndex)?;
        let amount = raffle
            .prize_amount
            .checked_mul(prize_bp as i128)
            .ok_or(Error::ArithmeticOverflow)?
            / 10000;
        let amount = calculate_tier_prize(&raffle, tier_index)?;
        if amount <= 0 {
            return Err(Error::ZeroPrize);
        }

        raffle.claimed_winners.set(tier_index, true);

        let mut all_claimed = true;
        for claimed in raffle.claimed_winners.iter() {
            if !claimed {
                all_claimed = false;
                break;
            }
        }
        if all_claimed {
            raffle.status = RaffleStatus::Claimed;
            RaffleStatusChanged {
                old_status: RaffleStatus::Finalized,
                new_status: RaffleStatus::Claimed,
                timestamp: env.ledger().timestamp(),
            }
            .publish(&env);
        }
        write_raffle(&env, &raffle);

        let token_client = token::Client::new(&env, &raffle.prize_token);
        let _ = token_client
            .try_transfer(&env.current_contract_address(), &winner, &amount)
            .map_err(|_| Error::TokenTransferFailed)?;

        PrizeClaimed {
            winner,
            tier_index,
            payment_token: raffle.prize_token.clone(),
            gross_amount: amount,
            net_amount: amount,
            platform_fee: 0,
            claimed_at: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(amount)
    }

    pub fn withdraw_fees(env: Env, recipient: Address, amount: i128) -> Result<(), Error> {
        let _admin = require_admin(&env)?;

        let raffle = read_raffle(&env)?;
        if raffle.status != RaffleStatus::Finalized && raffle.status != RaffleStatus::Claimed {
            return Err(Error::InvalidStatus);
        }

        if amount <= 0 {
            return Err(Error::InvalidParameters);
        }

        let accumulated: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedFees)
            .unwrap_or(0);
        if amount > accumulated {
            return Err(Error::InsufficientAccumulatedFees);
        }

        let token_client = token::Client::new(&env, &raffle.payment_token);
        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        env.storage()
            .instance()
            .set(&DataKey::AccumulatedFees, &(accumulated - amount));

        FeesWithdrawn {
            recipient,
            amount,
            token: raffle.payment_token.clone(),
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn get_accumulated_fees(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::AccumulatedFees)
            .unwrap_or(0)
    }

    pub fn cancel_raffle(env: Env, reason: CancelReason) -> Result<(), Error> {
        let mut raffle = read_raffle(&env)?;

        match reason {
            CancelReason::AdminCancelled => {
                let admin: Address = env
                    .storage()
                    .instance()
                    .get(&DataKey::Admin)
                    .ok_or(Error::NotAuthorized)?;
                admin.require_auth();
            }
            _ => raffle.creator.require_auth(),
        }

        if raffle.status == RaffleStatus::Finalized
            || raffle.status == RaffleStatus::Cancelled
            || raffle.status == RaffleStatus::Claimed
        {
            return Err(Error::InvalidStatus);
        }

        let was_drawing = raffle.status == RaffleStatus::Drawing;
        let _old_status = raffle.status.clone();
        raffle.status = RaffleStatus::Cancelled;
        write_raffle(&env, &raffle);

        // If cancellation happens during drawing, clear pending randomness and
        // release the drawing lock so the contract cannot remain bricked.
        if was_drawing {
            env.storage()
                .instance()
                .remove(&DataKey::RandomnessRequested);
            env.storage()
                .instance()
                .remove(&DataKey::RandomnessRequestId);
            env.storage()
                .instance()
                .remove(&DataKey::RandomnessRequestLedger);
            env.storage().instance().set(&DataKey::DrawingLock, &false);
        }

        RaffleCancelled {
            creator: raffle.creator.clone(),
            reason,
            tickets_sold: raffle.tickets_sold,
            prize_refunded: raffle.prize_deposited,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    /// Executes a previously scheduled admin cancellation (#406).
    ///
    /// Only succeeds once the timelock set by `cancel_raffle` has elapsed.
    /// Calling it earlier returns `CancelTimelockActive`; calling it with no
    /// pending schedule returns `CancelNotScheduled`.
    pub fn execute_admin_cancel(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAuthorized)?;
        admin.require_auth();

        let cancel_at: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdminCancel)
            .ok_or(Error::InvalidParameters)?;

        let mut raffle = read_raffle(&env)?;

        if raffle.status == RaffleStatus::Finalized
            || raffle.status == RaffleStatus::Cancelled
            || raffle.status == RaffleStatus::Claimed
        {
            return Err(Error::InvalidStatus);
        }

        let now = env.ledger().timestamp();
        if now < cancel_at {
            return Err(Error::InvalidStateTransition);
        }

        env.storage()
            .instance()
            .remove(&DataKey::PendingAdminCancel);

        raffle.status = RaffleStatus::Cancelled;
        write_raffle(&env, &raffle);

        RaffleCancelled {
            creator: raffle.creator.clone(),
            reason: CancelReason::AdminCancelled,
            tickets_sold: raffle.tickets_sold,
            prize_refunded: raffle.prize_deposited,
            timestamp: now,
        }
        .publish(&env);

        Ok(())
    }

    /// Returns the timestamp at which a scheduled admin cancel becomes
    /// executable, or `None` if no cancel is currently scheduled (#406).
    pub fn get_pending_cancel(env: Env) -> Option<u64> {
        env.storage().instance().get(&DataKey::PendingAdminCancel)
    }

    pub fn refund_prize(env: Env) -> Result<(), Error> {
        let mut raffle = read_raffle(&env)?;
        raffle.creator.require_auth();

        if raffle.status != RaffleStatus::Cancelled && raffle.status != RaffleStatus::Failed {
            return Err(Error::InvalidStatus);
        }

        if !raffle.prize_deposited {
            return Err(Error::PrizeNotDeposited);
        }

        raffle.prize_deposited = false;
        write_raffle(&env, &raffle);

        let token_client = token::Client::new(&env, &raffle.payment_token);
        token_client.transfer(
            &env.current_contract_address(),
            &raffle.creator,
            &raffle.prize_amount,
        );
        let _ = token_client
            .try_transfer(
                &env.current_contract_address(),
                &raffle.creator,
                &raffle.prize_amount,
            )
            .map_err(|_| Error::TokenTransferFailed)?;

        PrizeRefunded {
            creator: raffle.creator.clone(),
            amount: raffle.prize_amount,
            token: raffle.prize_token.clone(),
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn emergency_withdraw(env: Env, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut raffle = read_raffle(&env)?;

        if !raffle.prize_deposited {
            return Err(Error::PrizeNotDeposited);
        }

        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAuthorized)?;
        if caller != raffle.creator && caller != admin {
            return Err(Error::NotAuthorized);
        }

        let now = env.ledger().timestamp();

        // Allow emergency withdraw only after a long timeout.
        match raffle.status {
            RaffleStatus::Finalized => {
                if let Some(finalized_at) = raffle.finalized_at {
                    if now < finalized_at + EMERGENCY_WITHDRAW_DELAY_SECONDS {
                        return Err(Error::EmergencyTooEarly);
                    }
                } else {
                    return Err(Error::EmergencyTooEarly);
                }
            }
            RaffleStatus::Drawing => {
                if raffle.no_deadline {
                    let request_ledger: u32 = env
                        .storage()
                        .instance()
                        .get(&DataKey::RandomnessRequestLedger)
                        .unwrap_or(0);
                    let estimated_seconds =
                        (env.ledger().sequence().saturating_sub(request_ledger) as u64) * 5;
                    if estimated_seconds < EMERGENCY_WITHDRAW_DELAY_SECONDS {
                        return Err(Error::EmergencyTooEarly);
                    }
                } else if now < raffle.end_time + EMERGENCY_WITHDRAW_DELAY_SECONDS {
                    return Err(Error::EmergencyTooEarly);
                }
            }
            _ => return Err(Error::InvalidStatus),
        }

        // Mark prize as withdrawn and transfer back to creator
        raffle.prize_deposited = false;
        raffle.status = RaffleStatus::Cancelled;
        write_raffle(&env, &raffle);

        let token_client = token::Client::new(&env, &raffle.prize_token);
        token_client.transfer(
            &env.current_contract_address(),
            &raffle.creator,
            &raffle.prize_amount,
        );

        EmergencyWithdrawn {
            withdrawn_by: caller,
            to: raffle.creator.clone(),
            amount: raffle.prize_amount,
            token: raffle.prize_token.clone(),
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn refund_ticket(env: Env, ticket_id: u32) -> Result<i128, Error> {
        let raffle = read_raffle(&env)?;

        // #406: Ticket holders may refund as soon as an admin cancel is
        // *scheduled*, without waiting for the timelock to execute the cancel.
        let cancel_scheduled = env.storage().instance().has(&DataKey::PendingAdminCancel);

        // #258: status check BEFORE require_auth to prevent double-spend on
        // status transitions that occur between auth and the gate.
        if raffle.status != RaffleStatus::Cancelled
            && raffle.status != RaffleStatus::Failed
            && !cancel_scheduled
        {
            return Err(Error::InvalidStatus);
        }

        let _guard = Guard::new(&env)?;
        let ticket: Ticket = env
            .storage()
            .persistent()
            .get(&DataKey::Ticket(ticket_id))
            .ok_or(Error::TicketNotFound)?;
        ticket.owner.require_auth();

        // Check if already refunded
        if env
            .storage()
            .persistent()
            .has(&DataKey::TicketRefunded(ticket_id))
        {
            return Err(Error::PrizeAlreadyClaimed);
        }

        env.storage()
            .persistent()
            .set(&DataKey::TicketRefunded(ticket_id), &true);

        let token_client = token::Client::new(&env, &raffle.payment_token);
        token_client.transfer(
            &env.current_contract_address(),
            &ticket.owner,
            &raffle.ticket_price,
        );
        let _ = token_client
            .try_transfer(
                &env.current_contract_address(),
                &ticket.owner,
                &raffle.ticket_price,
            )
            .map_err(|_| Error::TokenTransferFailed)?;

        TicketRefunded {
            buyer: ticket.owner,
            ticket_number: ticket.ticket_number,
            amount: raffle.ticket_price,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(raffle.ticket_price)
    }

    pub fn batch_refund_tickets(
        env: Env,
        owner: Address,
        ticket_ids: Vec<u32>,
    ) -> Result<i128, Error> {
        owner.require_auth();
        acquire_guard(&env)?;
        let raffle = read_raffle(&env)?;

        if raffle.status != RaffleStatus::Cancelled && raffle.status != RaffleStatus::Failed {
            return Err(Error::InvalidStatus);
        }
        if ticket_ids.len() > 50 {
            // per-tx cap to stay within compute limits
            return Err(Error::InvalidParameters);
        }

        let mut total_refund = 0i128;

        for ticket_id in ticket_ids.iter() {
            let ticket: Ticket = env
                .storage()
                .persistent()
                .get(&DataKey::Ticket(ticket_id))
                .ok_or(Error::TicketNotFound)?;

            if ticket.owner != owner {
                return Err(Error::NotAuthorized);
            }

            let refund_key = (DataKey::Ticket(ticket_id), Symbol::new(&env, "refunded"));
            if env.storage().persistent().has(&refund_key) {
                continue;
            }

            env.storage().persistent().set(&refund_key, &true);
            total_refund += raffle.ticket_price;

            crate::events::TicketRefunded {
                buyer: ticket.owner,
                ticket_number: ticket.ticket_number,
                amount: raffle.ticket_price,
                timestamp: env.ledger().timestamp(),
            }
            .publish(&env);
        }

        if total_refund > 0 {
            let token_client = token::Client::new(&env, &raffle.payment_token);
            token_client.transfer(&env.current_contract_address(), &owner, &total_refund);
        }

        release_guard(&env);
        Ok(total_refund)
    }

    pub fn get_raffle(env: Env) -> Result<Raffle, Error> {
        read_raffle(&env)
    }

    pub fn get_fairness_data(env: Env) -> Result<FairnessData, Error> {
        let metadata: FairnessMetadata = env
            .storage()
            .persistent()
            .get(&DataKey::RandomnessSeed)
            .ok_or(Error::InvalidStatus)?;
        let raffle = read_raffle(&env)?;
        let mut ticket_ids = Vec::new(&env);
        let count = raffle.tickets_sold;
        for i in 1..=count {
            ticket_ids.push_back(i);
        }

        Ok(FairnessData {
            seed: metadata.seed,
            randomness_source: metadata.randomness_source,
            ticket_ids,
            winning_ticket_indices: metadata.winning_ticket_indices,
            draw_timestamp: metadata.draw_timestamp,
            draw_sequence: metadata.draw_sequence,
        })
    }

    /// Return all ticket IDs owned by `owner`.
    ///
    /// Uses the `OwnerTickets` index maintained during `buy_tickets` for an
    /// O(1) read.  Falls back to an empty Vec when the address has never
    /// purchased a ticket.
    pub fn get_my_tickets(env: Env, owner: Address) -> Vec<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::OwnerTickets(owner))
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn wipe_storage(env: Env) -> Result<(), Error> {
        let factory: Address = env
            .storage()
            .instance()
            .get(&DataKey::Factory)
            .ok_or(Error::NotAuthorized)?;
        factory.require_auth();

        let raffle = read_raffle(&env)?;
        if raffle.status != RaffleStatus::Cancelled
            && raffle.status != RaffleStatus::Claimed
            && raffle.status != RaffleStatus::Failed
        {
            return Err(Error::InvalidStatus);
        }

        // Wipe ticket storage
        for i in 1..=raffle.tickets_sold {
            env.storage().persistent().remove(&DataKey::Ticket(i));
            env.storage()
                .persistent()
                .remove(&DataKey::TicketRefunded(i));
            env.storage().persistent().remove(&DataKey::CommitEntry(i));
        }

        let buyers: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::TicketBuyers)
            .unwrap_or_else(|| Vec::new(&env));
        for buyer in buyers.iter() {
            env.storage()
                .persistent()
                .remove(&DataKey::TicketCount(buyer.clone()));
            env.storage()
                .persistent()
                .remove(&DataKey::OwnerTickets(buyer.clone()));
        }
        env.storage().persistent().remove(&DataKey::TicketBuyers);

        // Wipe instance storage
        env.storage().instance().remove(&DataKey::Raffle);
        env.storage().instance().remove(&DataKey::Factory);
        env.storage().instance().remove(&DataKey::Admin);
        env.storage().instance().remove(&DataKey::Paused);
        env.storage().instance().remove(&DataKey::ReentrancyGuard);
        env.storage().instance().remove(&DataKey::AccumulatedFees);
        env.storage()
            .instance()
            .remove(&DataKey::RandomnessRequested);
        env.storage()
            .instance()
            .remove(&DataKey::RandomnessRequestLedger);
        env.storage()
            .instance()
            .remove(&DataKey::RandomnessRequestId);
        env.storage().instance().remove(&DataKey::DrawingLock);
        env.storage().instance().remove(&DataKey::FinishTime);
        env.storage()
            .instance()
            .remove(&DataKey::PendingAdminCancel);

        // Wipe persistent instance-level keys
        env.storage().persistent().remove(&DataKey::RandomnessSeed);
        env.storage().persistent().remove(&DataKey::Admin);

        Ok(())
    }

    pub fn pause(env: Env) -> Result<(), Error> {
        let factory: Address = env
            .storage()
            .instance()
            .get(&DataKey::Factory)
            .ok_or(Error::NotAuthorized)?;
        factory.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);

        ContractPaused {
            paused_by: factory,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let factory: Address = env
            .storage()
            .instance()
            .get(&DataKey::Factory)
            .ok_or(Error::NotAuthorized)?;
        factory.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);

        ContractUnpaused {
            unpaused_by: factory,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        self::admin::set_admin(env, new_admin)
    }

    pub fn pause_ticket_sales(env: Env, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut raffle = read_raffle(&env)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAuthorized)?;
        if caller != raffle.creator && caller != admin {
            return Err(Error::NotAuthorized);
        }
        if raffle.status != RaffleStatus::Active {
            return Err(Error::InvalidStatus);
        }
        raffle.ticket_sales_paused = true;
        write_raffle(&env, &raffle);

        TicketSalesPaused {
            paused_by: caller,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn resume_ticket_sales(env: Env, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let mut raffle = read_raffle(&env)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAuthorized)?;
        if caller != raffle.creator && caller != admin {
            return Err(Error::NotAuthorized);
        }
        if raffle.status != RaffleStatus::Active {
            return Err(Error::InvalidStatus);
        }
        raffle.ticket_sales_paused = false;
        write_raffle(&env, &raffle);

        TicketSalesResumed {
            resumed_by: caller,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn is_ticket_sales_paused(env: Env) -> bool {
        read_raffle(&env)
            .map(|raffle| raffle.ticket_sales_paused)
            .unwrap_or(false)
    }

    /// Sweep tokens that were accidentally sent to this contract.
    /// The raffle's own payment_token cannot be swept while a prize is held in escrow,
    /// ensuring active raffle funds are never at risk.
    pub fn rescue_tokens(
        env: Env,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAuthorized)?;
        admin.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidParameters);
        }

        // Protect active escrow: block sweeping the prize token while the prize
        // is deposited. Also block the payment token if it equals the prize token
        // to prevent draining the fee pool via a mis-directed rescue.
        if let Ok(raffle) = read_raffle(&env) {
            if raffle.prize_deposited
                && (token == raffle.prize_token || token == raffle.payment_token)
            {
                return Err(Error::InvalidParameters);
            }
        }

        let token_client = token::Client::new(&env, &token);
        let _ = token_client
            .try_transfer(&env.current_contract_address(), &recipient, &amount)
            .map_err(|_| Error::TokenTransferFailed)?;

        TokensRescued {
            rescued_by: admin,
            token,
            recipient,
            amount,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    pub fn update_oracle_address(env: Env, new_oracle: Address) -> Result<(), Error> {
        self::admin::update_oracle_address(env, new_oracle)
    }

    pub fn set_protocol_fee_bp(env: Env, new_fee_bp: u32) -> Result<(), Error> {
        self::admin::set_protocol_fee_bp(env, new_fee_bp)
    }

    pub fn set_swap_deadline(env: Env, new_deadline_seconds: u64) -> Result<(), Error> {
        self::admin::set_swap_deadline(env, new_deadline_seconds)
    }
}

#[cfg(test)]
mod test;
