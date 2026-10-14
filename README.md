# Strata — Rust 地形工作台

地形类型软件：节点式程序化地形生成 + 侵蚀模拟 + 无级缩放实时渲染，
Rust 内核（GPU-first），UI 走 iOS 26 Liquid Glass（磨砂玻璃）规范，
Python 侧挂物理引擎/外部渲染管线。

## 文档索引

| 文档 | 内容 |
|---|---|
| [01-market-pain-points.md](docs/01-market-pain-points.md) | 热门地形软件（Gaea / World Machine / Houdini / UE Landscape / World Creator / Instant Terra）遇到的问题清单，逐条给出处 |
| [02-papers-tech-map.md](docs/02-papers-tech-map.md) | 论文→技术的映射：每个痛点对应的学术解法与我们采用的方案 |
| [03-architecture.md](docs/03-architecture.md) | Rust 工作区划分、数据模型、DAG 执行器、LOD/精度体系 |
| [04-ui-liquid-glass.md](docs/04-ui-liquid-glass.md) | UI 设计规范：磨砂玻璃分层、"方便易用不繁杂"的硬约束 |
| [05-python-bridge.md](docs/05-python-bridge.md) | PyO3 双向桥：Python 接物理引擎与渲染导出 |
| [06-ci-cd-gates.md](docs/06-ci-cd-gates.md) | 严苛 CI/CD 门禁线（含变异门与 golden image） |
| [07-roadmap.md](docs/07-roadmap.md) | 里程碑：先把地形搞好，再搞其他 |

## 一句话定位

> Gaea 的速度 + Houdini 的可编程性 + Blender 的免费，痛点对着 01 号文档逐条避开。

## 现状

- 里程碑：M0–M1 已关账（多层 clipmap 漫游、位确定生成核、237fps@1280×720、RSS 243MB）；M2 侵蚀待启动
- CI/CD：GitHub Actions 三平台矩阵（`cargo xtask gate` 全链 = 本地同一入口），门禁 9/14 建成且全部带负对照
- 本地跑：`cargo run -p strata-app`（视口）｜`cargo xtask gate`（门线）｜`cargo run -p strata-app -- --bench-render 90`（性能自检）

