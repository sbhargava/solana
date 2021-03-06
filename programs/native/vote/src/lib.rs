//! Vote program
//! Receive and processes votes from validators

use log::*;
use solana_sdk::account::KeyedAccount;
use solana_sdk::native_program::ProgramError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::solana_entrypoint;
use solana_sdk::vote_program::*;
use std::collections::VecDeque;

solana_entrypoint!(entrypoint);
fn entrypoint(
    _program_id: &Pubkey,
    keyed_accounts: &mut [KeyedAccount],
    data: &[u8],
    _tick_height: u64,
) -> Result<(), ProgramError> {
    solana_logger::setup();

    trace!("process_instruction: {:?}", data);
    trace!("keyed_accounts: {:?}", keyed_accounts);

    // all vote instructions require that accounts_keys[0] be a signer
    if keyed_accounts[0].signer_key().is_none() {
        error!("account[0] is unsigned");
        Err(ProgramError::InvalidArgument)?;
    }

    match bincode::deserialize(data) {
        Ok(VoteInstruction::RegisterAccount) => {
            if !check_id(&keyed_accounts[1].account.owner) {
                error!("account[1] is not assigned to the VOTE_PROGRAM");
                Err(ProgramError::InvalidArgument)?;
            }

            // TODO: a single validator could register multiple "vote accounts"
            // which would clutter the "accounts" structure. See github issue 1654.
            let vote_state = VoteProgram {
                votes: VecDeque::new(),
                node_id: *keyed_accounts[0].signer_key().unwrap(),
            };

            vote_state.serialize(&mut keyed_accounts[1].account.userdata)?;

            Ok(())
        }
        Ok(VoteInstruction::NewVote(vote)) => {
            if !check_id(&keyed_accounts[0].account.owner) {
                error!("account[0] is not assigned to the VOTE_PROGRAM");
                Err(ProgramError::InvalidArgument)?;
            }
            debug!("{:?} by {}", vote, keyed_accounts[0].signer_key().unwrap());
            solana_metrics::submit(
                solana_metrics::influxdb::Point::new("vote-native")
                    .add_field("count", solana_metrics::influxdb::Value::Integer(1))
                    .to_owned(),
            );

            let mut vote_state = VoteProgram::deserialize(&keyed_accounts[0].account.userdata)?;

            // TODO: Integrity checks
            // a) Verify the vote's bank hash matches what is expected
            // b) Verify vote is older than previous votes

            // Only keep around the most recent MAX_VOTE_HISTORY votes
            if vote_state.votes.len() == MAX_VOTE_HISTORY {
                vote_state.votes.pop_front();
            }

            vote_state.votes.push_back(vote);
            vote_state.serialize(&mut keyed_accounts[0].account.userdata)?;

            Ok(())
        }
        Err(_) => {
            info!("Invalid transaction instruction userdata: {:?}", data);
            Err(ProgramError::InvalidUserdata)
        }
    }
}
