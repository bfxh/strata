# 04 · UI 设计规范（iOS 26 Liquid Glass / 磨砂玻璃）

目标三约束：**磨砂玻璃视觉 + 方便易用 + 不能繁杂**。繁杂是竞品共性痛点（01 的 H1/W5/G3），本规范把它当硬门而不是愿景。

## 技术落地

- **egui + wgpu**，backdrop blur 用 `backdrop-blur-egui`（lib.rs 在维护的 GPU 磨砂层，真 vibrancy 而非假透明）；自绘不够时走 egui 自定义 `Shape`/paint callback 接 wgpu pass。
- 为什么不是别的：Slint/iced 无成熟 backdrop-blur 件；Tauri/web 栈内存重（本机内存是硬约束，01 W3 类 OOM 要避免）。
- 风险登记：egui 官方 issue #1758 承认 blur 需后处理 pass、复杂度高 ⇒ blur pass 由我们侧的 `strata-render` 统一出，UI 只是消费者；若上游不合并，自持 pass 是可接受成本。

## 玻璃分层规则（照 Apple 原则，适配桌面 DCC）

Apple 的核心口径：**玻璃只属于浮在内容之上的导航层，内容层不玻璃化**（WWDC25 "Meet Liquid Glass"；HIG 原文）。落成四条：

| 层 | 处理 | 例 |
|---|---|---|
| L0 内容 | 完全不透明、全饱和 | 3D 视口、预览缩略图 |
| L1 悬浮控件 | 磨砂玻璃（blur + 折射高光 + 边缘光） | 工具栏、浮动调色/预设盘、Toast |
| L2 面板 | 厚玻璃（更高 tint 不透明度 + 更强 blur） | 检视器、节点图、工程树 |
| L3 模态 | 玻璃 + 背景整体 dim | 导出对话框、确认框 |

- **prominence 三档**：regular / prominent / tinted（Apple 术语直接映射成我们的 token：`glass.regular.a=0.55`、`glass.prominent.a=0.75`、tint 仅限选中态与主操作）。数字是实现起点，最终值由视觉基线锁进 CI（06 golden image）。
- 明暗自适应：玻璃 tint 随系统外观切换；折射强度随内容亮度微调（Apple 的 lensing 行为）。

## 可读性硬底线（对 Apple 自身被批评点的反向防御）

uxdesign 对 iOS 26 的主要批评 = 玻璃透明度伤可读性/对比度。我们立三条可测门：
1. 玻璃层上文字与玻璃底色合成后对比度 **≥ 4.5:1**（WCAG AA），做进 golden image 的自动断言；
2. 视口内容高亮区域下，L1/L2 面板文字不得小于 body 13px 等效；
3. blur 半径、tint 是 token，不允许组件私调参数（防 design debt）。

## 交互架构：简单模式默认，节点图渐进

针对 P7（学习曲线）+ 用户"方便易用、不能繁杂"：

- **首屏即地形**：新建工程默认已有一块能转的示例地形（不做空画布）。
- **三段式界面**：视口（主）+ 底部抽屉（节点图/堆栈，默认折叠）+ 右侧检视器（跟随选中）。没有第四区域。
- **双视图同一真相**：
  - 简单模式 = **图层堆栈**（Photoshop 心智：生成/形态/侵蚀/材质四组，每组几个预设开关）；
  - 专家模式 = 节点图（Gaea/WM 心智）。
  - 堆栈操作在底层编译成 DAG，两视图可无损互切——繁简是同一模型的两种投影，不是两套系统。
- **渐进披露**：检视器默认 3–5 个"意图级"参数（如 山脉高度/湿度/侵蚀强度），其余收进 disclosure；hover 出滑条即时预览。
- **一句话即得**：预设盘（火山岛/喀斯特/丹霞/雪线/荒漠…）拖入即成图；每个预设是一个可展开的 DAG 模板。
- **全局可逆**：一切操作进撤销栈；破坏性操作（清图、重算全图）才弹 L3 确认。
- 快捷键覆盖一切面板操作；鼠标是可选路径而非唯一路径（DCC 老手口径）。

## 反繁杂清单（评审时逐条对照，出现即打回）

- [ ] 任何功能不出现"设置页里藏开关"的形态——设置只有外观（明暗/语言）和性能预算三类
- [ ] 同一意图只有一个入口（不允许菜单+工具栏+右键三处重复）
- [ ] 首启动 60 秒内产出第一张地形（可用性测试脚本，进 07 的 M3 验收）
- [ ] 面板层级 ≤2（悬浮控件和模态不算层级）
- [ ] 图标+文字并用，不裸图标

## 视觉基线

- 字体：系统默认（Windows Segoe UI / SF）；数字参数用等宽变体。
- 动效：玻璃层出现 = scale 0.96→1 + blur 半径收敛（Apple 的"液态"感），统一 180ms、spring 曲线；视口交互期间禁自动动画（防抖）。
- 全部 token（tint/blur/圆角/间距 8pt 栅格）落 `design-tokens.md`（M3 实现时生成），CI 检查组件不得硬编码色值。

Sources:
- [Liquid Glass — Apple Developer](https://developer.apple.com/documentation/technologyoverviews/liquid-glass)
- [Meet Liquid Glass — WWDC25 Session 219](https://developer.apple.com/videos/play/wwdc2025/219/)
- [iOS 26 Liquid Glass SwiftUI 参考（"玻璃保留给悬浮导航层"口径）](https://medium.com/@madebyluddy/overview-37b3685227aa)
- [Did Apple abandon its design heuristics?（可读性批评）](https://uxdesign.cc/did-apple-abandoned-its-own-design-heuristics-accessibility-principles-2d616ed7ace5)
- [backdrop-blur-egui](https://lib.rs/crates/backdrop-blur-egui)
- [egui #1758 — blur 需后处理 pass 的官方确认](https://github.com/emilk/egui/issues/1758)
