use soroban_sdk::{token, Address, Env};

use raffle_shared::CancelReason;

use crate::events::{
    ContractPaused, ContractUnpaused, EmergencyWithdrawn, FeesWithdrawn, OracleAddressUpdated,
    ProtocolFeeUpdated, RaffleCancelled, SwapDeadlineUpdated, TicketSalesPaused,
    TicketSalesResumed, TokensRescued,
};
use crate::{
    read_raffle, require_admin, write_raffle, DataKey, Error, RaffleStatus,
    EMERGENCY_WITHDRAW_DELAY_SECONDS, MAX_PROTOCOL_FEE_BP, MAX_SWAP_DEADLINE_SECONDS,
};

pub(crate) fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
    let _old = require_admin(&env)?;
    if !new_admin.exists() || new_admin == env.current_contract_address() {
        return Err(Error::InvalidAdminAddress);
    }
    env.storage().persistent().set(&DataKey::Admin, &new_admin);
    Ok(())
}

pub(crate) fn update_oracle_address(env: Env, new_oracle: Address) -> Result<(), Error> {
    let admin = require_admin(&env)?;
    let mut raffle = read_raffle(&env)?;
    if raffle.randomness_source != raffle_shared::RandomnessSource::External {
        return Err(Error::InvalidParameters);
    }
    if new_oracle == env.current_contract_address() {
        return Err(Error::InvalidParameters);
    }
    if raffle.status == RaffleStatus::Finalized
        || raffle.status == RaffleStatus::Claimed
        || raffle.status == RaffleStatus::Cancelled
    {
        return Err(Error::InvalidStatus);
    }
    let old = raffle.oracle_address.clone();
    raffle.oracle_address = Some(new_oracle.clone());
    write_raffle(&env, &raffle);
    OracleAddressUpdated {
        old_oracle: old,
        new_oracle,
        updated_by: admin,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    Ok(())
}

pub(crate) fn set_protocol_fee_bp(env: Env, new_fee_bp: u32) -> Result<(), Error> {
    let admin = require_admin(&env)?;
    if new_fee_bp > MAX_PROTOCOL_FEE_BP {
        return Err(Error::InvalidParameters);
    }
    let mut raffle = read_raffle(&env)?;
    if raffle.tickets_sold > 0 {
        return Err(Error::InvalidStatus);
    }
    let old = raffle.protocol_fee_bp;
    raffle.protocol_fee_bp = new_fee_bp;
    write_raffle(&env, &raffle);
    ProtocolFeeUpdated {
        old_fee_bp: old,
        new_fee_bp,
        updated_by: admin,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    Ok(())
}

pub(crate) fn set_swap_deadline(env: Env, new_deadline_seconds: u64) -> Result<(), Error> {
    let admin = require_admin(&env)?;
    if new_deadline_seconds > MAX_SWAP_DEADLINE_SECONDS {
        return Err(Error::InvalidParameters);
    }
    let mut raffle = read_raffle(&env)?;
    if raffle.tickets_sold > 0 {
        return Err(Error::InvalidStatus);
    }
    let old = raffle.swap_deadline_seconds;
    raffle.swap_deadline_seconds = new_deadline_seconds;
    write_raffle(&env, &raffle);
    SwapDeadlineUpdated {
        old_deadline_seconds: old,
        new_deadline_seconds,
        updated_by: admin,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    Ok(())
}

pub(crate) fn cancel_raffle(env: Env, reason: CancelReason) -> Result<(), Error> {
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
    raffle.status = RaffleStatus::Cancelled;
    write_raffle(&env, &raffle);
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

pub(crate) fn pause(env: Env) -> Result<(), Error> {
    let f: Address = env
        .storage()
        .instance()
        .get(&DataKey::Factory)
        .ok_or(Error::NotAuthorized)?;
    f.require_auth();
    env.storage().instance().set(&DataKey::Paused, &true);
    ContractPaused {
        paused_by: f,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    Ok(())
}

pub(crate) fn unpause(env: Env) -> Result<(), Error> {
    let f: Address = env
        .storage()
        .instance()
        .get(&DataKey::Factory)
        .ok_or(Error::NotAuthorized)?;
    f.require_auth();
    env.storage().instance().set(&DataKey::Paused, &false);
    ContractUnpaused {
        unpaused_by: f,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    Ok(())
}

pub(crate) fn pause_ticket_sales(env: Env, caller: Address) -> Result<(), Error> {
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

pub(crate) fn resume_ticket_sales(env: Env, caller: Address) -> Result<(), Error> {
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

pub(crate) fn withdraw_fees(env: Env, recipient: Address, amount: i128) -> Result<(), Error> {
    let _admin = require_admin(&env)?;
    let raffle = read_raffle(&env)?;
    if raffle.status != RaffleStatus::Finalized && raffle.status != RaffleStatus::Claimed {
        return Err(Error::InvalidStatus);
    }
    if amount <= 0 {
        return Err(Error::InvalidParameters);
    }
    let acc: i128 = env
        .storage()
        .instance()
        .get(&DataKey::AccumulatedFees)
        .unwrap_or(0);
    if amount > acc {
        return Err(Error::InsufficientAccumulatedFees);
    }
    let tc = token::Client::new(&env, &raffle.payment_token);
    tc.transfer(&env.current_contract_address(), &recipient, &amount);
    env.storage()
        .instance()
        .set(&DataKey::AccumulatedFees, &(acc - amount));
    FeesWithdrawn {
        recipient,
        amount,
        token: raffle.payment_token.clone(),
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    Ok(())
}

pub(crate) fn rescue_tokens(
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
    if let Ok(raffle) = read_raffle(&env) {
        if token == raffle.payment_token && raffle.prize_deposited {
            return Err(Error::InvalidParameters);
        }
    }
    let tc = token::Client::new(&env, &token);
    let _ = tc
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

pub(crate) fn wipe_storage(env: Env) -> Result<(), Error> {
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

    for i in 1..=raffle.tickets_sold {
        env.storage().persistent().remove(&DataKey::Ticket(i));
        env.storage()
            .persistent()
            .remove(&DataKey::TicketRefunded(i));
        env.storage().persistent().remove(&DataKey::CommitEntry(i));
    }
    let buyers: soroban_sdk::Vec<Address> = env
        .storage()
        .persistent()
        .get(&DataKey::TicketBuyers)
        .unwrap_or_else(|| soroban_sdk::Vec::new(&env));
    for b in buyers.iter() {
        env.storage()
            .persistent()
            .remove(&DataKey::TicketCount(b.clone()));
    }
    env.storage().persistent().remove(&DataKey::TicketBuyers);

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
    env.storage().persistent().remove(&DataKey::RandomnessSeed);
    env.storage().persistent().remove(&DataKey::Admin);

    Ok(())
}

pub(crate) fn emergency_withdraw(env: Env, caller: Address) -> Result<(), Error> {
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
    match raffle.status {
        RaffleStatus::Finalized => match raffle.finalized_at {
            Some(fa) if now >= fa + EMERGENCY_WITHDRAW_DELAY_SECONDS => {}
            _ => return Err(Error::EmergencyTooEarly),
        },
        RaffleStatus::Drawing => {
            if raffle.no_deadline {
                let rl: u32 = env
                    .storage()
                    .instance()
                    .get(&DataKey::RandomnessRequestLedger)
                    .unwrap_or(0);
                let est = (env.ledger().sequence().saturating_sub(rl) as u64) * 5;
                if est < EMERGENCY_WITHDRAW_DELAY_SECONDS {
                    return Err(Error::EmergencyTooEarly);
                }
            } else if now < raffle.end_time + EMERGENCY_WITHDRAW_DELAY_SECONDS {
                return Err(Error::EmergencyTooEarly);
            }
        }
        _ => return Err(Error::InvalidStatus),
    }

    raffle.prize_deposited = false;
    raffle.status = RaffleStatus::Cancelled;
    write_raffle(&env, &raffle);

    let tc = token::Client::new(&env, &raffle.payment_token);
    tc.transfer(
        &env.current_contract_address(),
        &raffle.creator,
        &raffle.prize_amount,
    );

    EmergencyWithdrawn {
        withdrawn_by: caller,
        to: raffle.creator.clone(),
        amount: raffle.prize_amount,
        token: raffle.payment_token.clone(),
        timestamp: now,
    }
    .publish(&env);
    Ok(())
}
