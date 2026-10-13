# 门禁登记表（gates.md）

纪律：每条**判定类门**上线时必须同时登记"违规判据 + 覆盖面负对照"，缺负对照的门视同未建成（06 文档）。本表随里程碑更新，是 `cargo xtask gate` 的说明书。

| 门 | 状态 | 接线点 | 负对照（会判红的测试） |
|---|---|---|---|
| A1 fmt | ✅ 已建 | `xtask gate` | 未格式化代码必红（天然） |
| A2 clippy(all, -D warnings) | ✅ 已建 | `xtask gate` | 留任一 warning 的提交必红；实测：本轮 identity_op/cast/chunks 三项真实判红 |
| A2 clippy(pedantic: geo/exec/nodes) | ✅ 已建 | `xtask gate` | 同上，pedantic 项本轮判红 10+ 处（must_use/cast/float_cmp）|
| A3 build (--locked, 三平台) | ✅ 已建（本地加 --locked / CI 已接）| `xtask gate` + workflow | Cargo.lock 漂移 ⇒ --locked 判红（lock 已生成并入仓）|
| B1 test + 覆盖率下限 | ✅ test 已建 / ⏳ 覆盖率阈值待 gate.toml 生效 | `xtask gate` + tarpaulin | 删边界测试使 <90%/70% ⇒ 红（阈值脚本化后）|
| B3 确定性（CPU 侧先行）| 🟡 半建：同 seed 双跑逐位相等 + 量化金标准 `[0,1130,80,1339,2642,3825,2969,4017]`（strata-nodes::region）；跨平台面 M4 接 | `cargo test` | 扰动 noise 核一个常数 ⇒ 金标准红；GPU 侧待 B2 |
| B2 CPU/GPU parity | ⏳ M1-GPU 建 | 待定 | 改 WGSL 一个常数必红 —— 依赖 strata-gpu 落地 |
| C1 golden image | 🟡 半建：本机 Vulkan/RTX4060Ti 真渲染 128²，三判决面（非空图/双跑复现/冻结哈希 17092285482363414448）；无 adapter 机器 SKIP 并披露 | `cargo test -p strata-app --test c1_golden` | 改 WGSL/拓扑必红（本轮建成时即真判红 3 次）；跨平台基线分文件登记待接入 |
| C2 UI 对比度 | ⏳ M3 建 | 待定 | 调低 glass tint 必红 |
| D1 性能回归 | ⏳ M1 建 | 待定 | 注入 O(n²) 退化 |
| D2 内存预算 | ⏳ M1 建 | 待定 | 关 LRU 淘汰使 RSS 超 budget.toml |
| E1 变异门 | ⏳ M2 建 | 待定 | 弱化断言必冒新增存活 |
| F1 三平台矩阵 | ✅ CI 已接（build/test 级）| workflow | 任一平台红 |
| G1 文档门（P1–P8 有归属）| ⏳ 脚本化待定 | 待定 | 新承重算法无 02 出处 ⇒ 红 |

已建门数 **6/14**（C1 半建计入），B2（CPU/GPU parity）仍是 ⏳——现 shader 采的是 CPU 生成的纹理，GPU 噪声核未落地，不能算 parity 已建。判定类硬门随后续里程碑逐个建，每个建时必须同批登记负对照，否则不计入。

## 承重设计已挡住的问题（对照 01，G1 门的雏形）
- P1 → TileCache 字节预算 + LRU（strata-field，本轮 3 个测试锁行为）
- P3 → Graph dirty 传播（strata-exec，5 测试）+ **RegionCache 增量实测**：4 tile 首建 4 次求算、失效 1 块只重算 1 次（strata-nodes::region）
- P4 → WorldMm i64 / LocalF32 / RTE（strata-geo，5 测试含无舍入金标准）
- P5(数据侧) → halo 重叠列跨 tile 逐位一致（region::halo 测试）；渲染侧 skirt/独立 near-far 待 M1-GPU
- P6 底座 → 确定性 i16 序列化（strata-io，4 测试含逐位一致）
- P8 规约 → 位确定 noise 核（无超越函数）+ fill 真值 + 量化金标准
- P2 → M6（SDF）
