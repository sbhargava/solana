#![feature(test)]

extern crate test;

use solana::bank::*;
use solana::mint::Mint;
use solana::status_deque::MAX_ENTRY_IDS;
use solana_sdk::hash::hash;
use solana_sdk::signature::{Keypair, KeypairUtil};
use solana_sdk::system_transaction::SystemTransaction;
use solana_sdk::transaction::Transaction;
use test::Bencher;

#[bench]
fn bench_process_transaction(bencher: &mut Bencher) {
    let mint = Mint::new(100_000_000);
    let bank = Bank::new(&mint);

    // Create transactions between unrelated parties.
    let transactions: Vec<_> = (0..4096)
        .into_iter()
        .map(|_| {
            // Seed the 'from' account.
            let rando0 = Keypair::new();
            let tx = Transaction::system_move(
                &mint.keypair(),
                rando0.pubkey(),
                10_000,
                bank.last_id(),
                0,
            );
            assert_eq!(bank.process_transaction(&tx), Ok(()));

            // Seed the 'to' account and a cell for its signature.
            let rando1 = Keypair::new();
            let tx = Transaction::system_move(&rando0, rando1.pubkey(), 1, bank.last_id(), 0);
            assert_eq!(bank.process_transaction(&tx), Ok(()));

            // Finally, return the transaction to the benchmark.
            tx
        })
        .collect();

    let mut id = bank.last_id();

    for _ in 0..(MAX_ENTRY_IDS - 1) {
        bank.register_tick(&id);
        id = hash(&id.as_ref())
    }

    bencher.iter(|| {
        // Since benchmarker runs this multiple times, we need to clear the signatures.
        bank.clear_signatures();
        let results = bank.process_transactions(&transactions);
        assert!(results.iter().all(Result::is_ok));
    })
}
