//! `cybernode mine` — the reference miner of the phase-1 root chain
//! (rewards.md §7, chain-economy §7.1, chain-wire §7).
//!
//! An attempt is a ticket: the miner takes a template from a node, fetches
//! the game the template names (`GET /game/{beacon}`, once per epoch),
//! writes a nonce into the template, computes the ticket of that template
//! — `W_TICKET + 1` cold solves of the epoch engine — commits it into
//! `sample` and hashes the 200 bytes. The block id below the target is the
//! block; nothing else is ground. The whole template seeds the ticket, so
//! no field is free to vary without a fresh sample: the GPU hash kernels of
//! the earlier miner have no work here and are gone (rewards.md §7 step 3,
//! §16 choice 1).
//!
//! Parallelism is across attempts: `workers` threads, each on its own nonce
//! stream, each attempt's solves run with the game's own threads
//! ([`crate::chain::ticket::solve_threads`]).

pub mod api;
mod asert_sim;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::header::Header;
use super::pow::id_meets_target;
use super::ticket::EpochGame;
use super::u256::U256;
use api::{Client, Template, Verdict};

/// `cybernode mine` settings.
#[derive(Clone, Debug)]
pub struct Settings {
    pub chain: String,
    pub node: String,
    pub address: String,
    /// attempt threads (0: every core)
    pub workers: usize,
    /// Seconds between `GET /tip` polls.
    pub tip_poll: Duration,
    /// Seconds after which a template is refreshed even if the tip stayed.
    pub refresh: Duration,
    /// Seconds between rate lines.
    pub report: Duration,
}

/// What one mined template ended with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A block was accepted by the node.
    Accepted { height: u64, id: String, nonce: u128, attempts: u64 },
    /// The tip moved or the template aged: refresh.
    Refresh,
    /// The node stored the block and verifies its sample when its
    /// `V_BUDGET` renews (chain.md §5.5 step 8): nothing to resubmit.
    Waiting { height: u64, reason: String },
    /// The node rejected the solved header (a bug on one side; logged).
    Rejected(String),
}

/// Attempts per second for display only (never consensus).
pub fn rate(n: u64, dt: Duration) -> f64 {
    if dt.is_zero() { 0.0 } else { n as f64 / dt.as_secs_f64() }
}

/// A session salt: the high 64 bits of every nonce this process mines.
pub fn salt() -> u64 {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut m = Vec::new();
    m.extend_from_slice(&t.as_nanos().to_le_bytes());
    m.extend_from_slice(&std::process::id().to_le_bytes());
    let h = hemera::hash(&m);
    u64::from_le_bytes(h.as_bytes()[..8].try_into().expect("8 bytes"))
}

/// One attempt (rewards.md §7 steps 1–3): the header with `nonce`, its
/// ticket solved on `game` and committed, and whether its id is below the
/// target. `threads` is the solve parallelism inside this attempt.
pub fn attempt(game: &EpochGame, template: &Header, nonce: u128, target: U256, threads: usize) -> (Header, bool) {
    let mut h = *template;
    h.nonce = nonce.to_le_bytes();
    game.solve(&mut h, threads);
    let ok = id_meets_target(&h.id(), target);
    (h, ok)
}

/// The worker count of `settings.workers` (0: every core).
pub fn workers_of(s: &Settings) -> usize {
    if s.workers == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()) } else { s.workers }
}

/// Mine one template on `workers` threads until a block is found, the tip
/// moves or `stop` is raised. `log` receives progress lines.
pub fn mine_template(client: &Client, game: &EpochGame, template: &Template, salt: u64, s: &Settings, stop: &AtomicBool, log: &dyn Fn(&str)) -> Result<Outcome, String> {
    let header = template.header()?;
    let target = template.target_u256()?;
    if header.bits != template.bits_u32()? {
        return Err("template bits disagree with its header".into());
    }
    let workers = workers_of(s);
    let threads = super::ticket::solve_threads(game.width, workers, game.base.len());
    let started = Instant::now();
    let attempts = AtomicU64::new(0);
    let found: Mutex<Option<(Header, u128)>> = Mutex::new(None);
    let done = AtomicBool::new(false);
    log(&format!(
        "template height {} prev {}… bits {:#010x} game epoch {} ({} players, {} axons, W {}): {workers} workers × {threads} solve threads",
        template.height,
        &template.prev[..12],
        header.bits,
        game.epoch,
        game.players.len(),
        game.base.len(),
        game.width
    ));
    std::thread::scope(|sc| {
        for w in 0..workers {
            let (attempts, found, done) = (&attempts, &found, &done);
            sc.spawn(move || {
                // the worker's nonce stream: salt ‖ worker in the high bits, a counter below
                let base = ((salt as u128) << 64) | ((w as u128) << 48);
                let mut i = 0u128;
                while !done.load(Relaxed) && !stop.load(Relaxed) {
                    let nonce = base | (i & ((1u128 << 48) - 1));
                    let (h, ok) = attempt(game, &header, nonce, target, threads);
                    attempts.fetch_add(1, Relaxed);
                    if ok {
                        *found.lock().expect("found") = Some((h, nonce));
                        done.store(true, Relaxed);
                        return;
                    }
                    i += 1;
                }
            });
        }
        // the watcher: reports, the tip poll, the template's age
        let mut last_report = started;
        let mut last_tip = started;
        while !done.load(Relaxed) && !stop.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            let now = Instant::now();
            if now - last_report >= s.report {
                let n = attempts.load(Relaxed);
                log(&format!("{:.2} tickets/s · {n} attempts · height {}", rate(n, now - started), template.height));
                last_report = now;
            }
            if now - last_tip >= s.tip_poll {
                last_tip = now;
                match client.tip() {
                    Ok(t) if t.id != template.prev => {
                        log(&format!("tip moved to height {} — refreshing", t.height));
                        done.store(true, Relaxed);
                    }
                    Ok(_) => {}
                    Err(e) => log(&format!("{e} (still mining)")),
                }
            }
            if now - started >= s.refresh {
                done.store(true, Relaxed);
            }
            let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            if template.expires != 0 && unix >= template.expires {
                done.store(true, Relaxed);
            }
        }
        done.store(true, Relaxed);
    });
    let n = attempts.load(Relaxed);
    let Some((solved, nonce)) = found.into_inner().expect("found") else {
        return Ok(Outcome::Refresh);
    };
    log(&format!("found nonce {nonce:#034x} after {n} attempts, {:.1}s", started.elapsed().as_secs_f64()));
    let verdict = Client::retry(|| client.submit(&solved.encode()), 5, Duration::from_secs(8), |m| log(m))?;
    Ok(match verdict {
        Verdict::Accepted(a) => Outcome::Accepted { height: a.height, id: a.id, nonce, attempts: n },
        Verdict::Stale => Outcome::Refresh,
        Verdict::Waiting(reason) => Outcome::Waiting { height: template.height, reason },
        Verdict::Rejected(r) => Outcome::Rejected(r),
    })
}

/// `cybernode mine`: the loop over templates. Returns only on an
/// unrecoverable error or when `stop` is raised.
pub fn run(s: &Settings, stop: &AtomicBool, log: &dyn Fn(&str)) -> Result<(), String> {
    let client = Client::new(&s.node, &s.address);
    let salt = salt();
    let workers = workers_of(s);
    log(&format!("cybernode mine · chain {} · node {} · {workers} workers · to {}", s.chain, s.node, s.address));
    let mut blocks = 0u64;
    let mut game: Option<EpochGame> = None;
    while !stop.load(Relaxed) {
        let template = Client::retry(|| client.template(), 0, Duration::from_secs(30), |m| log(m))?;
        if template.chain != s.chain {
            return Err(format!("node serves chain `{}`, not `{}`", template.chain, s.chain));
        }
        // the game once per epoch: the template names it by its beacon
        let beacon = template.game.beacon_id()?;
        if game.as_ref().is_none_or(|g| g.beacon != beacon) {
            let wire = Client::retry(|| client.game(&template.game.beacon), 0, Duration::from_secs(30), |m| log(m))?;
            let t0 = Instant::now();
            let g = wire.build(workers)?;
            log(&format!("game of epoch {} built: {} players, {} axons, cluster {}…, {:.2}s", g.epoch, g.players.len(), g.base.len(), &template.game.cluster[..12], t0.elapsed().as_secs_f64()));
            game = Some(g);
        }
        let g = game.as_ref().expect("a game");
        match mine_template(&client, g, &template, salt, s, stop, log)? {
            Outcome::Accepted { height, id, nonce, attempts } => {
                blocks += 1;
                log(&format!("block {height} accepted: {id} (nonce {nonce:#x}, {attempts} attempts); {blocks} this session"));
            }
            Outcome::Refresh => {}
            Outcome::Waiting { height, reason } => log(&format!("block {height} stored, its sample queued behind the node's verification budget: {reason}")),
            Outcome::Rejected(r) => log(&format!("rejected: {r}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_display_only() {
        assert_eq!(rate(100, Duration::from_secs(2)), 50.0);
        assert_eq!(rate(1, Duration::ZERO), 0.0);
        assert_eq!(workers_of(&Settings { chain: String::new(), node: String::new(), address: String::new(), workers: 3, tip_poll: Duration::ZERO, refresh: Duration::ZERO, report: Duration::ZERO }), 3);
    }
}
