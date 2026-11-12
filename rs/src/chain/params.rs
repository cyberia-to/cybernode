//! §0 constants, the rule-set activation table and the embedded chain ids.
//!
//! Every number in `specs/chain.md` §0 lives here and nowhere else.

use super::header::BlockId;
use super::u256::U256;

/// Header size in bytes (§1.1): the 168-byte template and the 32-byte
/// sample commitment at [`SAMPLE_OFFSET`]; 200 = 3 × 56 + 32, four hemera
/// permutations (rewards.md §7 step 3).
pub const HEADER_BYTES: usize = 200;
/// The template: the header's first 168 bytes, the seed of its sample
/// (rewards.md §7 step 1).
pub const TEMPLATE_BYTES: usize = 168;
/// Offset of `sample` in the header (§1.1).
pub const SAMPLE_OFFSET: usize = 168;
/// Target block interval `T`, seconds (§1.5).
pub const BLOCK_TIME: i64 = 30;
/// ASERT half-life `H`, seconds (§1.5).
pub const HALF_LIFE: i64 = 600;
/// Median-time-past window, headers (§1.4).
pub const MTP_WINDOW: usize = 11;
/// Maximum future drift against the validating node's clock, seconds (§1.4).
pub const MAX_FUTURE_DRIFT: u64 = 120;
/// Compact form of `POW_LIMIT` = 2^252 (§1.3 amended, rewards.md §7
/// "Difficulty"): 16 expected attempts, about 35 s on one laptop core at
/// pussy's size and six to seven minutes at bostrom's, so neither chain
/// stalls below one laptop core. Bostrom's `GENESIS_BITS` is this floor.
pub const POW_LIMIT_BITS: u32 = 0x2010_0000;
/// Compact form of pussy's genesis target 2^249 (§1.3, §4; rewards.md §7):
/// 128 expected attempts, about 25 s for a laptop making 5.06 samples a
/// second (tru#46 §3.2). The constant the conformance net and the test
/// genesis files anchor at; the launch chains anchor at [`genesis_bits`].
pub const GENESIS_BITS: u32 = 0x2002_0000;

/// `GENESIS_BITS` per chain (chain.md §0 amended): pussy 2^249, bostrom the
/// floor 2^252 (one sample per 7–9 s on twelve threads); any other chain
/// pussy's.
pub fn genesis_bits(chain: &str) -> u32 {
    match chain {
        "bostrom" => POW_LIMIT_BITS,
        _ => GENESIS_BITS,
    }
}

// ── the settlement ticket (rewards.md §7, chain-economy §0 amended) ────

/// `W_TICKET` per chain: the entries of a ticket,
/// `max(2, 2^⌊log2(2.5 s / t_solve)⌋)` from the chain's one-thread solve —
/// pussy 64 (0.036 s, tru#46 §3.1), bostrom 2 (6.0 s, tru epoch receipt
/// §6.3); `pussy-rc` is pussy's burial state under another name and takes
/// pussy's. A dev or test net reads `SOFT3_DEV_W_TICKET` (2 when unset):
/// its graph is not the launch chains' and its solve not measured.
pub fn w_ticket(chain: &str) -> usize {
    match chain {
        "pussy" | "pussy-rc" => 64,
        "bostrom" => 2,
        _ => std::env::var("SOFT3_DEV_W_TICKET").ok().and_then(|v| v.trim().parse().ok()).filter(|w| *w >= 1).unwrap_or(2),
    }
}

/// `K_MIN`: the sampled marginals a player needs to be paid by their mean
/// (rewards.md §7 "Deadline, k_min and the payout"; chain-economy §5.2
/// amended).
pub const K_MIN: usize = 64;

/// `V_BUDGET = ⌊0.5·T / t_verify⌋`: full sample verifications a node runs
/// per block interval, all peers together (rewards.md §7 "Verification";
/// chain.md §5.5 amended): pussy 6 (t_verify 2.2 s), bostrom 2 (7 s on
/// twelve threads); `pussy-rc` takes pussy's. A dev or test net reads
/// `SOFT3_DEV_V_BUDGET`, unbounded when unset: its blocks come faster than
/// the interval the budget renews on.
pub fn v_budget(chain: &str) -> usize {
    match chain {
        "pussy" | "pussy-rc" => 6,
        "bostrom" => 2,
        _ => std::env::var("SOFT3_DEV_V_BUDGET").ok().and_then(|v| v.trim().parse().ok()).filter(|b| *b >= 1).unwrap_or(usize::MAX),
    }
}

/// `K_BLOCK`: folded tickets a block admits beyond its own (chain-economy
/// §7.3): 0 while the accumulator's condition is false.
pub const K_BLOCK: usize = 0;
/// Header + body budget, bytes (§1.6).
pub const MAX_BLOCK_BYTES: usize = 262_144;
/// Transactions per block including the coinbase (§1.6).
pub const MAX_BLOCK_TXS: usize = 1_024;
/// Largest transaction, bytes (§2).
pub const MAX_TX_BYTES: usize = 512;
/// Fee floor, native units per transaction byte (§2.6).
pub const MIN_FEE_PER_BYTE: u64 = 1;
/// Blocks before a coinbase output is spendable (§2.2).
pub const COINBASE_MATURITY: u64 = 100;
/// Blocks after which a block is final for every node that saw it (§5.2).
pub const FINALITY_DEPTH: u64 = 120;
/// Checkpoint cadence, blocks (§5.3).
pub const CHECKPOINT_INTERVAL: u64 = 120;
/// Last height a checkpoint is honored at (§5.3).
pub const CHECKPOINT_SUNSET: u64 = 525_600;
/// Blocks per year at `BLOCK_TIME` (§3.5).
pub const BLOCKS_PER_YEAR: u64 = 1_051_200;
/// `SUBSIDY = floor(S0 / SUBSIDY_DIVISOR)`: 1 % of genesis supply per year (§3.5).
pub const SUBSIDY_DIVISOR: u64 = 105_120_000;
/// Orphan pool size, headers (§5.4).
pub const ORPHAN_POOL_HEADERS: usize = 1_024;
/// Orphan pool age, seconds (§5.4).
pub const ORPHAN_POOL_SECONDS: u64 = 600;
/// Nonces ahead of the current one a mempool admits (§2.5).
pub const NONCE_WINDOW: u64 = 16;
/// Deepest reorganization a node performs (§5.2).
pub const MAX_REORG: u64 = FINALITY_DEPTH - 1;

// ── the focus economy (chain-economy §0) ───────────────────────────────

/// `EPOCH_LENGTH` (L): blocks per epoch; the boundary of epoch E is height
/// `E · L` (chain-economy §2.1–§2.2).
pub const EPOCH_LENGTH: u64 = 240;
/// `SETTLE_DELAY` (D) = `FINALITY_DEPTH`: settlement of epoch E is a step of
/// the block at `boundary(E + 1) + D` — the window of E's tickets is epoch
/// E + 1 (chain-economy §2.5 amended, rewards.md §7).
pub const SETTLE_DELAY: u64 = FINALITY_DEPTH;
/// `MIN_LOCK`: blocks a position's stake stays locked after its last
/// increase (chain-economy §3.4 amended). Must be at least `2L + D`: a
/// position increased in epoch E is still locked at
/// `settle(E) = boundary(E + 1) + D`.
pub const MIN_LOCK: u64 = 20_160;
/// `FOCUS_MAX_BPS`: the focus pool's cap on the subsidy, basis points (§6.1).
pub const FOCUS_MAX_BPS: u64 = 5_000;
/// `POOL_RELEASE`: an epoch's budget is `pool / POOL_RELEASE` (§5.5).
pub const POOL_RELEASE: u64 = 4;
/// `R_MAX_PPM`: a position earns at most this many ppm of its added stake
/// per settlement (§5.5).
pub const R_MAX_PPM: u64 = 10_000;
/// `HWM_KEEP` = 65,410 / 65,536 per epoch: the high-water mark decay
/// (half-life 360 epochs, §5.3).
pub const HWM_KEEP_NUM: u64 = 65_410;
pub const HWM_KEEP_DEN: u64 = 65_536;
/// `PHI_STEPS`: tri-kernel steps per cold solve (§4.4); tru's `EpochParams`
/// fixes the value at the phase1.toml pin.
pub const PHI_STEPS: usize = 48;
/// `ATTR_NODES`: Gauss–Legendre nodes of the attribution path gradient
/// (§5.1, amended 2026-10-17 from the one-node midpoint; chosen against
/// exact Shapley in tru audit/epoch-settlement-2026-10.md §6). The ground
/// weight of §4.4 (`GROUND` = in-weight / 4) is tru's `EpochParams` default.
pub const ATTR_NODES: usize = 4;
/// `SETTLE_BUDGET`: seconds of reference settlement compute (§7.2).
pub const SETTLE_BUDGET_SECS: u64 = 900;
/// `RHO_LAUNCH`, `KARMA_LAUNCH`: the surprise weight and karma of every
/// active position and neuron at genesis (§5.4, §3.7). Both are 1, so they
/// multiply nothing; named here so the reports rule set has one place to
/// replace them.
pub const RHO_LAUNCH: u64 = 1;
pub const KARMA_LAUNCH: u64 = 1;

/// Economy domain tags (chain-economy §3.1), each ending in NUL.
pub const DOMAIN_CARD: &[u8] = b"soft3/chain/card/v1\0";
pub const DOMAIN_STAKE: &[u8] = b"soft3/chain/stake/v1\0";
pub const DOMAIN_FOCUS: &[u8] = b"soft3/chain/focus/v1\0";
pub const DOMAIN_PHI: &[u8] = b"soft3/chain/phi/v1\0";
pub const DOMAIN_EPOCH: &[u8] = b"soft3/chain/epoch/v1\0";
pub const DOMAIN_PAYOUT_LEAF: &[u8] = b"soft3/chain/payout-leaf/v1\0";
pub const DOMAIN_PAYOUT_NODE: &[u8] = b"soft3/chain/payout-node/v1\0";

/// The epoch clock of one chain. The launch chains run [`EpochClock::SPEC`]
/// and nothing else; a private test or dev net may shorten the clock (the
/// way a test genesis relaxes the PoW limit) so a settlement happens inside
/// a test run. The rules are the same functions of `(L, D, MIN_LOCK)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochClock {
    pub length: u64,
    pub delay: u64,
    pub min_lock: u64,
}

impl EpochClock {
    pub const SPEC: EpochClock = EpochClock { length: EPOCH_LENGTH, delay: SETTLE_DELAY, min_lock: MIN_LOCK };

    /// `L ≥ D + 1` (one settlement in flight) and `MIN_LOCK ≥ 2L + D` (stake
    /// added in an epoch is still locked at its settlement), §2.2, §3.4.
    pub fn valid(&self) -> bool {
        self.delay >= 1 && self.length > self.delay && self.min_lock >= 2 * self.length + self.delay
    }

    /// `epoch(h) = ceil(h / L)`; height 0 is epoch 0 (§2.1).
    pub fn epoch(&self, h: u64) -> u64 {
        h.div_ceil(self.length)
    }

    /// `boundary(E) = E · L`, the last block of epoch E.
    pub fn boundary(&self, e: u64) -> u64 {
        e * self.length
    }

    pub fn is_boundary(&self, h: u64) -> bool {
        h % self.length == 0
    }

    /// `settle(E) = boundary(E + 1) + D`: the window of E's tickets is epoch
    /// E + 1, in and final by the reorg limit (rewards.md §7).
    pub fn settle(&self, e: u64) -> u64 {
        self.boundary(e + 1) + self.delay
    }

    /// The epoch E ≥ 1 whose settlement is a step of the block at `h`.
    pub fn settles_at(&self, h: u64) -> Option<u64> {
        (h > self.delay && (h - self.delay) % self.length == 0).then(|| (h - self.delay) / self.length).filter(|e| *e >= 2).map(|e| e - 1)
    }

    /// The epoch whose game the block at `h ≥ 1` samples: `epoch(h) − 1`
    /// (rewards.md §7: the window of E's game is epoch E + 1; epoch 0's game
    /// is empty).
    pub fn game_of(&self, h: u64) -> u64 {
        self.epoch(h).saturating_sub(1)
    }

    /// A dev or test clock from `SOFT3_DEV_EPOCH=L:D:MIN_LOCK`; `None` when
    /// unset or invalid. Never consulted for a launch chain.
    pub fn from_env() -> Option<EpochClock> {
        let v = std::env::var("SOFT3_DEV_EPOCH").ok()?;
        let mut it = v.split(':').map(|x| x.trim().parse::<u64>());
        let (Some(Ok(length)), Some(Ok(delay)), Some(Ok(min_lock))) = (it.next(), it.next(), it.next()) else { return None };
        let c = EpochClock { length, delay, min_lock };
        c.valid().then_some(c)
    }
}

/// The rule set this implementation speaks (§7.1).
pub const RULE_SET: u16 = 1;

/// Activation table: `(rule_set, height)` in ascending height (§7.1).
/// Rule set 1 activates at height 0; a later release appends rule set 2.
pub const ACTIVATIONS: &[(u16, u64)] = &[(RULE_SET, 0)];

/// One entry of the compiled revert table (§5.6): the chain returns to the
/// block `keep` at `height` (H), every id in `vetoed` and its descendants is
/// invalid, the difficulty re-anchors at `anchor_time`, and `rule_set` is
/// the rule set from `H + 1` on (it appends `(rule_set, H + 1)` to
/// [`ACTIVATIONS`]). The runtime form, with owned vetoed ids and the
/// rehearsal flag, is `revert::RevertEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revert {
    pub height: u64,
    pub keep: BlockId,
    pub vetoed: &'static [BlockId],
    pub anchor_time: u64,
    pub rule_set: u16,
}

/// The revert table of this release (§5.6). Entries are sorted by `height`;
/// the genesis release carries the machinery with an empty table.
pub const REVERTS: &[Revert] = &[];

/// The rule set in force at `height`, or `None` when the table ends before
/// it — a node then stops accepting blocks at that height (§7.1). This is
/// the compiled [`ACTIVATIONS`] alone; the engine uses
/// `RevertTable::rule_set_at`, which adds the revert entries.
pub fn rule_set_at(height: u64) -> Option<u16> {
    ACTIVATIONS
        .iter()
        .filter(|(_, h)| *h <= height)
        .map(|(r, _)| *r)
        .max()
}

/// Dialect URN of a genesis file (§4.1).
pub const GENESIS_DIALECT: &str = "urn:cyberia:chain-genesis:1";

/// Domain tags (§1.7, §2.1, §2.8, §5.3, chain-wire §2). Each ends in NUL.
pub const DOMAIN_LEAF: &[u8] = b"soft3/chain/leaf/v1\0";
pub const DOMAIN_NODE: &[u8] = b"soft3/chain/node/v1\0";
pub const DOMAIN_TXID: &[u8] = b"soft3/chain/txid/v1\0";
pub const DOMAIN_SIGN: &[u8] = b"soft3/chain/sign/v1\0";
pub const DOMAIN_LEGACY: &[u8] = b"soft3/chain/legacy/v1\0";
pub const DOMAIN_CHECKPOINT: &[u8] = b"soft3/chain/checkpoint/v1\0";
/// The revert statement's own tag (§5.6): never the checkpoint's.
pub const DOMAIN_REVERT: &[u8] = b"soft3/chain/revert/v1\0";
pub const DOMAIN_TOPIC: &[u8] = b"soft3/chain/topic/v1\0";

/// `POW_LIMIT` as a target.
///
/// Under `cfg(test)` only, a thread-local override lets the simulated
/// miner of the engine tests work at trivial targets (chain-conformance
/// §9 "a private test genesis"); release binaries have no such hook.
pub fn pow_limit() -> U256 {
    #[cfg(test)]
    if let Some(limit) = test_pow::LIMIT.with(|l| l.get()) {
        return limit;
    }
    #[cfg(feature = "sim")]
    if sim_pow::relaxed() {
        return U256::ONE << 255;
    }
    U256::ONE << 252
}

/// The out-of-tree twin of [`test_pow`], behind the `sim` feature: the
/// network simulator (cyberia-to/simulation) runs many in-process nodes on
/// a private test genesis and needs the same trivial targets on every
/// thread (settlement workers included), so the switch is process-wide.
/// Without the feature the module does not exist and `POW_LIMIT` is 2^236;
/// a node binary never enables it. The launch genesis files keep their
/// 2^232 target either way; the simulator runs them in a process that
/// never calls [`sim_pow::relax`].
#[cfg(feature = "sim")]
pub mod sim_pow {
    use std::sync::atomic::{AtomicBool, Ordering};

    static RELAXED: AtomicBool = AtomicBool::new(false);

    /// Raise the limit for the whole process; targets up to 2^255 become legal.
    pub fn relax() {
        RELAXED.store(true, Ordering::SeqCst);
    }

    pub fn relaxed() -> bool {
        RELAXED.load(Ordering::SeqCst)
    }

    /// Canonical bits of 2^255 (one hash in two), as `test_pow::EASY_BITS`.
    pub const EASY_BITS: u32 = 0x2100_8000;
}

#[cfg(test)]
pub mod test_pow {
    use super::U256;
    use std::cell::Cell;

    thread_local! {
        pub static LIMIT: Cell<Option<U256>> = const { Cell::new(None) };
    }

    /// Raise the limit on this thread; targets up to 2^255 become legal.
    pub fn relax() {
        LIMIT.with(|l| l.set(Some(U256::ONE << 255)));
    }

    /// Canonical bits of 2^255: the test genesis target (one hash in two).
    pub const EASY_BITS: u32 = 0x2100_8000;
}

/// Pussy's genesis target 2^249 (`GENESIS_BITS`).
pub fn genesis_target() -> U256 {
    U256::ONE << 249
}

/// Subsidy per block for a genesis supply (§3.5). No floats, floor division.
pub fn subsidy(s0: u64) -> u64 {
    s0 / SUBSIDY_DIVISOR
}

/// An embedded chain: name, hrp, chain id (= genesis file particle) and the
/// id of block 0. Non-zero ids are pinned: the node refuses a genesis file
/// whose particle differs, and a kernel whose block 0 differs. pussy is
/// pinned to the values blessed 2026-10-17 in `conformance/chain-genesis.toml`
/// (and `chain.snap`). Zero ids mean "not yet pinned": bostrom waits until
/// pussy's genesis is covered end to end; until pinned, `--genesis` is
/// mandatory for that chain and any file naming it is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedChain {
    pub name: &'static str,
    pub hrp: &'static str,
    pub chain_id: [u8; 32],
    pub genesis_block: [u8; 32],
    pub genesis_time: u64,
}

impl EmbeddedChain {
    /// True once the chain id is pinned (non-zero).
    pub fn pinned(&self) -> bool {
        self.chain_id != [0; 32]
    }
}

/// Decode 64 hex digits at compile time; a typo in a pin fails the build.
const fn h32(s: &str) -> [u8; 32] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("pin is not lowercase hex"),
        }
    }
    let b = s.as_bytes();
    assert!(b.len() == 64, "pin is not 64 hex digits");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(b[2 * i]) << 4) | nibble(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// The two chains one binary serves (§4.4).
pub const CHAINS: &[EmbeddedChain] = &[
    EmbeddedChain {
        name: "pussy",
        hrp: "pussy",
        // re-blessed 2026-11-12: the file carries GENESIS_BITS 0x20020000 (2^249) and the
        // 200-byte block 0 (rewards.md §7, chain.md §0/§1.1 amended); bae85f40… / 73208ee2…
        // were the 0x1e010000 file and its 168-byte block 0 (conformance/chain-genesis.toml)
        chain_id: h32("a94ee10120f2d24009f091eb5f620de0e65f67ef2619073988766d50be566d7f"),
        genesis_block: h32("b5aca6a0446c3fd3080dc8cac4a03c3b0bafc41fe9847b9779f19a6faaeb6e06"),
        genesis_time: 1_793_885_820,
    },
    EmbeddedChain {
        // unpinned on purpose: bostrom waits for pussy's genesis coverage
        name: "bostrom",
        hrp: "bostrom",
        chain_id: [0; 32],
        genesis_block: [0; 32],
        genesis_time: 1_793_884_962,
    },
];

/// Look up an embedded chain by name.
pub fn embedded(name: &str) -> Option<&'static EmbeddedChain> {
    CHAINS.iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> [u8; 32] {
        let b: Vec<u8> = (0..64).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
        b.try_into().unwrap()
    }

    #[test]
    fn pussy_pins_match_the_blessed_files_and_bostrom_is_unpinned() {
        // soft3 owns the blessed files; resolve its directory the way chain_snap_path does.
        let snap_path = crate::chain::conformance::chain_snap_path();
        let root = snap_path.parent().unwrap();
        let pussy = embedded("pussy").unwrap();
        assert!(pussy.pinned());
        assert!(!embedded("bostrom").unwrap().pinned());

        let toml_text = std::fs::read_to_string(root.join("chain-genesis.toml")).unwrap();
        let table: toml::Table = toml_text.parse().unwrap();
        let sect = table["pussy"].as_table().unwrap();
        assert_eq!(pussy.chain_id, unhex(sect["particle"].as_str().unwrap()));
        assert_eq!(pussy.genesis_block, unhex(sect["block0_id"].as_str().unwrap()));

        let snap = std::fs::read_to_string(&snap_path).unwrap();
        let entries = crate::chain::conformance::parse(&snap);
        let find = |scenario: &str| entries.iter().find(|e| e.key == "chain::genesis@v1" && e.scenario == scenario).unwrap().value;
        assert_eq!(pussy.chain_id, find("pussy particle"));
        assert_eq!(pussy.genesis_block, find("pussy block0_id"));
    }

    #[test]
    fn subsidy_is_one_percent_per_year_floored() {
        assert_eq!(subsidy(105_120_000), 1);
        assert_eq!(subsidy(105_119_999), 0);
        assert_eq!(subsidy(1_152_075_800_167_936_256), 10_959_625_191);
        assert_eq!(BLOCKS_PER_YEAR * 100, SUBSIDY_DIVISOR);
    }

    #[test]
    fn ticket_constants_per_chain() {
        assert_eq!((w_ticket("pussy"), w_ticket("bostrom"), w_ticket("pussy-rc")), (64, 2, 64));
        assert_eq!((v_budget("pussy"), v_budget("bostrom"), v_budget("pussy-rc")), (6, 2, 6));
        assert_eq!(v_budget("test"), usize::MAX);
        assert_eq!((genesis_bits("pussy"), genesis_bits("bostrom")), (0x2002_0000, 0x2010_0000));
        assert_eq!(super::super::header::decode_bits(GENESIS_BITS), Ok(U256::ONE << 249));
        assert_eq!(super::super::header::decode_bits(POW_LIMIT_BITS), Ok(U256::ONE << 252));
        assert_eq!((K_MIN, K_BLOCK), (64, 0));
    }

    #[test]
    fn rule_set_table_starts_at_genesis() {
        assert_eq!(rule_set_at(0), Some(1));
        assert_eq!(rule_set_at(u64::MAX), Some(1));
        assert_eq!(MAX_REORG, 119);
    }

    #[test]
    fn epoch_clock_matches_chain_economy_section_10() {
        let c = EpochClock::SPEC;
        assert!(c.valid());
        assert_eq!((c.epoch(0), c.epoch(1), c.epoch(240), c.epoch(241)), (0, 1, 1, 2));
        assert_eq!(c.settle(1), 600, "the first payout, of epoch 1's game, lands at height 600");
        assert_eq!(c.settles_at(600), Some(1));
        assert_eq!(c.settles_at(840), Some(2));
        assert_eq!((c.settles_at(120), c.settles_at(360), c.settles_at(599), c.settles_at(240)), (None, None, None, None));
        assert_eq!((c.game_of(1), c.game_of(240), c.game_of(241), c.game_of(480), c.game_of(481)), (0, 0, 1, 1, 2));
        assert!(MIN_LOCK >= 2 * EPOCH_LENGTH + SETTLE_DELAY);
        assert_eq!(MIN_LOCK / (2 * EPOCH_LENGTH + SETTLE_DELAY), 33);
        assert!(!EpochClock { length: 10, delay: 10, min_lock: 30 }.valid());
        assert!(!EpochClock { length: 10, delay: 5, min_lock: 24 }.valid());
        assert!(EpochClock { length: 10, delay: 5, min_lock: 25 }.valid());
    }

    #[test]
    fn embedded_chains_have_the_launch_times() {
        assert_eq!(embedded("pussy").unwrap().genesis_time, 1_793_885_820);
        assert_eq!(embedded("bostrom").unwrap().genesis_time, 1_793_884_962);
        assert!(embedded("spacepussy-test").is_none());
    }
}
