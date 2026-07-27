#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tikka_raffle_instance::randomness::OracleSeedWinnerSelection;

#[derive(Debug, Arbitrary)]
struct WinnerSelectionInput {
    seed: u64,
    total_tickets: u8, // 1..=255 to stay fast
    winner_count: u8,  // 1..=total_tickets
}

fuzz_target!(|input: WinnerSelectionInput| {
    let n = (input.total_tickets as u32).max(1);
    let w = ((input.winner_count as u32) % n).max(1);
    let selector = OracleSeedWinnerSelection::new(input.seed);

    // This must always terminate
    let indices = selector.select_winner_indices_pure(n, w);

    // INVARIANT: correct count
    assert_eq!(indices.len(), w as usize);
    // INVARIANT: all within range
    assert!(indices.iter().all(|&i| i < n));
    // INVARIANT: all unique
    let unique: std::collections::HashSet<_> = indices.iter().collect();
    assert_eq!(unique.len(), indices.len());
});
