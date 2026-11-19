//! The game on the wire and the ticket as a cache record: the two byte
//! forms of the settlement ticket that no solve reads. [`GameWire`] is what
//! `GET /game/{beacon}` serves a miner without the node's state
//! (chain-wire §7 amended); [`encode_ticket`] is the record of a verified
//! ticket under `tickets/`, checksummed so a flipped bit never pays the
//! wrong player.

use tru::focusing::epoch::coalition::{Probe, Ticket};

use super::super::header::hash32;
use super::super::state::Id;
use super::{EpochGame, Player};

/// The game as a miner receives it (chain-wire §7 amended): the base axons
/// and the players, enough to build the same [`EpochGame`] — the same
/// cluster, the same solves — on a machine without the node's state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameWire {
    pub epoch: u64,
    pub beacon: String,
    pub cluster: String,
    pub w_ticket: usize,
    /// `[from, to, weight]`, weight as a decimal string
    pub base: Vec<(String, String, String)>,
    /// `[card, side, delta, p, q, owner]`, delta as a decimal string
    pub players: Vec<(String, i8, String, String, String, String)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex32(s: &str) -> Result<Id, String> {
    if s.len() != 64 {
        return Err(format!("{s}: not 64 hex digits"));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

impl GameWire {
    pub fn of(g: &EpochGame) -> GameWire {
        GameWire {
            epoch: g.epoch,
            beacon: hex(&g.beacon),
            cluster: hex(&g.cluster),
            w_ticket: g.width,
            base: g.base.iter().map(|(p, q, w)| (hex(p), hex(q), w.to_string())).collect(),
            players: g.players.iter().map(|p| (hex(&p.card), p.side, p.delta.to_string(), hex(&p.p), hex(&p.q), hex(&p.owner))).collect(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a game serializes")
    }

    /// The game rebuilt (one solve); its cluster must equal the one served.
    pub fn build(&self, workers: usize) -> Result<EpochGame, String> {
        let base = self.base.iter().map(|(p, q, w)| Ok((unhex32(p)?, unhex32(q)?, w.parse::<u128>().map_err(|e| e.to_string())?))).collect::<Result<Vec<_>, String>>()?;
        let players = self
            .players
            .iter()
            .map(|(card, side, delta, p, q, owner)| {
                Ok(Player { card: unhex32(card)?, side: *side, delta: delta.parse().map_err(|e: std::num::ParseIntError| e.to_string())?, p: unhex32(p)?, q: unhex32(q)?, owner: unhex32(owner)? })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let g = EpochGame::build(self.epoch, unhex32(&self.beacon)?, base, players, self.w_ticket, workers)?;
        if hex(&g.cluster) != self.cluster {
            return Err("the game's cluster differs from the served one".into());
        }
        Ok(g)
    }
}

// ── the ticket as a cache record ───────────────────────────────────────

const MAGIC: &[u8; 8] = b"S3TCKT02";

/// A verified ticket's bytes: everything the commitment and the settlement
/// read (the ordering is implied by the nonce and not stored), with a hemera
/// checksum trailer. The header's `sample` commits only to the marginal and
/// probe values (`Ticket::preimage`), so a cached record's `covered`, `rank`
/// and probe `axon/t/u` are bound by this checksum instead — a flipped bit
/// in any of them is rejected on load rather than paying the wrong player at
/// settlement (R18-7).
pub fn encode_ticket(t: &Ticket) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&t.nonce.to_le_bytes());
    out.extend_from_slice(&(t.rank as u64).to_le_bytes());
    out.extend_from_slice(&(t.covered.len() as u32).to_le_bytes());
    for (c, m) in t.covered.iter().zip(&t.marginals) {
        out.extend_from_slice(&(*c as u32).to_le_bytes());
        out.extend_from_slice(&m.to_le_bytes());
    }
    out.extend_from_slice(&(t.probes.len() as u32).to_le_bytes());
    for p in &t.probes {
        out.extend_from_slice(&(p.axon as u32).to_le_bytes());
        out.extend_from_slice(&p.t.to_le_bytes());
        out.extend_from_slice(&p.u.to_le_bytes());
        out.extend_from_slice(&p.value.to_le_bytes());
    }
    let sum = hash32(&out);
    out.extend_from_slice(&sum);
    out
}

pub fn decode_ticket(b: &[u8]) -> Option<Ticket> {
    // the checksum binds every field (R18-7): split it off and compare
    let body = b.get(..b.len().checked_sub(32)?)?;
    if hash32(body)[..] != b[body.len()..] {
        return None;
    }
    let b = body;
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(at..at + n)?;
        at += n;
        Some(s)
    };
    if take(8)? != MAGIC {
        return None;
    }
    let u64_ = |s: &[u8]| u64::from_le_bytes(s.try_into().expect("8"));
    let u32_ = |s: &[u8]| u32::from_le_bytes(s.try_into().expect("4"));
    let i128_ = |s: &[u8]| i128::from_le_bytes(s.try_into().expect("16"));
    let nonce = u64_(take(8)?);
    let rank = u64_(take(8)?) as usize;
    let n = u32_(take(4)?) as usize;
    if n > 1 << 20 {
        return None;
    }
    let mut covered = Vec::with_capacity(n);
    let mut marginals = Vec::with_capacity(n);
    for _ in 0..n {
        covered.push(u32_(take(4)?) as usize);
        marginals.push(i128_(take(16)?));
    }
    let k = u32_(take(4)?) as usize;
    if k > 1 << 20 {
        return None;
    }
    let mut probes = Vec::with_capacity(k);
    for _ in 0..k {
        let axon = u32_(take(4)?) as usize;
        let t = u32_(take(4)?);
        let u = u128::from_le_bytes(take(16)?.try_into().ok()?);
        let value = i128_(take(16)?);
        probes.push(Probe { axon, t, u, value });
    }
    if at != b.len() {
        return None;
    }
    Some(Ticket { nonce, rank, order: Vec::new(), covered, marginals, probes })
}
