# 03 · Rust 架构

## 工作区划分（一个 cargo workspace）

```
strata/
├─ crates/
│  ├─ strata-geo      # 坐标与精度体系：i64 tile 坐标、RTE 相机、无 unsafe 数值核
│  ├─ strata-field    # 域抽象：HeightTile / Mask / (后期 SDF)；tile 生命周期与预算
│  ├─ strata-exec     # 节点 DAG IR + 增量执行器（dirty tile 传播）
│  ├─ strata-nodes    # 节点实现：CPU Rust 版 + WGSL 版成对存放（parity 单位）
│  ├─ strata-gpu      # wgpu 设备/队列、compute 调度、texture-array tile cache
│  ├─ strata-render   # clipmap 渲染器、材质 splat、调试叠加层
│  ├─ strata-io       # 工程文件(.strata=zip) + 导入导出（16bit png/raw/exr/gltf/obj/USD）
│  ├─ strata-app      # egui 应用壳（组装层，不放逻辑）
│  └─ strata-py       # PyO3 双向桥（05 文档）
├─ shaders/           # WGSL，单独编译检查进 CI
└─ xtask/             # 门禁编排（cargo xtask gate）
```

依赖方向单向：`app → render/exec/nodes/io → gpu/field/geo`。`geo/field` 不依赖 wgpu（可进 no_std 化测试与 Python headless 用）。

## 核心数据模型

**Field**：一切节点输入输出。三种初始形态：

| 形态 | 存储 | 说明 |
|---|---|---|
| `HeightTile` | f32 网格（量化交付时 i16） | 主战场；tile 边距 halo 重叠 8px 消缝 |
| `MaskTile` | u8/f32 网格 | 选择域（侵蚀强度、材质权重来源） |
| `Scalar` | f32/i32/seed | 参数节点 |

- 世界 = 无限 tile 空间：`TilePos { x: i64, y: i64, level: u8 }`，默认 tile 512²，level 即 mip。
- **禁止**出现"世界坐标 f32"类型；编译期由 `strata-geo` 的 newtype 把关（`WorldI64` / `LocalF32` 不互通）。这钉死 P4。

## 节点与执行器（P3/P8）

- 节点 = `id + kind + params + inputs`；图是纯数据（可序列化、可 diff、可撤销）。
- `strata-exec` 任务模型：
  1. 参数变更 ⇒ 该节点输出 tile **全 dirty**；结构性变更（连线/增删）⇒ 下游受影响子图 dirty。
  2. dirty 以 tile 粒度传播（节点声明空间依赖半径，如 erosion 迭代 N 步 ⇒ 半径 N），调度器只重算 dirty tile。
  3. 所有执行走异步队列（rayon CPU / wgpu compute），UI 线程永不等待；tile 到达即换显。
- **parity 规约**：每个空间节点同目录放 `foo.rs`（CPU）与 `foo.wgsl`，CI 跑同 seed 对比，量化 i16 后逐位一致（02 文档的容差定义）。这同时给了无 GPU 回退与 Python headless 路径。
- 内存预算制：`GpuTileCache`/`CpuTileCache` 各有显式 budget（默认 512MB，可下调），超限按 LRU + 降 level 淘汰 ⇒ 小内存机器（本机实况）是一等测试场景而非例外。

## 渲染管线（P5）

- 视角相关：camera-centered geometry clipmap（层数 = log2(view_range/tile)，默认 12 层）。
- 每层一个 draw call + skirt 条带封 T-junction 缝；每层独立 near/far（GPU Gems 2 方案）消 z-fighting。
- 高度来源：texture array 采样，顶点着色器取高 ⇒ 网格常驻、只有高度流式，切换视角零网格重建。
- 材质：三平面映射 + splat mask（由图里 MaskTile 节点喂入），M2 再做权重编辑刷。
- 编辑器大地形（非漫游）模式：分块网格直接画 + 离线金字塔，同一 clipmap 调度器退化处理（层 0 覆盖全图）。

## 撤销/重做与确定性

- 图是纯数据 ⇒ 撤销 = 状态机回退，不撤销模拟结果（结果由图 + seed 决定）。
- 全部生成节点吃显式 `seed: u64`；同图同 seed 同平台 ⇒ 输出逐位一致；跨平台一致范围 = i16 量化后（写进 06 的 release 判据）。

## 技术选型定论

| 面 | 选型 | 理由 / 否决项 |
|---|---|---|
| 图形 | **wgpu**（DX12/Vulkan/Metal/GL 四后端） | 纯 Rust、WGSL parity 友好；否决原生 glow+GL（macOS 只到 4.1） |
| 引擎框架 | **不用 Bevy**，自持 winit+wgpu 主循环 | Bevy ECS 对本项目收益低、版本 churn 高；`bevy_terrain`/`bevy_noised` 只作参考实现 |
| 噪声 | 自研核（FastNoiseLite 算法族移植），不引 `noise` crate | 需要 WGSL 双实现，crate 抽象反而碍事 |
| 并行 | rayon | 成熟、无 runtime 负担 |
| GUI | egui + wgpu（见 04） | 即时模式契合节点图/DCC；否决 Slint/iced（backdrop blur 无成熟件）、Tauri（内存重，本机伤不起） |
| 序列化 | 工程文件 rkyv/自研 bin + zip 容器；交换格式走公开标准 | 见 01 G4/H3 对策 |
| 物理（后期经 Python） | rapier3d 内核 + Python 场景描述 | 见 05 |
