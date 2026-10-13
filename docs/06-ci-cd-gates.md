# 06 · CI/CD 严苛门禁

设计口径：每条门必须**能判红**（有负对照），数字判据锚真实测量，不做"建议优化"式软检查。借鉴既有纪律：变异门、金丝雀语料、基线双向披露、收尾整条门线重放。

## 门线总表（`cargo xtask gate` 一键全跑，CI 与本地同一入口）

| # | 门 | 判据 | 失败=红 | 负对照 |
|---|---|---|---|---|
| A1 | fmt | `cargo fmt --check` | 任何 diff | — |
| A2 | clippy | `-D warnings`，core 三 crate（geo/exec/nodes）另开 `pedantic` | 任何警告 | 故意留 warning 的 PR 必红 |
| A3 | 构建锁定 | 全部构建带 `--locked`；`cargo deny`（audit + bans + licenses） | 依赖漂移/漏洞/许可证违规 | 引入已知漏洞 crate 的分支必红 |
| B1 | 单元测试 | `cargo test --workspace`，geo/exec/erosion 行覆盖 ≥ 90%，其余 ≥ 70%（覆盖下限登记在 `gate.toml`，改下限需单独提交说明） | 低于下限 | 删一个边界测试必红 |
| B2 | **CPU/GPU parity** | 每个空间节点：同 seed 跑 CPU 与 WGSL，量化 i16 后**逐位一致** | 任何 tile 不一致 | 故意改 WGSL 里一个常数必红 |
| B3 | 确定性 | 同 seed 双跑逐位一致；跨平台（win/linux/macos）i16 量化一致 | 不一致 | — |
| C1 | 渲染 golden image | headless wgpu（llvmpipe/Metal 离线）渲染固定工程固定机位，L1 像素差 ≤ 阈值，阈值逐图登记 | 超阈值 | 改一个 shader 常数必红 |
| C2 | UI 对比度 | 玻璃 token 合成后文字对比 ≥4.5:1 自动断言（04 规范） | 低于 4.5 | 调低 tint 不透明度必红 |
| D1 | 性能回归 | criterion 关键核：噪声 512² tile、侵蚀 10⁶ 步、clipmap 更新；相对基线劣化 >10% 判红。基线=main 最近绿 commit 实测值，存 `bench/baseline.json` | >10% | 注入 O(n²) 退化必红 |
| D2 | 内存预算 | 固定大场景 RSS 峰值 ≤ `budget.toml`（默认 512MB tile cache + 总 1.5GB）；RSS 由 job object 实测 | 超预算 | 故意关闭 LRU 淘汰必红 |
| E1 | **变异门** | `cargo-mutants` 跑 strata-geo / strata-exec / strata-erosion；存活变异须逐个分诊登记（薄壳等价/断言弱/包作用域），基线外新增存活=红 | 未分诊存活 | 弱化一个断言必冒出新增存活 |
| F1 | 三平台矩阵 | windows-msvc / ubuntu / macos：build+test+gate 全绿才绿；wgpu 各后端（DX12/Vulkan/Metal）至少各跑一次 C1 | 任一平台红 | — |
| G1 | 文档门 | 01 痛点表 P1–P8 每条必须能指向一个"由哪条门/哪个设计决策挡住"；新增承重算法必须先在 02 登记出处 | 孤儿算法 | — |

## 发布线（tag → release）

1. `xtask gate --full`（含变异门 + 三平台 + golden 重录审查）；
2. 三平台产物 + Windows 签名 + macOS notarize；
3. canary 频道（自动装 canary 跑 `strata-py` 自检 + 固定工程复现校验：同 seed 输出哈希对得上）；
4. 人工视觉评审（玻璃层观感、视口帧率截图）→ 才允许 stable。
- 回滚：stable/canary 双频道，发布不可变（版本号不复用）。

## 门禁工程纪律（沿用已验证的教训）

- **门报绿 ≠ 有效**：每道判定类门（B2/C1/C2/D1/D2/G1）建门时必须附一个会判红的负对照测试（金丝雀），登记在 `gates.md`。
- 数字主张（"覆盖 90%""劣化 10%"）必须可复算：判据脚本化，禁止人肉抄数；基线变更走单独提交、单值变更。
- 变异门与性能门的本机成本约束：并发一律 jobs=1（本机内存约束，既有实测教训），scratch/TMP 指到 NVMe 盘并设上限。
- 本地门与 CI 门等价（同一 `xtask gate`），禁止"CI 独有步骤"成为盲区。
- 试验面与承重面分开：golden 阈值、性能基线属承重件，改动要 review；示例工程、基准夹具属试验面，免跑重门。
- 收尾重放：任何里程碑合并前重放**整条门线**，不挑相关步骤跑。

## 明确不做（防过度建设）

- 不追 100% 覆盖；miri/ASAN 只做 weekly 夜间任务，不进合入门（成本 > 收益，M4 后再评估）。
- fuzzing 只对 `strata-io`（解析外部文件）做，libFuzzer 挂 nightly。
- 不做静态分析全家桶（semgrep 等）——Rust 类型系统 + A2/A3 已覆盖大部分面。
