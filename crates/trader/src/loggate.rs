//! Exactly-once, in-order delivery of chain logs to the engine.
//!
//! RPC nodes behind one URL are not always in sync, and some of them answer outside the requested range. Measured on
//! this chain's public RPC: with `fromBlock` above the serving node's head it returns that node's head-block logs (below
//! `fromBlock`), and with `toBlock` above its head it silently returns fewer blocks. Polling `fromBlock = last + 1`
//! against such nodes repeated ~2-3% of all events (an LP add counted twice made a drained pool look funded) and can
//! skip events. The gate fixes both:
//! - every poll re-reads the last `overlap` blocks, so blocks a lagging node did not have yet are picked up next poll;
//! - logs outside the requested range, removed logs and logs already delivered are dropped;
//! - a log that arrives after its block was passed is delivered at the next block (`Late`), never out of order.

use std::collections::HashSet;

/// Blocks re-read on every poll (0.1 s blocks: 2.5 s of tolerance for a lagging node).
pub const OVERLAP_BLOCKS: u64 = 25;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Admit {
    /// first delivery, block not passed yet
    New,
    /// first delivery, but its block was already passed: deliver it at the next block
    Late,
    Drop,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateStats {
    /// already delivered logs of the re-read window (expected on every poll)
    pub reread: u64,
    /// a log repeated inside the new part of a range (the RPC misbehaved)
    pub duplicate: u64,
    pub outside_range: u64,
    pub removed: u64,
    pub late: u64,
}

pub struct LogGate {
    seen: HashSet<(u64, u64)>,
    overlap: u64,
    primed: bool,
    pub stats: GateStats,
}

impl LogGate {
    pub fn new(overlap: u64) -> LogGate {
        LogGate { seen: HashSet::new(), overlap, primed: false, stats: GateStats::default() }
    }

    /// First block to request when every block `<= last` has been delivered. The first poll after a start does not
    /// re-read (the replayed store already holds those blocks, and their keys are not in memory).
    pub fn from_block(&self, last: u64) -> u64 {
        if self.primed {
            (last + 1).saturating_sub(self.overlap)
        } else {
            last + 1
        }
    }

    /// Classify one log of a response to a request for `[from, to]`, when every block `<= last` has been delivered.
    pub fn classify(&mut self, blk: u64, log_index: u64, removed: bool, from: u64, last: u64, to: u64) -> Admit {
        if removed {
            self.stats.removed += 1;
            return Admit::Drop;
        }
        if blk < from || blk > to {
            self.stats.outside_range += 1;
            return Admit::Drop;
        }
        if !self.seen.insert((blk, log_index)) {
            if blk <= last {
                self.stats.reread += 1;
            } else {
                self.stats.duplicate += 1;
            }
            return Admit::Drop;
        }
        if blk <= last {
            self.stats.late += 1;
            Admit::Late
        } else {
            Admit::New
        }
    }

    /// The poll covering up to `to` was delivered: forget keys the next request can no longer return.
    pub fn delivered(&mut self, to: u64) {
        self.primed = true;
        let keep_from = (to + 1).saturating_sub(self.overlap);
        self.seen.retain(|k| k.0 >= keep_from);
    }
}

/// Position of late logs inside the block they are delivered at: before the block's own logs, in chain order.
/// `n` late logs get log indices `-n, ..., -1`.
pub fn late_log_index(rank: usize, n: usize) -> f64 {
    rank as f64 - n as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_poll_after_start_does_not_reread() {
        let mut g = LogGate::new(25);
        assert_eq!(g.from_block(1000), 1001);
        g.delivered(1010);
        assert_eq!(g.from_block(1010), 986);
        assert_eq!(LogGate::new(25).from_block(3), 4);
        let mut g = LogGate::new(25);
        g.delivered(3);
        assert_eq!(g.from_block(3), 0, "saturates at genesis");
    }

    #[test]
    fn drops_below_range_repeats_and_removed_logs() {
        let mut g = LogGate::new(25);
        // poll 1: blocks 101..=110
        assert_eq!(g.classify(105, 3, false, 101, 100, 110), Admit::New);
        assert_eq!(g.classify(105, 3, false, 101, 100, 110), Admit::Drop, "same log twice in one response");
        assert_eq!(g.classify(111, 0, false, 101, 100, 110), Admit::Drop, "above the requested range");
        assert_eq!(g.classify(99, 7, false, 101, 100, 110), Admit::Drop, "node clamped the range to its own head");
        assert_eq!(g.classify(106, 1, true, 101, 100, 110), Admit::Drop, "removed by a reorg");
        g.delivered(110);
        // poll 2 re-reads from 86: the logs delivered in poll 1 come back and are dropped
        let from = g.from_block(110);
        assert_eq!(from, 86);
        assert_eq!(g.classify(105, 3, false, from, 110, 120), Admit::Drop);
        assert_eq!(g.classify(115, 2, false, from, 110, 120), Admit::New);
        assert_eq!(g.stats, GateStats { reread: 1, duplicate: 1, outside_range: 2, removed: 1, late: 0 });
    }

    #[test]
    fn log_missed_by_a_lagging_node_is_recovered_late() {
        let mut g = LogGate::new(25);
        // poll 1 asked for 101..=110 but the node only had up to 104: the log at 108 did not come back
        assert_eq!(g.classify(103, 0, false, 101, 100, 110), Admit::New);
        g.delivered(110);
        // poll 2 (another node) re-reads 86..=115 and returns it
        let from = g.from_block(110);
        assert_eq!(g.classify(103, 0, false, from, 110, 115), Admit::Drop);
        assert_eq!(g.classify(108, 4, false, from, 110, 115), Admit::Late);
        assert_eq!(g.classify(112, 1, false, from, 110, 115), Admit::New);
        assert_eq!(g.stats.late, 1);
    }

    #[test]
    fn forgets_only_keys_outside_the_next_window() {
        let mut g = LogGate::new(25);
        g.classify(70, 0, false, 60, 59, 100);
        g.classify(76, 0, false, 60, 59, 100);
        g.delivered(100);
        // the next request starts at 76: keys below it are forgotten, keys from it on are kept
        assert!(!g.seen.contains(&(70, 0)));
        assert!(g.seen.contains(&(76, 0)));
    }

    #[test]
    fn late_logs_sort_before_the_block_and_keep_their_order() {
        let ix: Vec<f64> = (0..3).map(|r| late_log_index(r, 3)).collect();
        assert_eq!(ix, vec![-3.0, -2.0, -1.0]);
    }
}
