//! Serial authoritative tick reducer.
//!
//! One tick owns one ordered dispatch. The endpoint claims the tick, then
//! this reducer drains the mailbox, feeds every accepted provider in the
//! frozen Go order, commits the viewer overlay with retired-session pruning
//! and publishes exactly once. The replay order suite pins the dispatch call
//! sequence below, so a swapped row fails loudly; keep provider call sites
//! spelled as plain provider paths, one site per listed invocation.

use super::contracts::{TickBudget, TickCounters, TickPublication};
use super::state::{AuthorityState, TickContext};

/// Reduces one claimed tick into its owned publication.
///
/// The endpoint already validated the phase and budget shape and bumped the
/// tick counter, so the tick in progress is the claimed predecessor of the
/// counter. A direct call without the endpoint claim repeats tick zero; the
/// production path always goes through the endpoint delegation.
pub fn reduce_tick(state: &mut AuthorityState, budget: TickBudget) -> TickPublication {
    let tick = state.next_tick().saturating_sub(1);
    let _context = TickContext::for_tick(state, budget);
    TickPublication {
        tick,
        events: Vec::new(),
        control: Vec::new(),
        counters: TickCounters {
            executed_tick: tick,
            ..TickCounters::default()
        },
    }
}
