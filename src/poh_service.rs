//! The `poh_service` module implements a service that records the passing of
//! "ticks", a measure of time in the PoH stream

use crate::poh_recorder::PohRecorder;
use crate::result::Result;
use crate::service::Service;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::thread::{self, Builder, JoinHandle};
use std::time::Duration;
pub const NUM_TICKS_PER_SECOND: usize = 10;

#[derive(Copy, Clone)]
pub enum Config {
    /// * `Tick` - Run full PoH thread.  Tick is a rough estimate of how many hashes to roll before transmitting a new entry.
    Tick(usize),
    /// * `Sleep`- Low power mode.  Sleep is a rough estimate of how long to sleep before rolling 1 poh once and producing 1
    /// tick.
    Sleep(Duration),
}

impl Default for Config {
    fn default() -> Config {
        // TODO: Change this to Tick to enable PoH
        Config::Sleep(Duration::from_millis(1000 / NUM_TICKS_PER_SECOND as u64))
    }
}

pub struct PohService {
    tick_producer: JoinHandle<Result<()>>,
    pub poh_exit: Arc<AtomicBool>,
}

impl PohService {
    pub fn exit(&self) {
        self.poh_exit.store(true, Ordering::Relaxed);
    }

    pub fn close(self) -> thread::Result<Result<()>> {
        self.exit();
        self.join()
    }

    pub fn new(poh_recorder: PohRecorder, config: Config) -> Self {
        // PohService is a headless producer, so when it exits it should notify the banking stage.
        // Since channel are not used to talk between these threads an AtomicBool is used as a
        // signal.
        let poh_exit = Arc::new(AtomicBool::new(false));
        let poh_exit_ = poh_exit.clone();
        // Single thread to generate ticks
        let tick_producer = Builder::new()
            .name("solana-poh-service-tick_producer".to_string())
            .spawn(move || {
                let mut poh_recorder_ = poh_recorder;
                let return_value = Self::tick_producer(&mut poh_recorder_, config, &poh_exit_);
                poh_exit_.store(true, Ordering::Relaxed);
                return_value
            })
            .unwrap();

        Self {
            tick_producer,
            poh_exit,
        }
    }

    fn tick_producer(poh: &mut PohRecorder, config: Config, poh_exit: &AtomicBool) -> Result<()> {
        loop {
            match config {
                Config::Tick(num) => {
                    for _ in 1..num {
                        poh.hash()?;
                    }
                }
                Config::Sleep(duration) => {
                    sleep(duration);
                }
            }
            poh.tick()?;
            if poh_exit.load(Ordering::Relaxed) {
                debug!("tick service exited");
                return Ok(());
            }
        }
    }
}

impl Service for PohService {
    type JoinReturnType = Result<()>;

    fn join(self) -> thread::Result<Result<()>> {
        self.tick_producer.join()
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, PohService};
    use crate::bank::Bank;
    use crate::mint::Mint;
    use crate::poh_recorder::PohRecorder;
    use crate::result::Result;
    use crate::service::Service;
    use crate::test_tx::test_tx;
    use solana_sdk::hash::hash;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::thread::{Builder, JoinHandle};

    #[test]
    fn test_poh_service() {
        let mint = Mint::new(1);
        let bank = Arc::new(Bank::new(&mint));
        let prev_id = bank.last_id();
        let (entry_sender, entry_receiver) = channel();
        let poh_recorder = PohRecorder::new(bank, entry_sender, prev_id, None);
        let exit = Arc::new(AtomicBool::new(false));

        let entry_producer: JoinHandle<Result<()>> = {
            let poh_recorder = poh_recorder.clone();
            let exit = exit.clone();

            Builder::new()
                .name("solana-poh-service-entry_producer".to_string())
                .spawn(move || {
                    loop {
                        // send some data
                        let h1 = hash(b"hello world!");
                        let tx = test_tx();
                        assert!(poh_recorder.record(h1, vec![tx]).is_ok());

                        if exit.load(Ordering::Relaxed) {
                            break Ok(());
                        }
                    }
                })
                .unwrap()
        };

        const HASHES_PER_TICK: u64 = 2;
        let poh_service = PohService::new(poh_recorder, Config::Tick(HASHES_PER_TICK as usize));

        // get some events
        let mut hashes = 0;
        let mut need_tick = true;
        let mut need_entry = true;
        let mut need_partial = true;

        while need_tick || need_entry || need_partial {
            for entry in entry_receiver.recv().unwrap() {
                if entry.is_tick() {
                    assert!(entry.num_hashes <= HASHES_PER_TICK);

                    if entry.num_hashes == HASHES_PER_TICK {
                        need_tick = false;
                    } else {
                        need_partial = false;
                    }

                    hashes += entry.num_hashes;

                    assert_eq!(hashes, HASHES_PER_TICK);

                    hashes = 0;
                } else {
                    assert!(entry.num_hashes >= 1);
                    need_entry = false;
                    hashes += entry.num_hashes - 1;
                }
            }
        }
        exit.store(true, Ordering::Relaxed);
        poh_service.exit();
        assert!(poh_service.join().is_ok());
        assert!(entry_producer.join().is_ok());
    }

}
