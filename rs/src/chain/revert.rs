//! Revert by release (chain.md §5.6): the effective revert table (the
//! compiled [`REVERTS`] plus the rehearsal entries of `--revert`), its
//! revert statements of the owner key (`revert_statement.rs`), its
//! consistency checks, the rule sets and difficulty anchors it implies, the
//! `--revert` flag syntax and the small log of the reverts a node applied.
//!
//! Pure data and functions; the engine (`engine.rs`) enforces the table in
//! validation and rolls the stored chain back at open.

use std::io::Write;
use std::path::Path;

use super::daa::ReAnchor;
use super::header::BlockId;
use super::net::codec::{hex, unhex32};
use super::params::{ACTIVATIONS, REVERTS, Revert};
use super::store::Store;

/// One revert, runtime form. An entry of `counter > 0` stands for a revert
/// statement of the owner key (§5.6): `vetoed` is empty, the time floor and
/// the descendants of the invalid blocks do the vetoing. `rule_set == 0` is only valid before
/// [`RevertTable::new`], which resolves it to "the last known rule set + 1"
/// (a rehearsal entry given without a fifth `--revert` field).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertEntry {
    pub height: u64,
    pub keep: BlockId,
    pub vetoed: Vec<BlockId>,
    pub anchor_time: u64,
    pub rule_set: u16,
    /// from `--revert`, not from the compiled table
    pub rehearsal: bool,
    /// the counter of the revert statement this entry stands for; 0 for a
    /// compiled or rehearsal entry
    pub counter: u64,
}

impl From<&Revert> for RevertEntry {
    fn from(r: &Revert) -> RevertEntry {
        RevertEntry { height: r.height, keep: r.keep, vetoed: r.vetoed.to_vec(), anchor_time: r.anchor_time, rule_set: r.rule_set, rehearsal: false, counter: 0 }
    }
}

impl RevertEntry {
    /// `height:keep:vetoed[+vetoed…]:anchor_time:rule_set`, the `/status` form.
    pub fn describe(&self) -> String {
        let vetoed: Vec<String> = self.vetoed.iter().map(|v| hex(v)).collect();
        format!(
            "{}:{}:{}:{}:{}{}{}",
            self.height,
            hex(&self.keep),
            vetoed.join("+"),
            self.anchor_time,
            self.rule_set,
            if self.rehearsal { ":rehearsal" } else { "" },
            if self.counter > 0 { format!(":statement{}", self.counter) } else { String::new() }
        )
    }
}

/// The effective table: compiled entries first, then rehearsal entries,
/// strictly ascending by height (checked by [`RevertTable::check_static`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevertTable {
    entries: Vec<RevertEntry>,
}

fn compiled_last_rule_set() -> u16 {
    ACTIVATIONS.iter().map(|(r, _)| *r).max().unwrap_or(0)
}

/// The highest rule set the compiled tables name, the activations and the
/// compiled reverts: a statement of counter `n` carries this plus `n` (§5.6).
pub fn compiled_max_rule_set() -> u16 {
    REVERTS.iter().map(|r| r.rule_set).fold(compiled_last_rule_set(), u16::max)
}

impl RevertTable {
    /// The compiled table plus `rehearsal` entries (sorted among
    /// themselves, marked `rehearsal`, rule set 0 resolved to the last
    /// known + 1).
    pub fn new(compiled: &[Revert], rehearsal: Vec<RevertEntry>) -> RevertTable {
        let mut entries: Vec<RevertEntry> = compiled.iter().map(RevertEntry::from).collect();
        let mut extra = rehearsal;
        extra.sort_by_key(|e| e.height);
        for mut e in extra {
            e.rehearsal = true;
            entries.push(e);
        }
        let mut last = compiled_last_rule_set();
        for e in &mut entries {
            if e.rule_set == 0 {
                e.rule_set = last.saturating_add(1);
            }
            last = last.max(e.rule_set);
        }
        RevertTable { entries }
    }

    /// This table with the entries of the held revert statements appended
    /// (`RevertStatementSet::entries`): ascending by height and above the
    /// compiled ones, which [`RevertTable::check_static`] checks.
    pub fn with_statements(mut self, statements: Vec<RevertEntry>) -> RevertTable {
        self.entries.extend(statements);
        self
    }

    /// The table this release compiles in, with `rehearsal` added.
    pub fn effective(rehearsal: Vec<RevertEntry>) -> RevertTable {
        RevertTable::new(REVERTS, rehearsal)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[RevertEntry] {
        &self.entries
    }

    pub fn any_rehearsal(&self) -> bool {
        self.entries.iter().any(|e| e.rehearsal)
    }

    pub fn any_statement(&self) -> bool {
        self.entries.iter().any(|e| e.counter > 0)
    }

    /// Every vetoed id of every entry.
    pub fn vetoed_ids(&self) -> impl Iterator<Item = &BlockId> {
        self.entries.iter().flat_map(|e| e.vetoed.iter())
    }

    /// The consistency that needs no store (§5.6): heights from 1, strictly
    /// ascending; `vetoed` non-empty and not containing `keep`; rule sets
    /// strictly above every earlier one (the activation table's included);
    /// no `keep` among an earlier entry's `vetoed`.
    pub fn check_static(&self) -> Result<(), String> {
        let mut last_rule_set = compiled_last_rule_set();
        for (i, e) in self.entries.iter().enumerate() {
            let at = format!("entry {i} (height {})", e.height);
            if e.height == 0 {
                return Err(format!("{at}: a revert returns to a block above genesis"));
            }
            if i > 0 && e.height <= self.entries[i - 1].height {
                return Err(format!("{at}: entries must be sorted by strictly ascending height"));
            }
            if e.vetoed.is_empty() && e.counter == 0 {
                return Err(format!("{at}: vetoed is empty"));
            }
            if e.vetoed.contains(&e.keep) {
                return Err(format!("{at}: keep is among the vetoed ids"));
            }
            if e.rule_set <= last_rule_set {
                return Err(format!("{at}: rule set {} is not above the earlier rule sets (last {last_rule_set})", e.rule_set));
            }
            last_rule_set = e.rule_set;
            for p in &self.entries[..i] {
                if p.vetoed.contains(&e.keep) {
                    return Err(format!("{at}: keep is vetoed by the entry at height {}", p.height));
                }
            }
        }
        Ok(())
    }

    /// The consistency that needs the store (§5.6): a stored `keep` is at
    /// its height and descends from every earlier `keep` and from no earlier
    /// `vetoed`; a stored `vetoed` id lies above its entry's height. A
    /// `keep` the store has not seen yet cannot be checked and is not an
    /// error (the node may be syncing).
    pub fn check_store(&self, store: &Store) -> Result<(), String> {
        for (i, e) in self.entries.iter().enumerate() {
            let at = format!("entry {i} (height {})", e.height);
            for v in &e.vetoed {
                if let Some(entry) = store.get(v) {
                    if entry.header.height <= e.height {
                        return Err(format!("{at}: vetoed {} is at height {}, not above the entry", hex(v), entry.header.height));
                    }
                }
            }
            let Some(keep) = store.get(&e.keep) else { continue };
            if keep.header.height != e.height {
                return Err(format!("{at}: keep {} is at height {}", hex(&e.keep), keep.header.height));
            }
            for p in &self.entries[..i] {
                if store.ancestor_at(&e.keep, p.height) != Some(p.keep) {
                    return Err(format!("{at}: keep does not descend from the keep of the entry at height {}", p.height));
                }
                for v in &p.vetoed {
                    if let Some(vh) = store.get(v).map(|x| x.header.height) {
                        if vh <= e.height && store.ancestor_at(&e.keep, vh) == Some(*v) {
                            return Err(format!("{at}: keep descends from {} vetoed at height {}", hex(v), p.height));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The rule set in force at `height` (§7.1): the highest activation
    /// whose height is `≤ height`, the compiled [`ACTIVATIONS`] extended by
    /// `(rule_set, H + 1)` per entry. `None` when none applies.
    pub fn rule_set_at(&self, height: u64) -> Option<u16> {
        let compiled = ACTIVATIONS.iter().filter(|(_, h)| *h <= height).map(|(r, _)| *r);
        let reverts = self.entries.iter().filter(|e| e.height.saturating_add(1) <= height).map(|e| e.rule_set);
        compiled.chain(reverts).max()
    }

    /// The last rule set this node knows; a header above it is an
    /// `upgrade-required` (§7.1).
    pub fn last_rule_set(&self) -> u16 {
        self.entries.iter().map(|e| e.rule_set).fold(compiled_last_rule_set(), u16::max)
    }

    /// The activation table as `/status` prints it: `1@0,2@501`.
    pub fn activations_line(&self) -> String {
        let mut acts: Vec<(u16, u64)> = ACTIVATIONS.to_vec();
        acts.extend(self.entries.iter().map(|e| (e.rule_set, e.height.saturating_add(1))));
        acts.sort_by_key(|(r, h)| (*h, *r));
        acts.iter().map(|(r, h)| format!("{r}@{h}")).collect::<Vec<_>>().join(",")
    }

    /// The `keep` block of the entry at exactly `height`, if any (§5.6 rule 1).
    pub fn keep_at(&self, height: u64) -> Option<BlockId> {
        self.entries.iter().find(|e| e.height == height).map(|e| e.keep)
    }

    /// The earliest allowed `time` at `height`: the `anchor_time` of the
    /// entry whose `H + 1` it is (§5.6 rule 3).
    pub fn time_floor(&self, height: u64) -> Option<u64> {
        self.entries.iter().find(|e| e.height.saturating_add(1) == height).map(|e| e.anchor_time)
    }

    /// The `anchor_time` of the statement entry whose `H + 1` is `height`:
    /// the floor the open-time rollback also applies to stored blocks.
    pub fn statement_floor(&self, height: u64) -> Option<u64> {
        self.entries.iter().find(|e| e.counter > 0 && e.height.saturating_add(1) == height).map(|e| e.anchor_time)
    }

    /// The latest entry at or below `height`, for naming the checkpoint a
    /// vetoed header contradicts.
    pub fn latest_at_or_below(&self, height: u64) -> Option<&RevertEntry> {
        self.entries.iter().rev().find(|e| e.height <= height)
    }

    /// The difficulty re-anchors for [`super::daa::next_bits_reverted`].
    pub fn reanchors(&self) -> Vec<ReAnchor> {
        self.entries.iter().map(|e| ReAnchor { height: e.height, anchor_time: e.anchor_time }).collect()
    }

    /// The `reverts` line of `/status`: `none` or the entries, comma separated.
    pub fn status_line(&self) -> String {
        if self.entries.is_empty() {
            "none".into()
        } else {
            self.entries.iter().map(RevertEntry::describe).collect::<Vec<_>>().join(",")
        }
    }
}

/// A revert applied by this node: what [`super::api::Event::Reverted`]
/// carries, kept for `/status` and persisted in `reverted.log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reverted {
    pub to_height: u64,
    pub keep: BlockId,
    pub vetoed: Vec<BlockId>,
    pub rehearsal: bool,
}

impl Reverted {
    fn to_line(&self) -> String {
        let vetoed: Vec<String> = self.vetoed.iter().map(|v| hex(v)).collect();
        format!("{} {} {} {}", self.to_height, hex(&self.keep), vetoed.join(","), u8::from(self.rehearsal))
    }

    fn from_line(line: &str) -> Option<Reverted> {
        let mut it = line.split(' ');
        let to_height = it.next()?.parse().ok()?;
        let keep = unhex32(it.next()?)?;
        // a statement entry vetoes nothing: the field is empty
        let field = it.next()?;
        let vetoed = if field.is_empty() { Vec::new() } else { field.split(',').map(unhex32).collect::<Option<Vec<_>>>()? };
        let rehearsal = it.next()? == "1";
        Some(Reverted { to_height, keep, vetoed, rehearsal })
    }

    /// The `reverted` line of `/status`.
    pub fn status_line(&self) -> String {
        let vetoed: Vec<String> = self.vetoed.iter().map(|v| hex(v)).collect();
        format!("height={} keep={} vetoed={} rehearsal={}", self.to_height, hex(&self.keep), vetoed.join("+"), self.rehearsal)
    }
}

/// The last record in the log at `path`, if the file exists and has one.
pub fn load_last(path: &Path) -> Option<Reverted> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().rev().find_map(Reverted::from_line)
}

/// Append a record to the log at `path` (durable before it returns).
pub fn append(path: &Path, r: &Reverted) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", r.to_line())?;
    f.sync_all()
}

/// Parse a `--revert <height>:<keep_hex>:<vetoed_hex>[,<vetoed_hex>…]:<anchor_time>[:<rule_set>]`
/// value. Without the fifth field the rule set is the last known + 1.
pub fn parse_flag(s: &str) -> Result<RevertEntry, String> {
    let usage = "--revert wants <height>:<keep_hex>:<vetoed_hex>[,<vetoed_hex>…]:<anchor_time>[:<rule_set>]";
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 4 && parts.len() != 5 {
        return Err(format!("{usage}; got {s:?}"));
    }
    let height: u64 = parts[0].parse().map_err(|e| format!("--revert height {:?}: {e}", parts[0]))?;
    let keep = unhex32(parts[1]).ok_or_else(|| format!("--revert keep {:?} is not 64 hex digits", parts[1]))?;
    let vetoed = parts[2]
        .split(',')
        .map(|v| unhex32(v).ok_or_else(|| format!("--revert vetoed {v:?} is not 64 hex digits")))
        .collect::<Result<Vec<_>, _>>()?;
    let anchor_time: u64 = parts[3].parse().map_err(|e| format!("--revert anchor_time {:?}: {e}", parts[3]))?;
    let rule_set: u16 = match parts.get(4) {
        Some(r) => r.parse().map_err(|e| format!("--revert rule_set {r:?}: {e}"))?,
        None => 0,
    };
    Ok(RevertEntry { height, keep, vetoed, anchor_time, rule_set, rehearsal: true, counter: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(height: u64, keep: u8, vetoed: u8, rule_set: u16) -> RevertEntry {
        RevertEntry { height, keep: [keep; 32], vetoed: vec![[vetoed; 32]], anchor_time: 1_000, rule_set, rehearsal: true, counter: 0 }
    }

    #[test]
    fn the_compiled_table_ships_empty_and_changes_nothing() {
        let t = RevertTable::effective(Vec::new());
        assert!(t.is_empty());
        assert_eq!(t.check_static(), Ok(()));
        assert_eq!(t.rule_set_at(0), Some(1));
        assert_eq!(t.rule_set_at(u64::MAX), Some(1));
        assert_eq!(t.last_rule_set(), 1);
        assert_eq!(t.activations_line(), "1@0");
        assert_eq!(t.status_line(), "none");
        assert!(t.reanchors().is_empty());
    }

    #[test]
    fn an_entry_starts_the_next_rule_set_at_h_plus_1() {
        let t = RevertTable::effective(vec![entry(10, 1, 2, 0)]);
        assert_eq!(t.entries()[0].rule_set, 2, "0 resolves to last known + 1");
        assert_eq!((t.rule_set_at(10), t.rule_set_at(11), t.rule_set_at(500)), (Some(1), Some(2), Some(2)));
        assert_eq!(t.last_rule_set(), 2);
        assert_eq!(t.activations_line(), "1@0,2@11");
        assert_eq!(t.keep_at(10), Some([1; 32]));
        assert_eq!(t.keep_at(11), None);
        assert_eq!(t.time_floor(11), Some(1_000));
        assert_eq!(t.time_floor(12), None);
        assert!(t.any_rehearsal());
    }

    #[test]
    fn the_static_check_names_what_is_wrong() {
        let ok = RevertTable::effective(vec![entry(10, 1, 2, 0), entry(20, 3, 4, 0)]);
        assert_eq!(ok.check_static(), Ok(()));
        assert_eq!(ok.entries()[1].rule_set, 3);
        let unsorted = RevertTable { entries: vec![entry(20, 3, 4, 2), entry(10, 1, 2, 3)] };
        assert!(unsorted.check_static().unwrap_err().contains("ascending"));
        let mut empty_vetoed = entry(10, 1, 2, 2);
        empty_vetoed.vetoed.clear();
        assert!(RevertTable { entries: vec![empty_vetoed] }.check_static().unwrap_err().contains("vetoed is empty"));
        assert!(RevertTable { entries: vec![entry(10, 1, 1, 2)] }.check_static().unwrap_err().contains("among the vetoed"));
        assert!(RevertTable { entries: vec![entry(10, 1, 2, 1)] }.check_static().unwrap_err().contains("rule set"));
        assert!(RevertTable { entries: vec![entry(10, 1, 2, 2), entry(20, 2, 4, 3)] }.check_static().unwrap_err().contains("keep is vetoed"));
        assert!(RevertTable { entries: vec![entry(0, 1, 2, 2)] }.check_static().unwrap_err().contains("above genesis"));
    }

    #[test]
    fn the_flag_parses_four_or_five_fields() {
        let k = hex(&[1; 32]);
        let v1 = hex(&[2; 32]);
        let v2 = hex(&[3; 32]);
        let e = parse_flag(&format!("10:{k}:{v1},{v2}:1700000000")).unwrap();
        assert_eq!((e.height, e.keep, e.vetoed.len(), e.anchor_time, e.rule_set, e.rehearsal), (10, [1; 32], 2, 1_700_000_000, 0, true));
        assert_eq!(parse_flag(&format!("10:{k}:{v1}:5:7")).unwrap().rule_set, 7);
        assert!(parse_flag("10:zz:00:5").is_err());
        assert!(parse_flag(&format!("10:{k}:{v1}")).is_err());
        assert!(parse_flag(&format!("x:{k}:{v1}:5")).is_err());
    }

    #[test]
    fn a_statement_entry_may_veto_nothing_and_a_rehearsal_may_not() {
        let mut st = entry(10, 1, 2, 2);
        st.vetoed.clear();
        st.rehearsal = false;
        st.counter = 1;
        let t = RevertTable { entries: vec![st.clone()] };
        assert_eq!(t.check_static(), Ok(()));
        assert!(t.any_statement() && !t.any_rehearsal());
        assert_eq!(t.statement_floor(11), Some(1_000));
        assert_eq!(t.statement_floor(12), None);
        assert!(st.describe().ends_with(":statement1"));
        let compiled_like = RevertEntry { counter: 0, ..st };
        assert!(RevertTable { entries: vec![compiled_like] }.check_static().unwrap_err().contains("vetoed is empty"));
    }

    #[test]
    fn the_log_line_round_trips() {
        let r = Reverted { to_height: 9, keep: [1; 32], vetoed: vec![[2; 32], [3; 32]], rehearsal: true };
        assert_eq!(Reverted::from_line(&r.to_line()), Some(r));
        assert_eq!(Reverted::from_line("garbage"), None);
        let none = Reverted { to_height: 9, keep: [1; 32], vetoed: Vec::new(), rehearsal: false };
        assert_eq!(Reverted::from_line(&none.to_line()), Some(none));
    }
}
