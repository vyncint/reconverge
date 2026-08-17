//! CFG algorithms: reachability, dominators, post-dominators, reducibility,
//! and divergence regions.
//!
//! Dominators use the iterative Cooper–Harper–Kennedy algorithm over a
//! reverse-postorder numbering. Post-dominators run the same algorithm on
//! the reversed CFG with a virtual exit; blocks that cannot reach any exit
//! (infinite loops) simply have no post-dominator, which downstream code
//! treats conservatively.

use crate::model::{BlockId, FnModel};

pub struct Cfg {
    pub succs: Vec<Vec<BlockId>>,
    pub preds: Vec<Vec<BlockId>>,
}

impl Cfg {
    #[must_use]
    pub fn build(f: &FnModel) -> Cfg {
        let n = f.blocks.len();
        let mut succs = vec![Vec::new(); n];
        let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
        for (b, block_succs) in succs.iter_mut().enumerate() {
            for s in f.successors(b) {
                block_succs.push(s);
                preds[s].push(b);
            }
        }
        Cfg { succs, preds }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.succs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.succs.is_empty()
    }
}

/// Blocks reachable from `entry` along normal edges.
#[must_use]
pub fn reachable(cfg: &Cfg, entry: BlockId) -> Vec<bool> {
    let mut seen = vec![false; cfg.len()];
    let mut stack = vec![entry];
    seen[entry] = true;
    while let Some(b) = stack.pop() {
        for &s in &cfg.succs[b] {
            if !seen[s] {
                seen[s] = true;
                stack.push(s);
            }
        }
    }
    seen
}

/// Reverse postorder over the reachable subgraph.
#[must_use]
pub fn reverse_postorder(cfg: &Cfg, entry: BlockId) -> Vec<BlockId> {
    let mut visited = vec![false; cfg.len()];
    let mut order = Vec::new();
    // Iterative DFS with an explicit "children pending" phase.
    let mut stack: Vec<(BlockId, usize)> = vec![(entry, 0)];
    visited[entry] = true;
    while let Some(&mut (b, ref mut i)) = stack.last_mut() {
        if *i < cfg.succs[b].len() {
            let s = cfg.succs[b][*i];
            *i += 1;
            if !visited[s] {
                visited[s] = true;
                stack.push((s, 0));
            }
        } else {
            order.push(b);
            stack.pop();
        }
    }
    order.reverse();
    order
}

/// Immediate dominators over the reachable subgraph (entry's idom is
/// itself). Unreachable blocks get `None`.
#[must_use]
pub fn dominators(cfg: &Cfg, entry: BlockId) -> Vec<Option<BlockId>> {
    let rpo = reverse_postorder(cfg, entry);
    let mut rpo_index = vec![usize::MAX; cfg.len()];
    for (i, &b) in rpo.iter().enumerate() {
        rpo_index[b] = i;
    }

    let mut idom: Vec<Option<BlockId>> = vec![None; cfg.len()];
    idom[entry] = Some(entry);

    let intersect = |idom: &[Option<BlockId>], mut a: BlockId, mut b: BlockId| -> BlockId {
        while a != b {
            while rpo_index[a] > rpo_index[b] {
                a = idom[a].expect("processed block must have an idom");
            }
            while rpo_index[b] > rpo_index[a] {
                b = idom[b].expect("processed block must have an idom");
            }
        }
        a
    };

    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new_idom: Option<BlockId> = None;
            for &p in &cfg.preds[b] {
                if idom[p].is_none() {
                    continue; // unreachable or not yet processed
                }
                new_idom = Some(match new_idom {
                    None => p,
                    Some(current) => intersect(&idom, p, current),
                });
            }
            if new_idom.is_some() && idom[b] != new_idom {
                idom[b] = new_idom;
                changed = true;
            }
        }
    }
    idom
}

/// Whether `a` dominates `b` (reflexive) under the given idom tree.
#[must_use]
pub fn dominates(idom: &[Option<BlockId>], a: BlockId, b: BlockId) -> bool {
    let mut cur = b;
    loop {
        if cur == a {
            return true;
        }
        match idom[cur] {
            Some(parent) if parent != cur => cur = parent,
            _ => return false,
        }
    }
}

/// Reducibility test: every retreating edge `u → v` of a DFS must target a
/// dominator of its source. Any other back edge makes the CFG irreducible
/// (docs/ARCHITECTURE.md: degrade to all-divergent and say so).
#[must_use]
pub fn is_reducible(cfg: &Cfg, entry: BlockId, idom: &[Option<BlockId>]) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        OnStack,
        Done,
    }
    let mut state = vec![State::Unvisited; cfg.len()];
    let mut stack: Vec<(BlockId, usize)> = vec![(entry, 0)];
    state[entry] = State::OnStack;
    while let Some(&mut (b, ref mut i)) = stack.last_mut() {
        if *i < cfg.succs[b].len() {
            let s = cfg.succs[b][*i];
            *i += 1;
            match state[s] {
                State::Unvisited => {
                    state[s] = State::OnStack;
                    stack.push((s, 0));
                }
                State::OnStack => {
                    // Back edge b → s: s must dominate b.
                    if !dominates(idom, s, b) {
                        return false;
                    }
                }
                State::Done => {}
            }
        } else {
            state[b] = State::Done;
            stack.pop();
        }
    }
    true
}

/// Immediate post-dominators, computed on the reversed CFG with a virtual
/// exit node (index `n`) fed **only by the given exit blocks** (normal
/// returns). Aborting dead-ends — `unreachable` arms, panic calls — do not
/// constrain reconvergence: a lane that traps takes the whole kernel down,
/// so control that survives a branch with an aborting arm reconverges at
/// the surviving arm's join. Blocks that cannot reach any exit have
/// `None`.
#[must_use]
pub fn post_dominators(cfg: &Cfg, is_exit: &[bool]) -> Vec<Option<BlockId>> {
    let n = cfg.len();
    let exit = n;
    // Reversed CFG with the virtual exit as entry.
    let mut rsuccs = vec![Vec::new(); n + 1];
    let mut rpreds = vec![Vec::new(); n + 1];
    for (b, block_succs) in cfg.succs.iter().enumerate() {
        for &s in block_succs {
            rsuccs[s].push(b);
            rpreds[b].push(s);
        }
        if is_exit[b] {
            rsuccs[exit].push(b);
            rpreds[b].push(exit);
        }
    }
    let rcfg = Cfg {
        succs: rsuccs,
        preds: rpreds,
    };
    let ipdom_with_exit = dominators(&rcfg, exit);
    (0..n)
        .map(|b| match ipdom_with_exit[b] {
            Some(p) if p != exit => Some(p),
            _ => None,
        })
        .collect()
}

/// The divergence region of a branch: blocks that execute under the
/// branch's control — reachable from a successor without passing through
/// the branch's immediate post-dominator. With no post-dominator (no path
/// to an exit), everything reachable from the successors is in the region.
#[must_use]
pub fn divergence_region(cfg: &Cfg, branch: BlockId, ipdom: Option<BlockId>) -> Vec<bool> {
    let mut region = vec![false; cfg.len()];
    let mut stack: Vec<BlockId> = cfg.succs[branch]
        .iter()
        .copied()
        .filter(|&s| Some(s) != ipdom)
        .collect();
    for &s in &stack {
        region[s] = true;
    }
    while let Some(b) = stack.pop() {
        for &s in &cfg.succs[b] {
            if Some(s) != ipdom && !region[s] {
                region[s] = true;
                stack.push(s);
            }
        }
    }
    region
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exits for tests: every no-successor block returns.
    fn no_succ_exits(g: &Cfg) -> Vec<bool> {
        g.succs.iter().map(Vec::is_empty).collect()
    }

    /// Build a Cfg directly from an edge list (blocks 0..n).
    fn cfg(n: usize, edges: &[(usize, usize)]) -> Cfg {
        let mut succs = vec![Vec::new(); n];
        let mut preds = vec![Vec::new(); n];
        for &(a, b) in edges {
            succs[a].push(b);
            preds[b].push(a);
        }
        Cfg { succs, preds }
    }

    #[test]
    fn diamond_dominators_and_postdominators() {
        // 0 → {1, 2} → 3 → (exit)
        let g = cfg(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let idom = dominators(&g, 0);
        assert_eq!(idom[1], Some(0));
        assert_eq!(idom[2], Some(0));
        assert_eq!(idom[3], Some(0));
        let ipdom = post_dominators(&g, &no_succ_exits(&g));
        assert_eq!(ipdom[0], Some(3));
        assert_eq!(ipdom[1], Some(3));
        assert_eq!(ipdom[2], Some(3));
        assert!(is_reducible(&g, 0, &idom));

        let region = divergence_region(&g, 0, ipdom[0]);
        assert!(region[1] && region[2]);
        assert!(!region[3], "join point is outside the region");
    }

    #[test]
    fn loop_region_and_reducibility() {
        // 0 → 1 → 2 → 1 (back edge), 1 → 3 → exit; branch at 1.
        let g = cfg(4, &[(0, 1), (1, 2), (2, 1), (1, 3)]);
        let idom = dominators(&g, 0);
        assert!(is_reducible(&g, 0, &idom));
        let ipdom = post_dominators(&g, &no_succ_exits(&g));
        assert_eq!(ipdom[1], Some(3));
        let region = divergence_region(&g, 1, ipdom[1]);
        assert!(region[2], "loop body is in the region");
        assert!(region[1], "loop header re-executes under the branch");
        assert!(!region[3], "loop exit reconverges");
    }

    #[test]
    fn irreducible_cfg_is_detected() {
        // Two entries into a cycle: 0 → 1, 0 → 2, 1 → 2, 2 → 1, 1 → 3.
        let g = cfg(4, &[(0, 1), (0, 2), (1, 2), (2, 1), (1, 3)]);
        let idom = dominators(&g, 0);
        assert!(!is_reducible(&g, 0, &idom));
    }

    #[test]
    fn infinite_loop_blocks_have_no_postdominator() {
        // 0 → 1 → 2 → 1; nothing reaches an exit from the loop.
        let g = cfg(3, &[(0, 1), (1, 2), (2, 1)]);
        let ipdom = post_dominators(&g, &no_succ_exits(&g));
        assert_eq!(ipdom[1], None);
        assert_eq!(ipdom[2], None);
        // Conservative region: everything reachable from the successors.
        let region = divergence_region(&g, 1, ipdom[1]);
        assert!(region[2] && region[1]);
    }
}
