# 07 · 路线图 —— 先把地形搞好，再搞其他

每个里程碑出口条件 = 06 门线对应步骤全绿 + 本文档验收项。M1–M2 期间**禁止**碰 Python 桥/物理/SDF（防范围蔓延）。

## M0 · 骨架（~1 周当量）

- cargo workspace 按 03 建齐 crate 骨架；winit+wgpu 开出一个空窗口；egui 能画面板。
- `xtask gate` 跑通 A1/A2/A3；CI 三平台矩阵 F1 先只跑 build。
- 验收：三平台 CI 绿；空窗口 60fps 空转。

## M1 · 地形内核 —— 能生成、能看、尺度不崩（核心，最长）

- `strata-geo`：`TilePos{i64,f32}` 类型体系 + RTE 相机（P4 钉死）。
- `strata-field` + tile cache（预算制，P1）。
- `strata-nodes` 第一批：常量场、perlin/simplex fBm、domain warp、terrace、mask 组合（每个 CPU+WGSL 成对，B2 parity 门此时建立）。
- `strata-render`：geometry clipmap 渲染 + skirt + 每层独立 near/far（P5 钉死）；漫游相机（fly/orbit/terrain-follow）。
- 简单 UI：视口 + 一个参数面板（滑条直改 DAG），磨砂玻璃 L0/L2 两层先落地。
- **验收**：
  - 连续漫游 10km×10km 无可见接缝/z-fighting（C1 golden + 人工复核）；
  - 相机拉远 1000km 无抖动（对照 UE LWC 抱怨 U2）；
  - 512MB 预算内稳态帧率 ≥60（D2 门此时建立）。

### 完成状态（截至 M1d，诚实标注）
| 验收项 | 状态 | 证据 |
|---|---|---|
| P4 尺度不抖（局部坐标有界）| ✅ 数学钉死 | clipmap 锚点量化单测：\|local\|<span；世界 i64 毫米进不了 f32 |
| 能生成（位确定）| ✅ | 双进程逐位一致 + 量化金标准（M1a）|
| 能看（GPU 出像素）| ✅ | C1 双哈希：单层 legacy `17092285…` + 十层 clipmap `37792393…`（本机 Vulkan/RTX 实测）|
| P3 漫游增量（不全图重烤）| ✅ | roam 测试 delta∈(0, 37×层)；亚-cell 移动零重生成 |
| 无缝（skirt + halo）| 🟡 数据侧已证 | halo 跨 tile 逐位一致；渲染缝靠 skirt，**观感须人工目测** |
| 512MB 内稳态帧率 ≥60 | ✅ 实测达标 | `--bench-render`：1280×720 十层 **237fps**（P50 4.3ms，含 GPU 同步）；**峰值 RSS 243MB < 512**；D1/D2 门已建（负对照判红）|
| 1000km 拉远无抖（人工）| ⏳ 待目测 | 数学面已钉死（局部坐标有界）；观感由用户 `cargo run -p strata-app` 确认 |

## M2 · 形态模拟 —— 地形"像真的"

- 侵蚀节点族：hydraulic（Mei 2007 + SCITEPRESS 2022 改良项）+ thermal/talus，GPU ping-pong + CPU parity 版（P3：迭代增量只算 dirty tile）。
- 材质基础：坡度/高度/湿度自动 splat + 三平面映射。
- 预设系统（火山岛/峡谷/喀斯特/雪原 4 个起步）。
- 撤销/重做栈（DAG 纯数据，03）。
- **验收**：1024² 区域侵蚀交互式（步进期间不掉出 30fps）；侵蚀语料回归集（Treetop 类基准）进 B1 门。

### 完成状态（截至 M2a，诚实标注）
| 项 | 状态 | 证据 |
|---|---|---|
| CPU 侵蚀核（热 talus + 水力 Mei）| ✅ | `strata-nodes::erosion` 7 测试全绿，进 `xtask gate` B1 |
| 守恒不变量 Σ(h+sed) | ✅ 实测钉死 | 闭合边界 40 步水/沉积守恒 <1e-4；水无源汇守恒 |
| 位确定 + 金标准 | ✅ | 热/水双跑逐位一致；hydraulic 量化首行 `[0,94,0,842,2279,3601,4378,4913]`（本机冻结，B3）|
| 热侵蚀收敛不震荡 | ✅ 判红选定 | 双缓冲并行版过冲振荡被实测否决 ⇒ 改 Gauss-Seidel 就地松弛（凸组合单调）|
| GPU ping-pong + parity(B2) | ✅ B2 门建成（M2b）| `strata-gpu::parity` + `noise_parity.wgsl`（device 申请 SHADER_INT64）对拍 CPU：plain fBm 4096/4096 逐位=0（承重路径），warped ≤1 LSB（本机 4094/4096，差值=FMA 融合经 warp 粗 round 放大，P8 一手证据分层登记）；负对照 seed+1 判红。B2-0 探针另证 u64 算术逐位准、splitmix64 可直译。**仍待**：GPU 侵蚀 ping-pong + 1024² 交互式 ≥30fps |
| 1024² 交互式 ≥30fps | ⏳ M2b | 需 GPU 步进 + dirty-tile 增量 |

## M3 · 双视图 UI —— 简繁同真相

- 节点图视图（可折叠抽屉，04 三段式）；图层堆栈模式编译成同一 DAG。
- 玻璃 L1/L3 层 + prominence 三档 token 化；C2 对比度门建立。
- 首启示例工程、预设盘拖入即得。
- **验收**：新用户 60 秒出第一张地形（可用性脚本实测）；反繁杂清单逐条过。

## M4 · 进出自由 —— 打 P6/G4/H3

- 工程文件 `.strata`（zip+bin）版本化；导入 16bit png/raw/exr；导出 png/exr/obj/gltf。
- **USD 导出**（地形承重件，供 Houdini/Solaris 路线）；导出=写文件，无进程绑定。
- `strata-io` 上 fuzzing（nightly）。
- 整图离线 bake（分块金字塔，超显存场景，P1 收口）。
- **验收**：Blender/Houdini 侧用官方 Python 导入导出结果可复现（脚本进 CI）。

## M5 · Python 桥 + 物理（此处才"搞其他的"）

- `strata-py`：嵌入态插件线程 + pip wheel 双形态（05-A/05-B）。
- rapier 微物理节点：滚石沉积、粒子推沙（回写高度场/mask）。
- 示例插件集进 CI（examples/py 全跑）。
- **验收**：`strata-py` wheel 在干净 venv 复现同 seed 工程输出（发布线 canary 自检项）。

## M6 · 倒扣与洞穴（P2，可选大件）

- SDF 域 + Dual Contouring mesh 导出；heightfield 作为 SDF 特例统一。
- 触发条件：M4 之后有真实需求（游戏用户要洞穴）才启动，否则永久后置。

## 永久后置清单（写了就不做）

- 多人协作、内置植被散布编辑器、水面模拟、贴图烘焙 3D 打印精度修复 —— 均不进 M1–M6。
