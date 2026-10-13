# 01 · 热门地形软件问题清单

证据等级：**A** = 论文/官方文档原文；**B** = 用户社区一线报告；**C** = 业内常识（无单一出处，标注为待实测确认）。

## Gaea (QuadSpinner)

| # | 问题 | 等级 | 出处/备注 |
|---|---|---|---|
| G1 | 全 GPU 纹理管线 ⇒ 分辨率与显存是硬上限；大图必须分块拼接，全局微调难 | B | polycount 对比帖："fast because it's all GPU but that means it has limitations" |
| G2 | 2.5D 模型（heightfield 单值高度）⇒ 悬崖倒扣、洞穴、拱桥表达不了 | C | 架构性限制，所有 raster 地形软件共有 |
| G3 | 改一个节点要重烤下游整条链，无 tile 级增量 | C | 交互卡顿主因之一 |
| G4 | 导出格式闭源倾向，pipeline 交付按版本收费分层 | B | 官网将 "pipeline delivery" 列为卖点/分级 |
| G5 | 老 GPU/驱动兼容崩溃 | B | foundry 社区讨论中多次提及 |

## World Machine

| # | 问题 | 等级 | 出处/备注 |
|---|---|---|---|
| W1 | CPU 计算，大图迭代以分钟计 | B | polycount/社区一致口径 |
| W2 | 代码库陈旧、更新近乎停滞（团队转入 QuadSpinner） | B | 社区共识 |
| W3 | 复杂图内存占用巨大，大分辨率 bake 常 OOM | B | 社区抱怨高频项 |
| W4 | mesh 导出（displace→mesh）转换质量差、噪点重 | B | polycount 帖 |
| W5 | Win32 老 UI，预览窗口质量低，学习曲线陡 | B | 对比视频中直观可见 |

## Houdini（heightfield / Mountain）

| # | 问题 | 等级 | 出处/备注 |
|---|---|---|---|
| H1 | 通用 DCC 的一个分支，搭地形要学整套 SOP/DOP 心智，门槛极高 | B | reddit r/unrealengine 对比帖 |
| H2 | 侵蚀/沉积模拟 CPU 为主，单帧几十万步不实时 | C | Hydbaed/Hydrolib 常识 |
| H3 | 与 UE 往返（bake→import）流程脆：UE5 World Partition + Houdini landscape 存档/烘焙必崩 | A/B | sidefx 论坛实例帖（crash on save / crash on bake，一致性复现） |

## Unreal Engine Landscape + World Partition

| # | 问题 | 等级 | 出处/备注 |
|---|---|---|---|
| U1 | World Partition cell 不卸载 / 流送卡顿，车一开快就 pop | B | UE 官方论坛帖、社区视频（"Faulty WP settings"） |
| U2 | LWC 解决了物体位置，但 landscape 顶点仍单精度 ⇒ 大坐标抖动 | C | Epic 已知问题区反复讨论 |
| U3 | 固定 gridsize（63/127/255 quads）约束多； sculpt 刷和层材质（8 层上限）难用 | B | UE 文档与社区 |

## 自研/开源地形（r/proceduralgeneration、教学实现）

| # | 问题 | 等级 | 出处/备注 |
|---|---|---|---|
| O1 | LOD 层间接缝、T-junction 裂缝 | A | TUM 地形讲义明确列为 "Problems: Z-fighting, terrain LOD adaptation" |
| O2 | 大世界 float 精度抖动（模型抖动、刷选错位） | A | Geometry Clipmaps 论文动机段 |
| O3 | 侵蚀在 CPU 上每步全图扫描，256² 以上就掉帧 | A | Mei 2007 GPU 论文的 motivation |
| O4 | 噪声在 CPU（玩法查询）与 GPU（位移）两套实现不一致 ⇒ 角色悬空/钻地 | B | bevy_noised 存在的理由（"CPU/GPU parity"） |

## 提炼：八类必须在架构层避开的问题

1. **P1 分辨率/显存硬顶**（G1, W3）→ 分块流式 + 内存预算制，不把任何尺寸写死
2. **P2 heightfield 单值顶**（G2）→ 域抽象，留 SDF/mesh 逃生口（低优先级）
3. **P3 无增量重算**（G3, W1）→ tile 级 dirty 传播 + 异步执行器
4. **P4 尺度与精度**（U2, O2）→ i64 分块坐标 + 局部 f32 + relative-to-eye，禁世界坐标单精度
5. **P5 LOD 缝/z-fighting**（O1, U1）→ geometry clipmap + skirt，深度分配显式设计
6. **P6 导出黑箱/往返脆**（G4, H3, W4）→ 开放格式一等公民（16bit png/raw/exr/gltf/USD），导出走文件不走进程绑定
7. **P7 繁杂 UI / 学习曲线**（H1, W5, G3）→ 简单堆栈模式默认，节点图渐进展开
8. **P8 CPU/GPU 结果不一致**（O4, O3）→ 每节点 CPU↔WGSL parity 测试钉进 CI

Sources:
- [Gaea vs World Machine vs World Creator vs Instant Terra (polycount)](https://polycount.com/discussion/228295/gaea-vs-world-machine-vs-world-creator-vs-instant-terra)
- [Houdini, Gaea, or World Creator? (r/unrealengine)](https://www.reddit.com/r/unrealengine/comments/15modpt/houdini_gaea_or_world_creator/)
- [Terrain software discussion (Foundry community)](https://community.foundry.com/discuss/topic/149203/terrain-software-e-on-terragen-quadspinner-gaea-etc-how-is-this-software-these-days)
- [Landscape tiles output not working for UE5 World Partition (sidefx forum)](https://www.sidefx.com/forum/post/457817/)
- [World Partition Cells not unloading properly (UE forums)](https://forums.unrealengine.com/t/world-partition-cells-not-unloading-properly/537984)
- [TUM Terrain Rendering 讲义（Z-fighting / LOD adaptation）](https://www.cs.cit.tum.de/fileadmin/w00cfj/cg/Research/Tutorials/Terrain.pdf)
- [bevy_noised（CPU/GPU parity 动机）](https://docs.rs/bevy_noised)
