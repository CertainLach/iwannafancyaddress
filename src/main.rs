use clap::Parser;
use rand::{thread_rng, RngCore};
use regex::Regex;
use sp_core::{
    crypto::{AccountId32, Ss58AddressFormat, Ss58Codec},
    sr25519, Pair,
};
use std::fmt::{self, Display};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::{
    thread,
    time::{Duration, SystemTime},
};

#[derive(Parser)]
struct Opts {
    /// Network address format
    ///
    /// Unique is 7391
    #[clap(long, short = 'f')]
    format: u16,
    /// How many threads to use
    #[clap(long, short = 't', default_value = "1")]
    threads: u8,
    /// How many accounts you want to generate
    #[clap(long, short = 'l', default_value = "1")]
    limit: usize,
    /// Regex, which generated address should match
    ///
    /// Make sure your requests are within valid ss58 alphabet:
    /// 1-9, a-z (excl. l), A-Z (excl. I, O)
    regex: String,
}

#[derive(Default, Clone)]
struct Account {
    seed: [u8; 32],
    address: String,
}

impl Account {
    fn generate<R: RngCore>(rng: &mut R, addr_format: u16) -> Account {
        let mut seed = <sr25519::Pair as Pair>::Seed::default();
        rng.fill_bytes(seed.as_mut());
        let pair = sr25519::Pair::from_seed(&seed);

        let address = AccountId32::from(pair.public())
            .to_ss58check_with_version(Ss58AddressFormat::custom(addr_format));
        Self { seed, address }
    }
}
impl Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}, seed = 0x{}", self.address, hex::encode(&self.seed))
    }
}

/// How many accounts should be generated before matching all of them
const THREAD_BATCH_SIZE: usize = 5000;

fn worker_thread(
    tx: Sender<Account>,
    attempts_tx: Sender<u64>,
    kill_pill: Receiver<()>,
    regex: Regex,
    addr_type: u16,
) {
    let mut attempts: u64 = 0;
    let mut thread_rng = thread_rng();
    let mut accounts = Vec::with_capacity(THREAD_BATCH_SIZE);

    loop {
        for _ in 0..THREAD_BATCH_SIZE {
            accounts.push(Account::generate(&mut thread_rng, addr_type));
        }
        attempts += THREAD_BATCH_SIZE as u64;
        for account in accounts.drain(..) {
            if regex.is_match(&account.address) {
                tx.send(account.clone()).unwrap();
            }
        }
        attempts_tx.send(attempts).unwrap();
        attempts = 0;
        match kill_pill.try_recv() {
            Ok(_) | Err(TryRecvError::Disconnected) => {
                break;
            }
            Err(TryRecvError::Empty) => {}
        }
    }
}

fn main() {
    let opts = Opts::parse();

    let regex = Regex::new(&opts.regex).unwrap();

    let (tx, rx) = mpsc::channel();
    let (attempt_count_tx, attempt_count_rx) = mpsc::channel();
    let mut children = Vec::new();
    let mut kill_pills = Vec::new();
    for _ in 0..opts.threads {
        let thread_tx = tx.clone();
        let thread_attempt_count_tx = attempt_count_tx.clone();
        let thread_matcher = regex.clone();
        let (kill_pill_tx, kill_pill_rx) = mpsc::channel();
        let child = thread::spawn(move || {
            worker_thread(
                thread_tx,
                thread_attempt_count_tx,
                kill_pill_rx,
                thread_matcher,
                opts.format,
            )
        });
        kill_pills.push(kill_pill_tx);
        children.push(child);
    }

    let start_time = SystemTime::now();
    let mut matches_found: usize = 0;
    let mut total_attempts: u64 = 0;
    while matches_found < opts.limit {
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(matched) => {
                matches_found += 1;
                println!("{}. {}", matches_found, matched);
            }
            Err(RecvTimeoutError::Disconnected) => panic!("wallet tx disconnected"),
            Err(RecvTimeoutError::Timeout) => {}
        }
        total_attempts += attempt_count_rx.try_iter().sum::<u64>();

        if let Ok(elapsed) = start_time.elapsed() {
            let elapsed_secs = elapsed.as_secs();
            if elapsed_secs != 0 {
                eprintln!(
                    "{} attempts per second, {:.6} matches per second. {:.10}% of total matched: {}/{}",
                    total_attempts / elapsed.as_secs(),
                    matches_found as f64 / elapsed.as_secs() as f64,
                    matches_found as f64 / total_attempts as f64,
                    matches_found,
                    opts.limit,
                )
            }
        }
    }

    for pill in kill_pills {
        pill.send(()).unwrap();
    }
    for child in children {
        child.join().unwrap();
    }
}
