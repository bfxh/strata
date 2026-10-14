//! 节点 DAG 的纯数据 IR + tile 级 dirty 传播（钉死 01-P3：增量重算）。
//!
//! 图本身是可序列化、可 diff、可撤销的纯数据（见 03 文档"撤销 = 状态机回退"）。
//! 本 crate 只负责"谁脏了"，不负责"怎么算"——求值在 `strata-nodes`/GPU 侧。
#![allow(clippy::must_use_candidate)] // core crate 开 pedantic，但豁免此高噪声风格项

use std::collections::{HashMap, HashSet, VecDeque};

use strata_geo::TilePos;

pub mod history;

/// 节点的唯一身份（结构变更时稳定，不随插入顺序漂移）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub u32);

/// 一条边的空间依赖半径（单位：源 tile 数）。erosion 迭代 N 步 ⇒ 半径 N。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepRadius {
    /// 逐点对应，halo 内即可
    Local,
    /// 需要邻域 pad 这么多 tile
    Stencil(u8),
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub radius: DepRadius,
}

/// 整个图。邻接表按"下游"存，便于 dirty 前向传播。
#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: Vec<NodeId>,
    /// from -> 出边
    out: HashMap<NodeId, Vec<Edge>>,
}

impl Graph {
    pub fn add_node(&mut self, id: NodeId) {
        if !self.nodes.contains(&id) {
            self.nodes.push(id);
        }
    }

    /// 图中节点数。
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 加边；拒绝自环与未知节点（结构性脏由调用方在此成功后自行触发下游传播）。
    ///
    /// # Errors
    /// 自环返回 [`GraphError::SelfLoop`]，端点不在图中返回 [`GraphError::UnknownNode`]。
    pub fn add_edge(&mut self, e: Edge) -> Result<(), GraphError> {
        if e.from == e.to {
            return Err(GraphError::SelfLoop);
        }
        if !self.nodes.contains(&e.from) || !self.nodes.contains(&e.to) {
            return Err(GraphError::UnknownNode);
        }
        self.out.entry(e.from).or_default().push(e);
        Ok(())
    }

    pub fn is_dag(&self) -> bool {
        self.cycle().is_none()
    }

    /// 返回任一环上的节点序列（用于报错），无环则 None。
    pub fn cycle(&self) -> Option<Vec<NodeId>> {
        let mut color: HashMap<NodeId, u8> = HashMap::new(); // 0 white,1 gray,2 black
        let mut stack: Vec<NodeId> = Vec::new();
        for &root in &self.nodes {
            if color.get(&root) == Some(&0) || !color.contains_key(&root) {
                if let Some(c) = self.dfs_cycle(root, &mut color, &mut stack) {
                    return Some(c);
                }
            }
        }
        None
    }

    fn dfs_cycle(
        &self,
        u: NodeId,
        color: &mut HashMap<NodeId, u8>,
        stack: &mut Vec<NodeId>,
    ) -> Option<Vec<NodeId>> {
        color.insert(u, 1);
        stack.push(u);
        for e in self.out.get(&u).into_iter().flatten() {
            match color.get(&e.to).copied().unwrap_or(0) {
                1 => {
                    if let Some(pos) = stack.iter().position(|&n| n == e.to) {
                        let mut cyc = stack[pos..].to_vec();
                        cyc.push(e.to);
                        return Some(cyc);
                    }
                }
                0 => {
                    if let Some(c) = self.dfs_cycle(e.to, color, stack) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        stack.pop();
        color.insert(u, 2);
        None
    }

    /// 某节点参数变更 ⇒ 返回其全部下游传递闭包（不含自身）。
    pub fn downstream(&self, from: NodeId) -> Vec<NodeId> {
        let mut seen = HashSet::new();
        let mut q = VecDeque::new();
        for e in self.out.get(&from).into_iter().flatten() {
            if seen.insert(e.to) {
                q.push_back(e.to);
            }
        }
        let mut order = Vec::new();
        while let Some(n) = q.pop_front() {
            order.push(n);
            for e in self.out.get(&n).into_iter().flatten() {
                if seen.insert(e.to) {
                    q.push_back(e.to);
                }
            }
        }
        order
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    SelfLoop,
    UnknownNode,
}

/// dirty 集：节点 → 受影响 tile 集合。空集合语义 = 该节点全 tile dirty。
#[derive(Clone, Debug, Default)]
pub struct DirtySet {
    per_node: HashMap<NodeId, Option<HashSet<TilePos>>>,
}

impl DirtySet {
    /// 参数变更：源节点 tile 全 dirty。
    pub fn mark_all(&mut self, id: NodeId) {
        self.per_node.insert(id, None);
    }

    pub fn mark_tiles(&mut self, id: NodeId, tiles: HashSet<TilePos>) {
        match self.per_node.get_mut(&id) {
            Some(None) => {} // 已是全 dirty，扩大无意义
            Some(Some(set)) => {
                set.extend(tiles);
            }
            None => {
                self.per_node.insert(id, Some(tiles));
            }
        }
    }

    /// 把源节点的 dirty 沿边传播到下游，按 [`DepRadius`] 膨胀 tile 集合。
    pub fn propagate(&mut self, graph: &Graph, source: NodeId) {
        let mut frontier = vec![source];
        let mut visited = HashSet::new();
        while let Some(n) = frontier.pop() {
            if !visited.insert(n) {
                continue;
            }
            let src_full = matches!(self.per_node.get(&n), Some(None));
            let src_tiles: Option<HashSet<TilePos>> = self.per_node.get(&n).cloned().flatten();
            for e in graph.out.get(&n).into_iter().flatten() {
                if src_full {
                    self.mark_all(e.to);
                } else if let Some(tiles) = &src_tiles {
                    self.mark_tiles(e.to, expand(tiles, e.radius));
                }
                frontier.push(e.to);
            }
        }
    }

    pub fn is_dirty(&self, id: NodeId) -> bool {
        self.per_node.contains_key(&id)
    }

    pub fn dirty_tiles(&self, id: NodeId) -> DirtyTiles {
        match self.per_node.get(&id) {
            None => DirtyTiles::Clean,
            Some(None) => DirtyTiles::All,
            Some(Some(s)) => DirtyTiles::Set(s.clone()),
        }
    }

    pub fn len(&self) -> usize {
        self.per_node.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_node.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirtyTiles {
    Clean,
    All,
    Set(HashSet<TilePos>),
}

/// 按依赖半径膨胀 tile 集合（Stencil(r) ⇒ 周边 r 圈）。
fn expand(tiles: &HashSet<TilePos>, radius: DepRadius) -> HashSet<TilePos> {
    let r = match radius {
        DepRadius::Local => 0,
        DepRadius::Stencil(r) => i64::from(r),
    };
    let mut out = HashSet::new();
    for t in tiles {
        for dy in -r..=r {
            for dx in -r..=r {
                out.insert(TilePos {
                    x: t.x + dx,
                    y: t.y + dy,
                    level: t.level,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(i: u32) -> NodeId {
        NodeId(i)
    }
    fn tp(x: i64) -> TilePos {
        TilePos { x, y: 0, level: 0 }
    }

    #[test]
    fn linear_chain_downstream_is_transitive() {
        let mut g = Graph::default();
        for i in 0..4 {
            g.add_node(n(i));
        }
        g.add_edge(Edge {
            from: n(0),
            to: n(1),
            radius: DepRadius::Local,
        })
        .unwrap();
        g.add_edge(Edge {
            from: n(1),
            to: n(2),
            radius: DepRadius::Local,
        })
        .unwrap();
        g.add_edge(Edge {
            from: n(2),
            to: n(3),
            radius: DepRadius::Local,
        })
        .unwrap();
        let mut d: Vec<_> = g.downstream(n(0));
        d.sort_by_key(|x| x.0);
        assert_eq!(d, vec![n(1), n(2), n(3)]);
        assert_eq!(g.downstream(n(3)), vec![]);
    }

    #[test]
    fn diamond_propagates_once_per_node() {
        let mut g = Graph::default();
        for i in 0..4 {
            g.add_node(n(i));
        }
        let e = |from, to| Edge {
            from: n(from),
            to: n(to),
            radius: DepRadius::Stencil(1),
        };
        g.add_edge(e(0, 1)).unwrap();
        g.add_edge(e(0, 2)).unwrap();
        g.add_edge(e(1, 3)).unwrap();
        g.add_edge(e(2, 3)).unwrap();
        assert!(g.is_dag());
        let mut dirty = DirtySet::default();
        dirty.mark_tiles(n(0), [tp(0)].into_iter().collect());
        dirty.propagate(&g, n(0));
        // 下游 3 号被 stencil(1) 两次膨胀：0 的邻居集合应含 3 号
        assert!(dirty.is_dirty(n(3)));
        match dirty.dirty_tiles(n(1)) {
            DirtyTiles::Set(s) => {
                assert!(s.contains(&tp(-1)) && s.contains(&tp(1)) && s.contains(&tp(0)));
            }
            other => panic!("expected set, got {other:?}"),
        }
    }

    #[test]
    fn rejects_self_loop_and_unknown() {
        let mut g = Graph::default();
        g.add_node(n(0));
        assert_eq!(
            g.add_edge(Edge {
                from: n(0),
                to: n(0),
                radius: DepRadius::Local
            }),
            Err(GraphError::SelfLoop)
        );
        assert_eq!(
            g.add_edge(Edge {
                from: n(0),
                to: n(9),
                radius: DepRadius::Local
            }),
            Err(GraphError::UnknownNode)
        );
    }

    #[test]
    fn cycle_detected() {
        let mut g = Graph::default();
        for i in 0..3 {
            g.add_node(n(i));
        }
        let e = |from, to| Edge {
            from: n(from),
            to: n(to),
            radius: DepRadius::Local,
        };
        g.add_edge(e(0, 1)).unwrap();
        g.add_edge(e(1, 2)).unwrap();
        g.add_edge(e(2, 0)).unwrap();
        assert!(!g.is_dag());
    }

    #[test]
    fn mark_all_supersedes_tiles() {
        let mut d = DirtySet::default();
        d.mark_tiles(n(0), [tp(0)].into_iter().collect());
        d.mark_all(n(0));
        assert_eq!(d.dirty_tiles(n(0)), DirtyTiles::All);
    }
}
