# Snip 截屏工具架构方案

> 状态：核心闭环已实现（`examples/snip`，运行 `cargo run -p snip`）。Windows 端已在真机上
> 采集、离屏渲染并通过交互测试；macOS 与 Linux 后端只做过类型检查，尚未在真机上运行（§9）。
> 使用说明见 [examples/snip/README.md](../examples/snip/README.md)。

## 0. 已确定的决策

| 问题         | 决定                                                                                     |
| ------------ | ---------------------------------------------------------------------------------------- |
| 定位         | 对标 Snipaste 的示例应用：冻结屏幕 → 选区 → 标注 → 复制 / 保存 / 贴图，常驻托盘          |
| 覆盖层拓扑   | 每个 GPUI display 一个覆盖层窗口；选区属于开始拖动的那块屏幕，并被限制在其中             |
| 坐标真相     | 冻结帧的物理像素（虚拟桌面坐标）；逻辑像素只出现在 GPUI 窗口边界                         |
| 渲染         | 一份矢量 `Scene`，两个后端：GPU 预览（GPUI `PathBuilder`）与 CPU 导出（tiny-skia）        |
| 难以对齐的部分 | 文字、序号、马赛克走 CPU 光栅 tile，预览与导出显示同一份像素                             |
| 历史         | 快照式：`Arc<[Arc<Annotation>]>` 结构共享，撤销 / 重做不写逆操作                         |
| 剪贴板       | 应用侧 `ImageClipboard` 接口，用 arboard 实现（Windows 写 DIB，Linux 支持 X11 / Wayland） |
| 采集         | 各平台原生实现，统一在 `Capturer` trait 后面                                             |
| 托盘         | Windows / macOS 用 `tray-icon`；Linux 用 `ksni`（直接走 D-Bus，不依赖 GTK）              |
| 界面语言     | 仅英文                                                                                   |

## 1. 核心思想：纯数据的会话，两个渲染后端

```text
 热键 / 托盘 / `snip capture`
        │
        ▼
┌────────────────┐   后台线程   ┌──────────────────────────┐
│ capture        │ ───────────▶ │ CaptureSet               │
│ 平台原生采集    │              │ 每屏一帧 + 窗口 z 序快照  │
└────────────────┘              └────────────┬─────────────┘
                                             ▼
                         ┌──────────────────────────────────┐
  每屏一个 Overlay ────▶ │ CaptureSession（唯一状态拥有者）   │
  指针 / 按键转发         │  └ SessionState（纯数据状态机）    │
                         └──────────────┬───────────────────┘
                     预览：GPUI paths    │   导出：tiny-skia
                     + tile 精灵         ▼   + 同一份 tile
                                   Scene（Figure / Tile）
```

由此得到三个性质：

1. **交互规则全部可单元测试。** `session/machine.rs` 不依赖 GPUI：选区、手柄、绘制、
   选中移动、Esc 的层层后退都是对 `SessionState` 的方法调用。
2. **所见即所得。** 预览与导出共用 `scene/outline.rs` 产出的 `Figure`，以及
   `raster/tiles.rs` 产出的 tile；GPUI 不提供非测试的帧回读，所以导出必须在 CPU 上重画，
   而重画用的是同一份几何。
3. **像素精确。** 选区与标注都在物理像素空间里，混合 DPI 也不会重采样冻结帧。

## 2. 分层与目录（按能力组织）

```text
examples/snip/src/
  main.rs            CLI、单实例转发、启动 GPUI
  app.rs             Snip 全局状态：设置、采集器、当前会话、热键 / 托盘 / IPC 输入
  geometry/          纯：PhysPoint / PhysRect / DisplayArea、手柄、窗口吸附
  scene/             纯：Annotation / Shape / Style、outline（Figure）、hit、History
  capture/           Capturer trait、Frame、CaptureSet
    windows/         DXGI Desktop Duplication、HDR 映射、GDI 兜底、EnumWindows
    macos/           ScreenCaptureKit、CGWindowList
    linux/           X11 根窗口 / 窗口栈、Wayland 截图 portal
  raster/            tiny-skia：tile、compose、编码与文件名
  session/           machine（纯）、CaptureSession、overlay、toolbar、paint
  output/            交付：复制 / 保存 / 另存为 / 贴图；clipboard 接口
  pin/               贴图窗口
  settings_window.rs 设置窗口（gpui-component 的 setting 组件）
  shell/             cli、ipc、hotkey、settings、tray、hud、platform
  preview.rs         `--render-preview`（preview feature）：离屏渲染界面
```

## 3. 模型

| 类型 | 含义 |
| --- | --- |
| `Frame` | 一块屏幕的冻结帧：不透明 RGBA、`DisplayArea`（物理边界 + 缩放）、平台显示器 id |
| `CaptureSet` | 同一时刻的所有帧、窗口快照（前到后）、指针位置 |
| `WindowSnapshot` | 窗口可见边框及其子区域（Windows 的子窗口），用于自动选区 |
| `Annotation` | `AnnotationId` + `Shape` + 物理像素下的 `Style` |
| `Shape` | Rectangle / Ellipse / Arrow / Line / Pen / Marker / Mosaic / Text / Step |
| `Scene` / `History` | 标注快照与撤销 / 重做栈；`amend` 与 `discard` 服务于拖动手势 |
| `Figure` | 一条路径 + 填充或描边，两个后端都画它 |
| `Tile` | 直通 alpha 的像素块，放在整像素的桌面位置 |

标注坐标属于桌面而不是选区，因此画完后再调整选区，标注留在原处，导出时随图裁剪。

## 4. 会话与按键

状态机的阶段：Selecting（悬停高亮窗口，单击取窗口、拖动画框）→ Adjusting（手柄缩放、
拖动移动、方向键微调）⇄ Annotating（有工具在手）。按在已有标注上时，无论是否拿着工具，
都会选中它并可拖动；拖动整体是一个撤销步骤。

**Esc 自上而下：** 正在输入的文字 → 提交；进行中的手势 → 取消（移动被撤回、选区复原）；
选中的标注 → 取消选中；手中的工具 → 放下；最后才结束会话。右键：先放弃选区回到
Selecting，再右键退出。

| 键 | 作用 |
| --- | --- |
| Enter、Cmd/Ctrl-C、双击 | 复制并结束 |
| Cmd/Ctrl-S / Cmd/Ctrl-Shift-S | 保存到保存目录 / 另存为… |
| F3、Cmd/Ctrl-T | 贴图 |
| Cmd/Ctrl-Z / Cmd/Ctrl-Shift-Z（Ctrl-Y） | 撤销 / 重做 |
| 方向键 / Shift+方向键 | 选区移动 1 / 10 像素 |
| Cmd/Ctrl+方向键 | 从右下角伸缩选区 |
| Cmd/Ctrl-A | 选中指针所在整屏 |
| R E A L P M X T N | 矩形、椭圆、箭头、直线、画笔、马克笔、马赛克、文字、序号 |
| `[` `]` | 线宽 |
| Delete、Backspace | 删除选中的标注 |
| C / Shift-C | 复制放大镜下的颜色（HEX / RGB） |

每条命令只定义一次 Action，工具栏按钮、按键都派发它。文字输入框内，字母键用
`NoAction` 屏蔽，Enter 与 Esc 绑定为提交文字，避免穿透到"复制"。

## 5. 平台层

| | Windows | macOS | Linux X11 | Linux Wayland |
| --- | --- | --- | --- | --- |
| 采集 | DXGI Desktop Duplication；HDR 帧按 SDR 白电平映射到 sRGB；失败的输出走 GDI | ScreenCaptureKit，每屏按 F（最大缩放）采集 | 根窗口一帧（GPUI 把整个 X screen 当一个 display） | 截图 portal，一张整桌面图 |
| 窗口识别 | EnumWindows + DWM 可见边框 + 子窗口 | CGWindowList（第 0 层） | `_NET_CLIENT_LIST_STACKING` + 边框扩展 | 无，退化为整屏 |
| 覆盖层窗口 | PopUp（topmost 工具窗口） | PopUp（NSPanel） | 全屏 Normal 窗口 | 全屏 Normal 窗口 |
| 全局热键 | global-hotkey | global-hotkey | global-hotkey | 无，用桌面快捷键绑定 `snip capture` |
| 贴图置顶 | 是 | 是 | 否（Normal 窗口） | 否 |

显示器匹配：Windows 的 GPUI `DisplayId` 就是 `HMONITOR`，macOS 是 `CGDirectDisplayID`，
都按 id 匹配；其余按位置与尺寸匹配。Wayland 的单帧对单屏时，用两者宽度之比推出缩放。

## 6. 渲染

覆盖层只有一个 `canvas`，按层绘制：冻结帧 → 选区外的暗化 → 标注路径 → tile → 草稿 →
选区边框、手柄、选中标注的虚线框。GPUI 在同一 layer 内按图元种类排序（图片最后），
所以每一层都包在 `paint_layer` 里，否则暗化与手柄会被冻结帧盖住。

选区边框与手柄用主题的 `selection` 色：它表达的正是"选中的区域"，并且在深浅主题下
都能压住任意截图内容。暗化是对截图内容本身的处理，用固定的 45% 黑。

## 7. 验证方式

- `cargo test -p snip`：纯模块单元测试，加上 `session/tests.rs` 在 GPUI 测试平台上用假
  采集器、内存剪贴板走完整会话（拖选、单击取窗口、绘制、撤销重做、Esc 顺序、右键、
  复制结果的像素、贴图与关闭）。
- `cargo test -p snip -- --ignored`：在真机上采集并写出 PNG、列出窗口栈。
- `cargo run -p snip --features preview -- --render-preview <目录>`：真实采集后，在隐藏
  窗口里用合成输入走一遍会话，把覆盖层、贴图、设置窗口按深浅主题渲染成 PNG，屏幕上
  不出现任何窗口，也不碰用户的剪贴板。

## 8. 里程碑

| 里程碑 | 状态 |
| --- | --- |
| M1 几何、状态机、Windows 采集、覆盖层、选区、放大镜、复制 | 完成 |
| M2 场景、历史、GPU 绘制、工具栏与样式行 | 完成 |
| M3 CPU 合成、文字 / 马赛克 / 序号 tile、保存与另存为 | 完成 |
| M4 贴图：拖动、缩放、透明度、旋转、右键菜单 | 完成（缩放以左上角为锚点） |
| M5 托盘、热键、单实例与 CLI、设置窗口 | 完成 |
| M6 macOS / Linux 后端、标注选中 / 移动 / 删除 | 完成（非 Windows 后端未真机运行） |

## 9. 风险与已知限制

1. **macOS 与 Linux 未真机运行。** 两者只在隔离 crate 里对目标平台做过 `cargo check`。
   macOS 端假设 ScreenCaptureKit 返回 32 位 BGRA，且按请求尺寸出图；不符时报错而不是猜。
2. **跨屏选区。** Windows 与 macOS 每屏一个覆盖层，选区不能跨屏；X11 因为整个 screen
   是一个 display，反而可以。
3. **Wayland：** 仅支持单屏（portal 只给一张整桌面图）；没有全局热键、窗口识别和置顶贴图；
   覆盖层是全屏普通窗口而非 layer-shell。
4. **采集耗时。** Windows 调试构建下 4K HDR 一次约 0.5 秒（每次新建 D3D 设备与复制会话，
   加 HDR 映射）；发布构建未测量。常驻复制会话无法在画面静止时取到帧，所以每次新建。
5. **文字是单行的**，换行与多行编辑尚未实现；UI Automation 级别的控件识别也尚未实现。
6. **贴图缩放以左上角为锚点**：GPUI 没有设置窗口位置的 API，以指针为中心缩放需要各平台
   原生移动窗口。
7. **快捷键在设置里以文本输入**（如 `ctrl-shift-a`），不是按键录制器。
8. **离屏预览里设置窗口的侧栏页面列表为空**，内容区正常；是否只出现在隐藏窗口中尚未在
   屏幕上确认。
