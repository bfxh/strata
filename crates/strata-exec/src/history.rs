//! 撤销/重做栈（03 文档「DAG 纯数据 · 状态机回退」的落地）。
//!
//! 设计：快照式历史。`snapshots[cursor]` 恒为当前态；[`History::commit`] 截断
//! cursor 之后的 redo 尾（新分支不复活旧未来）；`limit` 为历史上限（丢最旧保当前，
//! 给撤销栈定内存预算，呼应 D2）。纯数据、与求值解耦——`S` 只需 `Clone`。

/// 快照历史栈。不变量：`snapshots` 非空、`cursor ∈ [0, len)`、
/// `limit == 0 || len <= limit`、`current() == snapshots[cursor]`。
#[derive(Clone, Debug)]
pub struct History<S> {
    snapshots: Vec<S>,
    cursor: usize,
    limit: usize,
}

impl<S: Clone> History<S> {
    /// 以 `initial` 为唯一历史态起步（无上限）。
    pub fn new(initial: S) -> Self {
        Self {
            snapshots: vec![initial],
            cursor: 0,
            limit: 0,
        }
    }

    /// 设置历史上限（≥1；0 视作无界）。超限时 commit 丢最旧快照。
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }

    /// 当前态。
    ///
    /// # Panics
    ///
    /// 永不 panic：`cursor` 由不变量保证始终落在 `snapshots` 内。
    pub fn current(&self) -> &S {
        &self.snapshots[self.cursor]
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.snapshots.len()
    }

    /// 当前历史深度（含未用的 redo 尾）。
    #[must_use]
    pub fn depth(&self) -> usize {
        self.snapshots.len()
    }

    /// 提交新态：截断 redo 尾后追加，cursor 指向最新；按 `limit` 丢最旧。
    pub fn commit(&mut self, s: S) {
        self.snapshots.truncate(self.cursor + 1);
        self.snapshots.push(s);
        self.cursor = self.snapshots.len() - 1;
        if self.limit > 0 && self.snapshots.len() > self.limit {
            let excess = self.snapshots.len() - self.limit;
            self.snapshots.drain(0..excess);
            // drain 只砍头部，当前（最新）仍在末尾。
            self.cursor = self.snapshots.len() - 1;
        }
    }

    /// 回退一步；已在最早态则返回 `None`（no-op）。
    pub fn undo(&mut self) -> Option<&S> {
        if self.can_undo() {
            self.cursor -= 1;
            Some(&self.snapshots[self.cursor])
        } else {
            None
        }
    }

    /// 前进一步；已最新则返回 `None`（no-op）。
    pub fn redo(&mut self) -> Option<&S> {
        if self.can_redo() {
            self.cursor += 1;
            Some(&self.snapshots[self.cursor])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(vals: &[i32]) -> History<i32> {
        let mut h = History::new(vals[0]);
        for &v in &vals[1..] {
            h.commit(v);
        }
        h
    }

    #[test]
    fn current_tracks_cursor_through_undo_redo() {
        let mut h = hist(&[0, 1, 2, 3]);
        assert_eq!(*h.current(), 3);
        assert_eq!(h.undo(), Some(&2));
        assert_eq!(h.undo(), Some(&1));
        assert_eq!(*h.current(), 1);
        assert_eq!(h.redo(), Some(&2));
        assert_eq!(h.redo(), Some(&3));
        assert_eq!(h.redo(), None, "最新之后 redo 应 no-op");
        assert_eq!(*h.current(), 3);
    }

    #[test]
    fn commit_after_undo_truncates_redo_branch() {
        let mut h = hist(&[0, 1, 2]);
        h.undo(); // → 1（未来还留着 2）
        assert!(h.can_redo());
        h.commit(9); // 新分支应抹掉 2
        assert!(!h.can_redo(), "commit-after-undo 后旧未来不得复活");
        assert_eq!(*h.current(), 9);
        assert_eq!(h.depth(), 3, "0,1,9 —— 2 被截断");
    }

    #[test]
    fn boundaries_are_noops() {
        let mut h = History::new(42);
        assert!(!h.can_undo());
        assert!(!h.can_redo());
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), None);
        assert_eq!(*h.current(), 42);
        assert_eq!(h.depth(), 1, "no-op 不得改变历史");
    }

    #[test]
    fn limit_drops_oldest_keeps_current() {
        let mut h = History::new(0).with_limit(3);
        for i in 1..=10 {
            h.commit(i);
        }
        assert_eq!(h.depth(), 3, "历史不超上限");
        assert_eq!(*h.current(), 10, "当前态永远是最新");
        // 只能回退到保留窗口内（10,9,8），第 4 次 undo 到底返回 None
        assert_eq!(h.undo(), Some(&9));
        assert_eq!(h.undo(), Some(&8));
        assert_eq!(h.undo(), None, "更早的已被丢弃");
    }

    #[test]
    fn undo_all_the_way_then_commit_keeps_valid_cursor() {
        let mut h = hist(&[0, 1, 2]);
        while h.can_undo() {
            h.undo();
        }
        assert_eq!(*h.current(), 0);
        h.commit(5); // 从最早态开新分支，丢弃 1,2 未来
        assert_eq!(*h.current(), 5);
        assert!(!h.can_redo());
        assert_eq!(h.depth(), 2);
    }

    // 集成：History 能持纯数据 DAG 快照，undo 回到旧图结构。
    #[test]
    fn graph_snapshots_undo_to_prior_structure() {
        use crate::{Edge, Graph, NodeId};
        let mut g = Graph::default();
        g.add_node(NodeId(0));
        g.add_node(NodeId(1));

        let mut h = History::new(g.clone());
        let mut g2 = g.clone();
        g2.add_edge(Edge {
            from: NodeId(0),
            to: NodeId(1),
            radius: crate::DepRadius::Local,
        })
        .unwrap();
        assert_eq!(h.depth(), 1);
        h.commit(g2);
        assert_eq!(h.current().node_count(), 2);
        // 初始图无此边 ⇒ undo 后回到无边结构
        h.undo();
        let before = h.current();
        assert!(
            before.downstream(NodeId(0)).is_empty(),
            "undo 后应回到未连边的 DAG"
        );
    }
}
