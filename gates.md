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
| C1 golden image | ✅ 建成（本机口径）：两张冻结哈希——C1#1 单层 legacy `17092285482363414448`、C1#2 十层 clipmap 场景 `3779239380702403447`；各跑双次自复现 + 非空图；无 adapter 机器 SKIP 并披露 | `cargo test -p strata-app --test c1_golden` | 改 WGSL/mesh/层参数必红；跨平台基线分文件登记待接入 |
| P3 漫游增量 | ✅ 建成：37 帧斜线漫游重生成次数 ∈ (0, 37×层) —— 既非全图重烤也非从不更新 | `roam_regen_is_incremental_not_full_rebake` | 把 move_eye 改成全 dirty ⇒ delta 触上限红；改成不动 ⇒ delta=0 红 |

> M1c 视口冒烟（非门，属人工验收）：strata.exe 渲染循环存活 8s 无 panic（eframe wgpu 面 + paint callback + terrain/sprite 双管线多帧执行）。**像素观感本代理会话无法人工确认，需用户 `cargo run -p strata-app` 目测**；拖拽/WASD/滚轮的交互手感同为待用户验证项。
| C2 UI 对比度 | ⏳ M3 建 | 待定 | 调低 glass tint 必红 |
| D1 性能回归 | ✅ 建成（本机口径）：`--bench-render` 实测 CPU 核 + GPU 帧管线（每帧 poll(Wait) 真同步）；基线 bench/baseline.env（NOISE 60.7ms / REGEN 19.4ms / FPS 237.3 / P50 4.33ms），余量 CPU+10%、FPS−25%（debug 口径、不含 egui/UI，已登记） | `xtask bench --check`（gate 内自动） | selftest 负对照：CPU 劣化2×/FPS掉40%/RSS超预算 全判红、5%抖动不误红；比较器与实门共用同一 evaluate_against |
| D2 内存预算 | ✅ 建成（bench 面）：GetProcessMemoryInfo 峰值 RSS 自报（本机 243MB < 上限 512MB） | 同上（PEAK_RSS_MB_MAX=512） | RSS 项入负对照表；UI 壳层 RSS 属 M3 另建 |
| E1 变异门 | ⏳ M2 建 | 待定 | 弱化断言必冒新增存活 |
| F1 三平台矩阵 | ✅ CI 已接（build/test/gate 级）| workflow `gate.yml`（push/PR on master）| 任一平台红 |

## CI/CD 现状与债务（GitHub Actions @ bfxh/strata，private）
- 三平台矩阵跑 `cargo xtask gate` 全链 = 本地同一入口；deps job 单独钉 `--locked` 一致性。
- **无登记 GPU 的 runner 上的行为（设计而非缺陷）**：C1 哈希项与 D1/D2 数值项按 TARGET 平台键 SKIP 并在日志披露；**恒判项**=双跑复现性、非空图、selftest 负对照、fmt/clippy/build/test 全家。
- 债务（M2 接入，首版不发防假红掩盖真红）：① 跨平台 golden/bench 分文件基线登记；② cargo-deny（audit/licenses）；③ 覆盖率下限 tarpaulin；④ release 流水线（tag→三平台产物→canary）。
| G1 文档门（P1–P8 有归属）| ⏳ 脚本化待定 | 待定 | 新承重算法无 02 出处 ⇒ 红 |

已建门数 **9/14**（D1/D2 入列，均带负对照），B2（CPU/GPU parity）仍是 ⏳——现 shader 采的是 CPU 生成的多层高度纹理，GPU 独立算噪声的成对实现未落地，不能算 parity 已建。判定类硬门随后续里程碑逐个建，每个建时必须同批登记负对照，否则不计入。

## 承重设计已挡住的问题（对照 01，G1 门的雏形）
- P1 → TileCache 字节预算 + LRU（strata-field，本轮 3 个测试锁行为）
- P3 → Graph dirty 传播（strata-exec，5 测试）+ **RegionCache 增量实测**：4 tile 首建 4 次求算、失效 1 块只重算 1 次（strata-nodes::region）
- P4 → WorldMm i64 / LocalF32 / RTE（strata-geo，5 测试含无舍入金标准）
- P5(数据侧) → halo 重叠列跨 tile 逐位一致（region::halo 测试）；渲染侧 skirt/独立 near-far 待 M1-GPU
- P6 底座 → 确定性 i16 序列化（strata-io，4 测试含逐位一致）
- P8 规约 → 位确定 noise 核（无超越函数）+ fill 真值 + 量化金标准
- P2 → M6（SDF）
