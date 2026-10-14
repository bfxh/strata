//! 预设 / DAG-as-data IR（03 文档「节点 = id + kind + params + inputs，图是纯数据」）。
//!
//! 这是"简繁同真相"的数据底座：预设、图层堆栈、节点图三视图都编译成同一个
//! [`Preset`] 结构，`compile` 出 [`crate::Graph`] 供执行器做 dirty 传播。纯数据 ⇒
//! 可直接喂进 [`super::history::History`] 做撤销/重做。本模块只定义 + 校验 + 编译拓扑，
//! 不含求值（求值在 strata-nodes/GPU）；把预设真正跑成地形属 M3 视口接线。

use std::collections::BTreeMap;

use crate::{DepRadius, Edge, Graph, GraphError, NodeId};

/// 标量参数（03：参数节点吃 f32/i32/seed）。`BTreeMap` 保证遍历/比较确定序。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Param {
    Float(f32),
    Int(i32),
    Seed(u64),
}

/// 一个节点声明：kind 字符串 + 具名参数 + 上游输入 id（顺序即输入端口序）。
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSpec {
    pub id: u32,
    pub kind: String,
    pub params: BTreeMap<String, Param>,
    /// 上游节点 id；每个输入构成一条 `input → self` 边。
    pub inputs: Vec<u32>,
    /// 本节点输出的空间依赖半径（erosion 迭代 N 步 ⇒ Stencil(N)），施加到其所有入边。
    pub dep_radius: DepRadius,
}

/// 一个预设 = 具名节点集合（DAG-as-data）。
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub nodes: Vec<NodeSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetError {
    DuplicateId(u32),
    UnknownInput {
        node: u32,
        input: u32,
    },
    /// 边非法（自环 / 端点缺失），转发自 [`crate::GraphError`]。
    Graph(GraphError),
    /// 拓扑有环，携带环上节点 id。
    Cycle(Vec<u32>),
}

impl NodeSpec {
    /// 便捷构造：无参、Local 半径。
    pub fn new(id: u32, kind: &str) -> Self {
        Self {
            id,
            kind: kind.to_string(),
            params: BTreeMap::new(),
            inputs: Vec::new(),
            dep_radius: DepRadius::Local,
        }
    }

    #[must_use]
    pub fn with_input(mut self, input: u32) -> Self {
        self.inputs.push(input);
        self
    }

    #[must_use]
    pub fn with_param(mut self, key: &str, v: Param) -> Self {
        self.params.insert(key.to_string(), v);
        self
    }

    #[must_use]
    pub fn with_radius(mut self, r: DepRadius) -> Self {
        self.dep_radius = r;
        self
    }
}

impl Preset {
    /// 编译成执行器用的 [`Graph`]：加节点、按 inputs 连边，并校验无重复 id、
    /// 输入引用存在、结果无环。
    ///
    /// # Errors
    ///
    /// - [`PresetError::DuplicateId`]：两个节点同一 id；
    /// - [`PresetError::UnknownInput`]：某输入引用了不存在的节点；
    /// - [`PresetError::Graph`]：边非法（如自环，节点把自己列为输入）；
    /// - [`PresetError::Cycle`]：拓扑有环（`is_dag` 为假）。
    ///
    /// 任一不满足 ⇒ `Err`（fail-closed，不产出半成品图）。
    pub fn compile(&self) -> Result<Graph, PresetError> {
        let mut g = Graph::default();
        let mut seen: Vec<u32> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            if seen.contains(&n.id) {
                return Err(PresetError::DuplicateId(n.id));
            }
            g.add_node(NodeId(n.id));
            seen.push(n.id);
        }
        for n in &self.nodes {
            for &input in &n.inputs {
                if !seen.contains(&input) {
                    return Err(PresetError::UnknownInput { node: n.id, input });
                }
                let e = Edge {
                    from: NodeId(input),
                    to: NodeId(n.id),
                    radius: n.dep_radius,
                };
                g.add_edge(e).map_err(PresetError::Graph)?;
            }
        }
        if !g.is_dag() {
            let cyc = g
                .cycle()
                .unwrap_or_default()
                .into_iter()
                .map(|id| id.0)
                .collect();
            return Err(PresetError::Cycle(cyc));
        }
        Ok(g)
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(n: u32) -> Preset {
        let mut nodes = vec![NodeSpec::new(0, "noise.fbm")];
        for i in 1..n {
            nodes.push(NodeSpec::new(i, "modify").with_input(i - 1));
        }
        Preset {
            name: "chain".into(),
            nodes,
        }
    }

    #[test]
    fn compile_linear_to_valid_dag() {
        let g = chain(4).compile().unwrap();
        assert_eq!(g.node_count(), 4);
        assert!(g.is_dag());
        let mut down = g.downstream(NodeId(0));
        down.sort_by_key(|x| x.0);
        assert_eq!(down, vec![NodeId(1), NodeId(2), NodeId(3)]);
    }

    #[test]
    fn compile_is_deterministic() {
        // 同一预设两次编译 ⇒ 拓扑逐一致（简繁同真相的前提）。
        let a = chain(5).compile().unwrap();
        let b = chain(5).compile().unwrap();
        assert_eq!(a.node_count(), b.node_count());
        for id in 0..5u32 {
            let mut da = a.downstream(NodeId(id));
            let mut db = b.downstream(NodeId(id));
            da.sort_by_key(|x| x.0);
            db.sort_by_key(|x| x.0);
            assert_eq!(da, db, "node {id} 下游不稳定");
        }
    }

    #[test]
    fn duplicate_id_rejected() {
        let p = Preset {
            name: "dup".into(),
            nodes: vec![NodeSpec::new(1, "a"), NodeSpec::new(1, "b")],
        };
        assert_eq!(p.compile().unwrap_err(), PresetError::DuplicateId(1));
    }

    #[test]
    fn unknown_input_rejected() {
        let p = Preset {
            name: "ghost".into(),
            nodes: vec![NodeSpec::new(0, "a").with_input(99)],
        };
        assert_eq!(
            p.compile().unwrap_err(),
            PresetError::UnknownInput { node: 0, input: 99 }
        );
    }

    #[test]
    fn cycle_rejected() {
        let p = Preset {
            name: "ring".into(),
            nodes: vec![
                NodeSpec::new(0, "a").with_input(1),
                NodeSpec::new(1, "b").with_input(0),
            ],
        };
        assert!(matches!(p.compile(), Err(PresetError::Cycle(_))));
    }

    // ---- 4 个起步预设（拓扑真值；观感留待视口目测放行） ----

    fn volcanic_island() -> Preset {
        Preset {
            name: "火山岛".into(),
            nodes: vec![
                NodeSpec::new(0, "noise.fbm")
                    .with_param("wavelength_mm", Param::Int(400_000))
                    .with_param("octaves", Param::Int(5))
                    .with_param("seed", Param::Seed(1)),
                NodeSpec::new(1, "terrace").with_input(0),
                NodeSpec::new(2, "erosion.hydraulic")
                    .with_input(1)
                    .with_param("iterations", Param::Int(24))
                    .with_radius(DepRadius::Stencil(24)),
                NodeSpec::new(3, "output.height").with_input(2),
            ],
        }
    }

    fn canyon() -> Preset {
        Preset {
            name: "峡谷".into(),
            nodes: vec![
                NodeSpec::new(0, "noise.fbm").with_param("seed", Param::Seed(2)),
                NodeSpec::new(1, "domain.warp").with_input(0),
                NodeSpec::new(2, "erosion.hydraulic")
                    .with_input(1)
                    .with_param("iterations", Param::Int(40))
                    .with_radius(DepRadius::Stencil(40)),
                NodeSpec::new(3, "output.height").with_input(2),
            ],
        }
    }

    fn karst() -> Preset {
        Preset {
            name: "喀斯特".into(),
            nodes: vec![
                NodeSpec::new(0, "noise.fbm").with_param("seed", Param::Seed(3)),
                NodeSpec::new(1, "erosion.thermal")
                    .with_input(0)
                    .with_param("iterations", Param::Int(16))
                    .with_radius(DepRadius::Stencil(16)),
                NodeSpec::new(2, "erosion.hydraulic")
                    .with_input(1)
                    .with_param("iterations", Param::Int(30))
                    .with_radius(DepRadius::Stencil(30)),
                NodeSpec::new(3, "mask.combine").with_input(2),
                NodeSpec::new(4, "output.height")
                    .with_input(2)
                    .with_input(3),
            ],
        }
    }

    fn snowfield() -> Preset {
        Preset {
            name: "雪原".into(),
            nodes: vec![
                NodeSpec::new(0, "noise.fbm").with_param("seed", Param::Seed(4)),
                NodeSpec::new(1, "erosion.thermal")
                    .with_input(0)
                    .with_param("iterations", Param::Int(10))
                    .with_radius(DepRadius::Stencil(10)),
                NodeSpec::new(2, "mask.combine").with_input(1),
                NodeSpec::new(3, "output.height").with_input(2),
            ],
        }
    }

    #[test]
    fn all_four_presets_compile_to_dags() {
        let presets = [volcanic_island(), canyon(), karst(), snowfield()];
        for p in presets {
            let g = p
                .compile()
                .unwrap_or_else(|e| panic!("预设 {} 编译失败: {e:?}", p.name));
            assert!(g.is_dag(), "预设 {} 编译出环", p.name);
            assert_eq!(
                g.node_count(),
                p.node_count(),
                "预设 {} 节点数与拓扑不符",
                p.name
            );
            assert!(g.node_count() >= 3, "预设 {} 太薄", p.name);
        }
    }

    #[test]
    fn presets_feed_undo_history() {
        // 预设可作纯数据快照进撤销栈（03「图是纯数据 ⇒ 撤销=状态机回退」的闭环验证）。
        let mut h = super::super::history::History::new(volcanic_island());
        h.commit(canyon());
        h.commit(karst());
        assert_eq!(h.current().name, "喀斯特");
        h.undo();
        assert_eq!(h.current().name, "峡谷");
        assert_eq!(h.current().compile().unwrap().node_count(), 4);
    }
}
