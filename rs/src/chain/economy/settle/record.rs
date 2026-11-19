//! The epoch record of chain-economy §5.6: the 218 bytes a settlement
//! block commits to through `time[settle(E)] = hemera(D_EPOCH ‖ record)`,
//! with the lottery's four fields (`players`, `samples`, `by_mean`,
//! `value`) after `payouts` (§5.6 amended).

use super::super::super::params::DOMAIN_EPOCH;
use super::super::super::state::Id;

/// The epoch record of §5.6.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub epoch: u64,
    pub boundary: u64,
    pub beacon: Id,
    pub boundary_root: Id,
    pub particles: u32,
    pub axons: u32,
    pub phi: Id,
    pub syntropy: i64,
    pub delta_syntropy: i64,
    pub focus_bps: u16,
    pub budget: u64,
    pub paid: u64,
    pub pool_after: u64,
    pub rewarded: u32,
    pub payouts: Id,
    /// `N`, the players of the epoch's game (rewards.md §7)
    pub players: u32,
    /// Σ k_c, the sampled marginals of the window
    pub samples: u32,
    /// players paid by their mean
    pub by_mean: u32,
    /// `v(N)` at 2^60, saturated to i64
    pub value: i64,
}

/// Record bytes (§5.6 amended: the lottery's four fields after `payouts`).
pub const RECORD_BYTES: usize = 218;

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(RECORD_BYTES);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.boundary.to_le_bytes());
        out.extend_from_slice(&self.beacon);
        out.extend_from_slice(&self.boundary_root);
        out.extend_from_slice(&self.particles.to_le_bytes());
        out.extend_from_slice(&self.axons.to_le_bytes());
        out.extend_from_slice(&self.phi);
        out.extend_from_slice(&self.syntropy.to_le_bytes());
        out.extend_from_slice(&self.delta_syntropy.to_le_bytes());
        out.extend_from_slice(&self.focus_bps.to_le_bytes());
        out.extend_from_slice(&self.budget.to_le_bytes());
        out.extend_from_slice(&self.paid.to_le_bytes());
        out.extend_from_slice(&self.pool_after.to_le_bytes());
        out.extend_from_slice(&self.rewarded.to_le_bytes());
        out.extend_from_slice(&self.payouts);
        out.extend_from_slice(&self.players.to_le_bytes());
        out.extend_from_slice(&self.samples.to_le_bytes());
        out.extend_from_slice(&self.by_mean.to_le_bytes());
        out.extend_from_slice(&self.value.to_le_bytes());
        debug_assert_eq!(out.len(), RECORD_BYTES);
        out
    }

    /// `time[settle(E)] = hemera(D_EPOCH ‖ record)`.
    pub fn digest(&self) -> Id {
        let mut buf = DOMAIN_EPOCH.to_vec();
        buf.extend_from_slice(&self.encode());
        *hemera::hash(&buf).as_bytes()
    }
}

/// i128 syntropy into the record's i64 (saturating: J ≤ ln n nats, which
/// at 2^60 exceeds i64 only past e^8 ≈ 3,000 particles of full concentration).
pub fn sat64(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}
