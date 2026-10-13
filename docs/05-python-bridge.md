# 05 · Python 桥（物理引擎 / 渲染引擎接入）

原则：**Rust 内核不依赖 Python 也能跑**（headless、发布版可裁剪）；Python 是扩展面，不是承重墙。针对 01 的 P6（往返脆、进程绑定死）——跨进程一律走文件与公开格式，不走内存级引擎绑定。

## 双向两种形态

### A. 嵌入式（app 内插件）

- `strata-py` crate 用 pyo3 在应用内嵌 CPython，独立 **plugin 线程**执行，GIL 由该线程独占持有。
- 插件 API（`import strata`）能力：
  - 读写工程 DAG（增删节点、连参数）；
  - **零拷贝取地形数据**：HeightTile → `memoryview`/numpy 数组（tile 粒度，不暴露全图，防大内存插件）；
  - 注册自定义节点（Python 实现，标记为非 parity 节点并在 UI 上标识性能/确定性风险）；
  - 订阅事件（选择变更、重算完成）。
- 崩溃隔离：插件未捕获异常只挂该插件并 UI 提示；死循环插件由看门狗线程超时 kill（subprocess 降级模式可选）。
- 版本策略：内嵌 CPython 3.12 独立构建（rust-python 静态链），不劫持用户系统 Python。

### B. 外置库（pip 包）

- maturin 把内核打成 `strata-py` wheel：`strata.generate(graph_json, seed) -> numpy`，可进 Jupyter/Blender/Houdini 脚本。
- 这解决 Gaea/WM 的导出黑箱（P6/G4）：**算法即库**，用户可在自己的 Python 环境复现工程输出（同 seed 同版本 → 断言逐位一致，进 06 CI）。

## 物理引擎接入

分层，避免"Python 拖住主循环"：

| 层 | 实现 | 说明 |
|---|---|---|
| 内核微物理 | rapier3d（纯 Rust，直接依赖） | 滚石、碎屑沉积、雪崩模拟作为**节点**（结果回写高度场/mask），无 Python、确定性可测 |
| 场景级物理 | Python 插件调 rapier 的 py 绑定或用户自带引擎（MuJoCo/PyBullet 经 pip） | 演示性玩法（倒水看侵蚀、推沙），产物以 mesh/粒子文件回注视口 |
| 刚体交互（后期） | 物理与地形碰撞走导出高度场文件 | 不做进程内双向实时耦合，避开 H3 那类跨引擎崩溃坑 |

内核节点走 rapier 的理由：Rust 原生、BSD、确定性行为可进 CI；Python 层只负责"玩法编排"，不碰承重计算。

## 渲染引擎接入

- **导出方向**（主路径）：USD（pxr Python 库生态）/ glTF / 16bit 高度场 → Blender、Houdini、Omniverse、UE 各自 Python 导入。Strata 不反向嵌这些引擎。
- **视口替代后端**（实验位）：`strata-render` 的设备抽象允许把离线渲染帧交给 Python 驱动的 renderer（如 path tracer 原型）——接口预留，默认不排期。
- WebGPU-python（wgpu-py）明确**不用**：双渲染栈心智成本大于收益。

## 边界与禁区

- Python 代码不得出现在帧循环、重算关键路径、undo 栈上。
- 插件 API 面只暴露 `strata.field.*`、`strata.graph.*`、`strata.events.*` 三命名空间，其余内核符号不发。
- 所有插件示例仓库放 `examples/py/`，CI 跑通每个示例（防 API 漂移）。
