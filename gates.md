# 门禁登记表（gates.md）

纪律：每条**判定类门**上线时必须同时登记"违规判据 + 覆盖面负对照"，缺负对照的门视同未建成（06 文档）。本表随里程碑更新，是 `cargo xtask gate` 的说明书。

| 门 | 状态 | 接线点 | 负对照（会判红的测试） |
|---|---|---|---|
| A1 fmt | ✅ 已建 | `xtask gate` | 未格式化代码必红（天然） |
| A2 clippy(all, -D warnings) | ✅ 已建 | `xtask gate` | 留任一 warning 的提交必红；实测：本轮 identity_op/cast/chunks 三项真实判红 |
| A2 clippy(pedantic: geo/exec/nodes) | ✅ 已建 | `xtask gate` | 同上，pedantic 项本轮判红 10+ 处（must_use/cast/float_cmp）|
| A3 build (--locked, 三平台) | ✅ 已建（本地加 --locked / CI 已接）| `xtask gate` + workflow | Cargo.lock 漂移 ⇒ --locked 判红（lock 已生成并入仓）|
| B1 test + 覆盖率下限 | ✅ test 已建 / ⏳ 覆盖率阈值待 gate.toml 生效 | `xtask gate` + tarpaulin | 删边界测试使 <90%/70% ⇒ 红（阈值脚本化后）|
| B2 CPU/GPU parity | ⏳ M1 建 | 待定 | 改 WGSL 一个常数必红 —— 依赖 strata-gpu 落地 |
| B3 确定性 | ⏳ M2 建 | 待定 | 同 seed 双跑注入非确定性源 |
| C1 golden image | ⏳ M1 建 | 待定 | 改 shader 常数使 L1>阈值 |
| C2 UI 对比度 | ⏳ M3 建 | 待定 | 调低 glass tint 必红 |
| D1 性能回归 | ⏳ M1 建 | 待定 | 注入 O(n²) 退化 |
| D2 内存预算 | ⏳ M1 建 | 待定 | 关 LRU 淘汰使 RSS 超 budget.toml |
| E1 变异门 | ⏳ M2 建 | 待定 | 弱化断言必冒新增存活 |
| F1 三平台矩阵 | ✅ CI 已接（build/test 级）| workflow | 任一平台红 |
| G1 文档门（P1–P8 有归属）| ⏳ 脚本化待定 | 待定 | 新承重算法无 02 出处 ⇒ 红 |

已建门数 **5/14**，覆盖 A/B/C 判定类门中已能立的 fmt/clippy/build/test/F1；判定类硬门（B2/C1/C2/D1/D2/E1/G1）随 GPU/M3/M4 逐个建，每个建时必须同批登记负对照，否则不计入。

## 承重设计已挡住的问题（对照 01，G1 门的雏形）
- P1 → TileCache 字节预算 + LRU（strata-field，本轮 3 个测试锁行为）
- P3 → Graph dirty 传播（strata-exec，本轮 5 测试）
- P4 → WorldMm i64 / LocalF32 / RTE（strata-geo，本轮 5 测试含无舍入金标准）
- P6 底座 → 确定性 i16 序列化（strata-io，本轮 4 测试含逐位一致）
- P8 规约 → parity 键与量化域定义（strata-nodes `fill` 作最小真值语料）
- P2/P5 → M1（SDF/clipmap），P7 → M3 UI
