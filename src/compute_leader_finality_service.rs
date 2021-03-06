//! The `compute_leader_finality_service` module implements the tools necessary
//! to generate a thread which regularly calculates the last finality times
//! observed by the leader

use crate::bank::Bank;

use crate::service::Service;
use solana_metrics::{influxdb, submit};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::timing;
use solana_sdk::vote_program::{self, VoteProgram};
use std::result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::thread::{self, Builder, JoinHandle};
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum FinalityError {
    NoValidSupermajority,
}

pub const COMPUTE_FINALITY_MS: u64 = 100;

pub struct ComputeLeaderFinalityService {
    compute_finality_thread: JoinHandle<()>,
}

impl ComputeLeaderFinalityService {
    fn get_last_supermajority_timestamp(
        bank: &Arc<Bank>,
        leader_id: Pubkey,
        now: u64,
        last_valid_validator_timestamp: u64,
    ) -> result::Result<u64, FinalityError> {
        let mut total_stake = 0;

        let mut ticks_and_stakes: Vec<(u64, u64)> = {
            let bank_accounts = bank.accounts.accounts_db.read().unwrap();
            // TODO: Doesn't account for duplicates since a single validator could potentially register
            // multiple vote accounts. Once that is no longer possible (see the TODO in vote_program.rs,
            // process_transaction(), case VoteInstruction::RegisterAccount), this will be more accurate.
            // See github issue 1654.
            bank_accounts
                .accounts
                .values()
                .filter_map(|account| {
                    // Filter out any accounts that don't belong to the VoteProgram
                    // by returning None
                    if vote_program::check_id(&account.owner) {
                        if let Ok(vote_state) = VoteProgram::deserialize(&account.userdata) {
                            if leader_id == vote_state.node_id {
                                return None;
                            }
                            let validator_stake = bank.get_stake(&vote_state.node_id);
                            total_stake += validator_stake;
                            // Filter out any validators that don't have at least one vote
                            // by returning None
                            return vote_state
                                .votes
                                .back()
                                .map(|vote| (vote.tick_height, validator_stake));
                        }
                    }

                    None
                })
                .collect()
        };

        let super_majority_stake = (2 * total_stake) / 3;

        if let Some(last_valid_validator_timestamp) =
            bank.get_finality_timestamp(&mut ticks_and_stakes, super_majority_stake)
        {
            return Ok(last_valid_validator_timestamp);
        }

        if last_valid_validator_timestamp != 0 {
            submit(
                influxdb::Point::new(&"leader-finality")
                    .add_field(
                        "duration_ms",
                        influxdb::Value::Integer((now - last_valid_validator_timestamp) as i64),
                    )
                    .to_owned(),
            );
        }

        Err(FinalityError::NoValidSupermajority)
    }

    pub fn compute_finality(
        bank: &Arc<Bank>,
        leader_id: Pubkey,
        last_valid_validator_timestamp: &mut u64,
    ) {
        let now = timing::timestamp();
        if let Ok(super_majority_timestamp) = Self::get_last_supermajority_timestamp(
            bank,
            leader_id,
            now,
            *last_valid_validator_timestamp,
        ) {
            let finality_ms = now - super_majority_timestamp;

            *last_valid_validator_timestamp = super_majority_timestamp;
            bank.set_finality((now - *last_valid_validator_timestamp) as usize);

            submit(
                influxdb::Point::new(&"leader-finality")
                    .add_field("duration_ms", influxdb::Value::Integer(finality_ms as i64))
                    .to_owned(),
            );
        }
    }

    /// Create a new ComputeLeaderFinalityService for computing finality.
    pub fn new(bank: Arc<Bank>, leader_id: Pubkey, exit: Arc<AtomicBool>) -> Self {
        let compute_finality_thread = Builder::new()
            .name("solana-leader-finality-stage".to_string())
            .spawn(move || {
                let mut last_valid_validator_timestamp = 0;
                loop {
                    if exit.load(Ordering::Relaxed) {
                        break;
                    }
                    Self::compute_finality(&bank, leader_id, &mut last_valid_validator_timestamp);
                    sleep(Duration::from_millis(COMPUTE_FINALITY_MS));
                }
            })
            .unwrap();

        (ComputeLeaderFinalityService {
            compute_finality_thread,
        })
    }
}

impl Service for ComputeLeaderFinalityService {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        self.compute_finality_thread.join()
    }
}

#[cfg(test)]
pub mod tests {
    use crate::bank::Bank;
    use crate::compute_leader_finality_service::ComputeLeaderFinalityService;
    use crate::create_vote_account::*;

    use crate::mint::Mint;
    use bincode::serialize;
    use solana_sdk::hash::hash;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::transaction::Transaction;
    use solana_sdk::vote_program::Vote;
    use solana_sdk::vote_transaction::VoteTransaction;
    use std::sync::Arc;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_compute_finality() {
        solana_logger::setup();

        let mint = Mint::new(1234);
        let dummy_leader_id = Keypair::new().pubkey();
        let bank = Arc::new(Bank::new(&mint));
        // generate 10 validators, but only vote for the first 6 validators
        let ids: Vec<_> = (0..10)
            .map(|i| {
                let last_id = hash(&serialize(&i).unwrap()); // Unique hash
                bank.register_tick(&last_id);
                // sleep to get a different timestamp in the bank
                sleep(Duration::from_millis(1));
                last_id
            })
            .collect();

        // Create a total of 10 vote accounts, each will have a balance of 1 (after giving 1 to
        // their vote account), for a total staking pool of 10 tokens.
        let vote_accounts: Vec<_> = (0..10)
            .map(|i| {
                // Create new validator to vote
                let validator_keypair = Keypair::new();
                let last_id = ids[i];

                // Give the validator some tokens
                bank.transfer(2, &mint.keypair(), validator_keypair.pubkey(), last_id)
                    .unwrap();
                let vote_account = create_vote_account(&validator_keypair, &bank, 1, last_id)
                    .expect("Expected successful creation of account");

                if i < 6 {
                    let vote = Vote {
                        tick_height: (i + 1) as u64,
                    };
                    let vote_tx = Transaction::vote_new(&vote_account, vote, last_id, 0);
                    bank.process_transaction(&vote_tx).unwrap();
                }
                vote_account
            })
            .collect();

        // There isn't 2/3 consensus, so the bank's finality value should be the default
        let mut last_finality_time = 0;
        ComputeLeaderFinalityService::compute_finality(
            &bank,
            dummy_leader_id,
            &mut last_finality_time,
        );
        assert_eq!(bank.finality(), std::usize::MAX);

        // Get another validator to vote, so we now have 2/3 consensus
        let vote_account = &vote_accounts[7];
        let vote = Vote { tick_height: 7 };
        let vote_tx = Transaction::vote_new(&vote_account, vote, ids[6], 0);
        bank.process_transaction(&vote_tx).unwrap();

        ComputeLeaderFinalityService::compute_finality(
            &bank,
            dummy_leader_id,
            &mut last_finality_time,
        );
        assert!(bank.finality() != std::usize::MAX);
        assert!(last_finality_time > 0);
    }
}
