use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    token, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};

use raffle_shared::{RandomnessSource, Ticket};

use crate::events::{DrawTriggered, RandomnessRequested, TicketPurchased};
use crate::{
    request_randomness, require_not_paused, transition_to_drawing, CommitRevealEntry, DataKey,
    Error, RaffleStatus,
};

pub(crate) fn buy_tickets(env: Env, buyer: Address, quantity: u32) -> Result<u32, Error> {
    let drawing_lock: bool = env
        .storage()
        .instance()
        .get(&crate::DataKey::DrawingLock)
        .unwrap_or(false);
    if drawing_lock {
        return Err(Error::DrawingAlreadyInProgress);
    }
    if quantity == 0 {
        return Err(Error::InvalidQuantity);
    }
    let mut raffle = crate::read_raffle(&env)?;
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

    let snapshot_sold = raffle.tickets_sold;
    let current_count: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::TicketCount(buyer.clone()))
        .unwrap_or(0);

    if snapshot_sold + quantity > raffle.max_tickets {
        return Err(Error::TicketsSoldOut);
    }
    if !raffle.allow_multiple && (current_count > 0 || quantity > 1) {
        return Err(Error::MultipleTicketsNotAllowed);
    }

    let timestamp = env.ledger().timestamp();
    let total_price = raffle
        .ticket_price
        .checked_mul(quantity as i128)
        .ok_or(Error::InvalidParameters)?;
    let protocol_fee = total_price
        .checked_mul(raffle.protocol_fee_bp as i128)
        .ok_or(Error::ArithmeticOverflow)?
        / 10000;

    let persisted = crate::read_raffle(&env)?;
    let persisted_sold = persisted.tickets_sold;
    let persisted_count: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::TicketCount(buyer.clone()))
        .unwrap_or(0);
    if persisted_sold != snapshot_sold || persisted_count != current_count {
        return Err(Error::InvalidStateTransition);
    }
    if persisted_sold + quantity > persisted.max_tickets {
        return Err(Error::TicketsSoldOut);
    }

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

    env.storage().persistent().set(
        &DataKey::TicketCount(buyer.clone()),
        &(current_count + quantity),
    );
    raffle.tickets_sold = snapshot_sold + quantity;

    if raffle.tickets_sold >= raffle.max_tickets {
        transition_to_drawing(&env, &mut raffle, timestamp)?;
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

    crate::write_raffle(&env, &raffle);

    if let Some(factory_address) = env
        .storage()
        .instance()
        .get::<_, Address>(&DataKey::Factory)
    {
        let args: Vec<Val> = (raffle.payment_token.clone(), total_price).into_val(&env);
        env.authorize_as_current_contract(Vec::from_array(
            &env,
            [InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: factory_address.clone(),
                    fn_name: Symbol::new(&env, "record_volume"),
                    args: args.clone(),
                },
                sub_invocations: Vec::new(&env),
            })],
        ));
        env.invoke_contract::<()>(&factory_address, &Symbol::new(&env, "record_volume"), args);
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
        let prev: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedFees)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::AccumulatedFees, &(prev + protocol_fee));
    }

    TicketPurchased {
        buyer,
        ticket_ids,
        quantity,
        ticket_price: raffle.ticket_price,
        effective_ticket_price: raffle.ticket_price,
        total_paid: total_price,
        protocol_fee,
        timestamp,
    }
    .publish(&env);
    Ok(raffle.tickets_sold)
}

pub(crate) fn submit_commit(env: Env, ticket_id: u32, hash: BytesN<32>) -> Result<(), Error> {
    let raffle = crate::read_raffle(&env)?;

    if raffle.randomness_source != RandomnessSource::CommitReveal {
        return Err(Error::InvalidParameters);
    }
    if raffle.status != RaffleStatus::Active && raffle.status != RaffleStatus::Drawing {
        return Err(Error::InvalidStatus);
    }

    let ticket: Ticket = env
        .storage()
        .persistent()
        .get(&DataKey::Ticket(ticket_id))
        .ok_or(Error::TicketNotFound)?;
    ticket.owner.require_auth();

    env.storage().persistent().set(
        &DataKey::CommitEntry(ticket_id),
        &CommitRevealEntry {
            committer: ticket.owner,
            hash,
        },
    );

    Ok(())
}
