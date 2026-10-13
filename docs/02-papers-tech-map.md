# 02 · 论文 → 技术映射

每个痛点（P1–P8，见 01）对应采用哪篇论文/哪个工程实践，以及为什么选它而不是备选。

## P4+P5 尺度与 LOD：Geometry Clipmaps（主方案）

- **采用**：Humus & Hopper, *Geometry Clipmaps: Terrain Rendering Using Nested Regular Grids*, SIGGRAPH 2004。
  - 嵌套以观察者为中心的正则网格；地形存成 vertex texture，GPU 取高度 ⇒ 数据是"图像"，天然分块流式。
  - 论文同时给出 z-fighting 对策（每层用独立 near/far 视锥）——直接钉死 P5。
- **工程参照**：GPU Gems 2 Ch.2（NVIDIA 实现版，vertex texture fetch + 层更新即 copy 一小条）。
- **超大尺离线补强（后期可选）**：Zou et al., *GigATerrain2/3*（view-dependent 离线网格 + 远程流送）与 *Virtual Terrain Project* 的 CLOD 系；M1 不做，接口留位。
- **备选与否决**：
  - 四叉树 CLOD（Dyken/ROAM 系）：裂缝处理（T-junction）与内存局部性都比分层 clipmap 差，否决。
  - Nanite 式 meshlet：需要预烘焙 mesh 资产，与"程序化实时生成"心智冲突，否决为默认路径。
- **精度协议**（钉 P4）：世界坐标 = `(i64 tile, f32 local)`；渲染全部 relative-to-eye（Thatcher 2021 双模式 clipmap 论文的核心做法），相机矩阵每帧减去 eye，浮点抖动在小数范围内归零。

## O3/G2 侵蚀：GPU 侵蚀族

- **采用**：Mei, Lecotre, Chiba, *Fast Hydraulic Erosion Simulation and Visualization on GPU*, VIPocu 2007（被引 159）。
  - 每像素水滴迁移（inflow/outflow/溶解/蒸发），全程 ping-pong texture，可放 compute shader 且可增量。
- **改良叠加**：
  - Loop & Blinn 系与 *Visually Improved Erosion Algorithm*（SCITEPRESS 2022）：解决 Mei 原法的"过度平坦化/河道过直"观感问题。
  - 热力侵蚀（thermal/talus angle）：廉价但视觉收益大，作为独立节点。
- **CPU 版本**：同一算法 Rust 实现一份（tile 级），用于 parity 测试与无 GPU fallback。
- **备选否决**：Houdini 式粒子流水（Quality 高、成本爆炸）——留作 Python 桥后期可选后端（05 文档），不进内核。
- **CI 语料**：Treetop erosion dataset 一类基准 ⇒ 侵蚀内核的金丝雀语料（06 文档）。

## O4/P8 噪声一致律：CPU/GPU parity

- **算法基**：FastNoiseLite（Jordan, MIT）的多倍频/fBm/multifractal/domain-warp 族；论文侧 Szeliski *Fractal Modeling* 与 Perlin 原始改进（Improved Perlin, Perlin 2002）。
- **工程约束**：每种噪声提供 **WGSL + Rust 两份实现，同 seed 同输出**（bevy_noised 的 "gameplay + rendering parity" 是同类需求的业界确认）。
- 注意：GPU 三角函数（sin/pow/exp）跨厂商非位确定 ⇒ parity 定义在**容差 + 量化后一致**（高度场量化为 i16 后必须逐位一致），不是裸 f32 相等。这条写进 CI 判据（06）。

## G2 倒扣/洞穴（低优先级，M6）

- **采用路线**：粗体素 SDF + Dual Contouring（Ju et al. 2002）/ Compact DC（Ni 2004）；体素用 OpenVDB 稀疏结构或自研 octree。
- heightfield 是 SDF 的特例（单值切片）⇒ 域抽象设计时把 heightfield 定义为 `SDF ∩ 单调列`，为 P2 留逃生口而不立刻实现。

## G3/P3 增量重算：DAG dirty 传播

- 无单一对应论文，工程参照：Gaea/WM 的 nodegraph 缓存缺陷 + 函数式增量计算（Adapton / rustpl 思路，仅借鉴概念不自研框架）。
- 设计定为：**节点声明输入依赖的 tile 区域 ⇒ 上游 tile dirty ⇒ 下游只重算受影响 tile**。实现放 `strata-exec`（03 文档）。

## G1/P1 显存与内存：tile 流式 + 预算制

- Clipmap 论文本身即"常驻小窗口 + 磁盘金字塔"模型；纹理金字塔（mip chain on disk）补离线大图。
- 用户侧硬约束（本机内存常年 2–3G 空闲）⇒ **内存预算是一等公民**：tile cache 有显式 budget，超限 LRU 淘汰并降级 mip，绝不让 OOM 成为崩溃路径。

## 参考清单

- [Geometry Clipmaps (Hoppe 原版 PDF)](https://hhoppe.com/proj/geomclipmap/)
- [GPU Gems 2 Ch.2 — GPU-Based Geometry Clipmaps](https://developer.nvidia.cn/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry)
- [Mei et al. 2007, Fast Hydraulic Erosion on GPU](https://inria.hal.science/inria-00402079/document)
- [Isheden 2022, Hydraulic Erosion on GPU for 3D terrains（学位论文，实现细节全）](https://www.diva-portal.org/smash/get/diva2:1646074/FULLTEXT01.pdf)
- [SCITEPRESS 2022, Visually Improved Erosion Algorithm](https://www.scitepress.org/Papers/2022/107997/107997.pdf)
- [GPU clipmaps 复现论文 (TU Wien 2008)](https://www.cg.tuwien.ac.at/research/publications/2008/fruehstueck-2008-gpu/fruehstueck-2008-gpu-paper.pdf)
- [ARM OpenGL ES clipmap 实现样例](https://arm-software.github.io/opengl-es-sdk-for-android/terrain.html)
- [CMU 15-618 GPU 侵蚀项目报告](https://patiltanma.github.io/15618-FinalProject/)
- [FastNoiseLite（crates.io terrain 关键词页）](https://crates.io/keywords/terrain?sort=alpha)
- [bevy_noised — CPU/GPU parity 动机](https://docs.rs/bevy_noised)
