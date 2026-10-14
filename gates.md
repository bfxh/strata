# 门禁登记表（gates.md）

纪律：每条**判定类门**上线时必须同时登记"违规判据 + 覆盖面负对照"，缺负对照的门视同未建成（06 文档）。本表随里程碑更新，是 `cargo xtask gate` 的说明书。

| 门 | 状态 | 接线点 | 负对照（会判红的测试） |
|---|---|---|---|
| A1 fmt | ✅ 已建 | `xtask gate` | 未格式化代码必红（天然） |
| A2 clippy(all, -D warnings) | ✅ 已建 | `xtask gate` | 留任一 warning 的提交必红；实测：本轮 identity_op/cast/chunks 三项真实判红 |
| A2 clippy(pedantic: geo/exec/nodes) | ✅ 已建 | `xtask gate` | 同上，pedantic 项本轮判红 10+ 处（must_use/cast/float_cmp）|
| A3 build (--locked, 三平台) | ✅ 已建（本地加 --locked / CI 已接）| `xtask gate` + workflow | Cargo.lock 漂移 ⇒ --locked 判红（lock 已生成并入仓）|
| B1 test + 覆盖率下限 | ✅ test 已建 / ⏳ 覆盖率阈值待 gate.toml 生效 | `xtask gate` + tarpaulin | 删边界测试使 <90%/70% ⇒ 红（阈值脚本化后）|
| B3 确定性（CPU 侧先行）| 🟡 半建：同 seed 双跑逐位相等 + 量化金标准 `[0,1130,80,1339,2642,3825,2969,4017]`（strata-nodes::region）+ **M2a 侵蚀金标准 `[0,94,0,842,2279,3601,4378,4913]`**（hydraulic 25 步双跑逐位 + 热/水 Σ(h+sed) 守恒 40 步）；跨平台面 M4 接 | `cargo test` | 扰动 noise/erosion 核一个常数 ⇒ 金标准红；GPU 侧待 B2 |
| B2 CPU/GPU parity | ✅ 建成（同机差分，不需平台哈希）：**噪声**`noise_parity.wgsl` + **水力侵蚀**`hydraulic_parity.wgsl`（device 申请 `SHADER_INT64`），量化 i16 对拍。**跨后端每格 max\|Δ\| ≤ 1 LSB 是诚实口径**（两平台实测钉死）：本机 Vulkan plain 4096/4096=0、warped 4094/4096；**CI macOS plain 4095/4096（1 格差 1 LSB）** ⇒ WGSL 允许驱动融 `a*b+c`→FMA，1-ulp 差在量化 .5 边界翻 LSB，连 plain 都会。**教训：本机 strict=0 是过拟合本机驱动的假象，第二平台才揭穿（本机绿≠CI 绿）**。**侵蚀核单步 max\|Δ\|=0**（三场逐位，证 compute 核忠实），但**多步是混沌放大**（本机 12 步 CPU↔GPU max\|Δ\|≈652）⇒ 门定"单步钉死核忠实 + GPU 多步自复现"，不假装多步跨后端逐位（P3/P8 固有）。**Naga 硬坑**：compute 函数按值返回 `array<f32,8>` ⇒ 驱动 0xc0000005 崩（非验证错，error scope 抓不到），改直写 storage。恒判 GPU 双跑自复现；无 adapter/SHADER_INT64 ⇒ SKIP 披露 | `cargo test -p strata-app --test b2_parity` + `--test b2_erosion_parity`（gate B1 自动） | 负对照实测：噪声 seed+1⇒Δ26922；侵蚀 k_capacity×2 单步⇒Δ210；均 ≫1 判红 |
| C1 golden image | ✅ 建成（本机口径）：两张冻结哈希——C1#1 单层 legacy `17092285482363414448`、C1#2 十层 clipmap 场景 `3779239380702403447`；各跑双次自复现 + 非空图；无 adapter 机器 SKIP 并披露 | `cargo test -p strata-app --test c1_golden` | 改 WGSL/mesh/层参数必红；跨平台基线分文件登记待接入 |
| P3 漫游增量 | ✅ 建成：37 帧斜线漫游重生成次数 ∈ (0, 37×层) —— 既非全图重烤也非从不更新 | `roam_regen_is_incremental_not_full_rebake` | 把 move_eye 改成全 dirty ⇒ delta 触上限红；改成不动 ⇒ delta=0 红 |

> M1c 视口冒烟（非门，属人工验收）：strata.exe 渲染循环存活 8s 无 panic（eframe wgpu 面 + paint callback + terrain/sprite 双管线多帧执行）。**像素观感本代理会话无法人工确认，需用户 `cargo run -p strata-app` 目测**；拖拽/WASD/滚轮的交互手感同为待用户验证项。
| C2 UI 对比度 | 🟡 纯数据层建成（`strata-ui`，std-only 无 wgpu/egui）：WCAG 相对亮度/对比 + 半透明合成 `over` + **最坏底判据**（有效底色随内容亮度单调 ⇒ 只需纯黑/纯白两极端取 min，有依据非乱采样）；token alpha 不是照抄 04 起点而是 `min_alpha_for` **实测反推 + 余量** 锁定（实测深色 regular 04 起点 0.55 合成到白底最坏仅 3.11 违反 AA —— 正是 04 要防的 iOS26 批评被门抓到，遂抬到 0.72/0.85 dark、0.62/0.80 light，当前最坏对比 5.27/5.65）。像素级 golden-image 断言仍待 M3 视口接线。**已接进 `xtask gate` 当独立判定项**（`c2_gate`，非匿名 B1） | `xtask gate` 内 `✓ C2 对比度(数据)`（跑 `cargo test -p strata-ui --lib`） | 负对照双证：① alpha 压到 0.30 ⇒ violations 非空判红；② **断言失踪守卫**——临时 `#[ignore]` 掉一条测试 ⇒ passed=7<8 ⇒ C2 判红而 B1 仍绿（抓到「测试静默失踪」，B1 数不出） |
| D1 性能回归 | ✅ 建成（本机口径）：`--bench-render` 实测 CPU 核 + GPU 帧管线（每帧 poll(Wait) 真同步）；**`--bench-erosion` 实测 1024² GPU 水力侵蚀步进（本机 8 步/帧 = 245.9fps / 0.508ms 每步，验收线「步进期间 ≥30fps」大余量）**；基线 bench/baseline.env，余量 CPU+10%、**帧时间(含 EROSION_STEP_MS) +25%**、FPS(含 EROSION_FPS) −25%（帧时间属 GPU 负载敏感项，实测 debug 面峰峰抖 ~25%，故从 CPU±10% 改归帧时间档；单值改动已披露） | `xtask bench --check`（gate 内自动） | selftest 负对照 8 项：CPU 劣化2×/FPS掉40%/帧时间劣化2×/**侵蚀FPS掉50%/侵蚀单步劣化2×**/RSS超预算 全判红；帧时间+15%、CPU+5% 不误红；比较器与实门共用同一 evaluate_against |
| D2 内存预算 | ✅ 建成（bench 面）：GetProcessMemoryInfo 峰值 RSS 自报（本机 243MB < 上限 512MB） | 同上（PEAK_RSS_MB_MAX=512） | RSS 项入负对照表；UI 壳层 RSS 属 M3 另建 |
| E1 变异门 | ⏳ M2 建 | 待定 | 弱化断言必冒新增存活 |
| F1 三平台矩阵 | ✅ CI 已接（build/test/gate 级）| workflow `gate.yml`（push/PR on master）| 任一平台红 |

## CI/CD 现状与债务（GitHub Actions @ bfxh/strata，private）
- **首次全绿已达成**（run 38499370906，2026-10-14，3m37s）：deps + windows/ubuntu/macos 四 job 全 success。
- 首轮 ubuntu 判红为真缺陷（eframe 关 default-features 后 Linux 需显式 x11/wayland），修复 `c477b19` 实证——三平台矩阵不是装饰。
- 三平台矩阵跑 `cargo xtask gate` 全链 = 本地同一入口；deps job 单独钉 `--locked` 一致性。
- **无登记 GPU 的 runner 上的行为（设计而非缺陷）**：C1 哈希项与 D1/D2 数值项按 TARGET 平台键 SKIP 并在日志披露；**恒判项**=双跑复现性、非空图、selftest 负对照、fmt/clippy/build/test 全家。
- 债务（M2 接入，首版不发防假红掩盖真红）：① 跨平台 golden/bench 分文件基线登记；② cargo-deny（audit/licenses）；③ 覆盖率下限 tarpaulin；④ release 流水线（tag→三平台产物→canary）。
- 🔴 债务（2026-10-15 目测暴露）：**窗口 sprite 贴回路径零测试覆盖**——C1 golden 读离屏 target 本身、绕过 sprite，M1c 只冒烟"无 panic"，结果 `h=from_bottom_px-top_px` 算成 0 致视口全白却一路 CI 绿（已修 `54803ff`）。待补：headless 走完整 sprite blit（terrain→display 纹理→回读非空且≈target）的窗口路径回归测，把"空白视口"钉成会判红。
| G1 文档门（P1–P8 有归属）| ⏳ 脚本化待定 | 待定 | 新承重算法无 02 出处 ⇒ 红 |

已建门数 **10/14**（M2b-B2 入列：CPU↔WGSL 噪声核同机差分对拍，plain 逐位=0、warped≤1 LSB 分层容差，负对照 seed+1 判红）。C2（UI 对比度）纯数据层已建（`strata-ui`，最坏底判据 + 实测反推 token alpha + 压 alpha 负对照），但**像素级 golden-image 断言待 M3 视口接线，暂不计入完整门**。判定类硬门随后续里程碑逐个建，每个建时必须同批登记负对照，否则不计入。

## 承重设计已挡住的问题（对照 01，G1 门的雏形）
- P1 → TileCache 字节预算 + LRU（strata-field，本轮 3 个测试锁行为）
- P3 → Graph dirty 传播（strata-exec，5 测试）+ **RegionCache 增量实测**：4 tile 首建 4 次求算、失效 1 块只重算 1 次（strata-nodes::region）
- P4 → WorldMm i64 / LocalF32 / RTE（strata-geo，5 测试含无舍入金标准）
- P5(数据侧) → halo 重叠列跨 tile 逐位一致（region::halo 测试）；渲染侧 skirt/独立 near-far 待 M1-GPU
- P6 底座 → 确定性 i16 序列化（strata-io，4 测试含逐位一致）
- P8 规约 → 位确定 noise 核（无超越函数）+ fill 真值 + 量化金标准；**B2 门实装 CPU↔WGSL 逐位对拍**（plain 0 / warped ≤1 LSB，FMA 边界显式登记）
- P2 → M6（SDF）
