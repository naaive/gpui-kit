# Launcher 架构方案

> 状态：M0–M2 与 M3 的大部分已实现（`examples/launcher`，运行 `cargo run -p launcher`）；
> 未实现的部分与已知限制见 §10、§12。定位为 GPUI Kit 的旗舰示例，架构按独立产品的标准
> 设计。使用与 SDK 参考见 [examples/launcher/README.md](../examples/launcher/README.md)；
> 引用的现有能力以 `crates/shell` 的源码与 [GPUI Shell](gpui-shell.md) 为准。

## 0. 已确定的决策

| 问题         | 决定                                                                 |
| ------------ | -------------------------------------------------------------------- |
| 定位         | 旗舰示例，放在 `examples/launcher`；平台先做好 macOS，其次 Linux     |
| 扩展界面     | 完全由宿主渲染：扩展只描述 List / Detail / Form，不绘制任何界面      |
| Raycast 兼容 | 不兼容；API 形状借鉴 Raycast，降低学习成本                           |
| 命令声明     | 静态写在 `launcher.json`，根搜索不执行扩展代码                       |
| VM           | 所有扩展共用一个 `ShellRuntime`，每个扩展持有自己的 `Policy`         |
| 第一步       | 端到端原型，先验证扩展路径，再做应用搜索与平台细节                   |
| SDK          | 分三层：`launcher`（页面节点）、`launcher/api`（宿主能力）、`launcher/utils`（纯 JS 辅助）；由参考扩展驱动（§6.6） |

## 1. 核心思想：一个模型，两个生产者

用户在启动器里看到的每一屏，都是同一种数据：**Page 模型**（List、Detail 或 Form）。

```text
          生产者                              消费者（只有一个）
┌────────────────────────┐
│ 内置页面（Rust）        │──┐
│ 根搜索、设置、系统命令  │  │     ┌──────────────┐     ┌─────────────────┐
└────────────────────────┘  ├──▶  │  PageModel   │ ──▶ │  宿主渲染器      │
┌────────────────────────┐  │     │  纯数据 + 操作 │     │  gpui-component │
│ 扩展页面（JavaScript）  │──┘     └──────────────┘     └─────────────────┘
│ gpui-shell 中的 View    │
└────────────────────────┘
```

由此得到三个性质：

1. **界面代码只写一次。** 根搜索本身就是一个内置的 List 页面，与扩展的 List 走同一个
   渲染器，所以扩展天然与内置命令一致，不需要额外的一致性规范。
2. **扩展边界是数据，不是界面。** 扩展能表达的就是 `PageModel` 能表达的，无法越界；
   这个边界可以脱离 GPU 用快照测试。
3. **行为大多是声明式的。** 打开链接、复制、粘贴、推入页面等常见操作以 `Effect`
   描述，由宿主执行，不必回到脚本；只有真正的业务逻辑才回调扩展。

## 2. 分层

```text
┌─────────────────────────────── examples/launcher ───────────────────────────────┐
│ shell      窗口 · 唤起/隐藏 · 全局快捷键 · 单实例 IPC                            │
├──────────────────────────────────────────────────────────────────────────────────┤
│ ui         SearchBar · ListView · DetailView · FormView · ActionPanel · Footer   │
│            只读 PageModel，不认识扩展                                            │
├──────────────────────────────────────────────────────────────────────────────────┤
│ session    Navigator（页面栈）· Selection · EffectRunner                         │
├───────────────────────────────┬──────────────────────────────────────────────────┤
│ model      PageModel · Item · Action · Effect（纯数据，无 GPUI 视图）              │
├───────────────────────────────┴──────────────────────────────────────────────────┤
│ pages      RootSearchPage · SettingsPage · ScriptPage（扩展页面的适配）           │
├──────────────────────────────────────────────────────────────────────────────────┤
│ search     Index · Matcher（含拼音）· Frecency       纯 Rust，无 UI               │
│ sources    Applications · System · Extensions（命令来源）                        │
│ extensions launcher.json · 安装与发现 · 权限 · 偏好 · gpui-shell 适配（bridge）   │
└──────────────────────────────────────────────────────────────────────────────────┘
        │                         │                         │
  gpui-component             gpui-shell（QuickJS）         gpui-kit
```

依赖只向下：`ui` 不知道扩展存在；`extensions` 不知道界面如何绘制；
`model` 和 `search` 不依赖任何视图，可以直接做单元测试。

### 2.1 目录

作为示例，先是一个 crate，内部按能力分模块（[Coding Guides](../website/docs/coding-guides.md)
「Organize large applications by capability」）；将来若独立成产品，`model`、`search`、
`extensions` 可以原样拆成 crate。

```text
examples/launcher/
├── src/
│   ├── main.rs         # 命令行 → 转交已运行的实例，或启动
│   ├── model/          # PageModel、Item、Action、Effect、Detail、Form、Image、Callback
│   ├── session/        # 页面栈、可见行（列表与网格）、选择
│   ├── pages/          # Page 接口、根搜索、ScriptPage
│   ├── ui/             # 窗口、列表/网格、Detail、Form、操作面板、页脚、Toast
│   ├── search/         # 模糊匹配、拼音、frecency
│   ├── sources/        # 应用（.app、.desktop、开始菜单）、系统命令、计算器、fallback
│   ├── extensions/     # 清单与目录、宿主（policy、权限、偏好、参数、生命周期）、Git 安装
│   │   ├── bridge/     # SDK：页面节点、launcher/api、launcher/utils、类型声明
│   │   └── pages/      # 权限、偏好、参数、扩展管理这些内置页面
│   └── shell/          # 单实例 IPC、全局快捷键、窗口显隐、深度链接、HUD、粘贴、设置
└── extensions/         # 参考扩展：gpui-kit、notes、emoji、github
```

## 3. 模型（`model`）

所有类型字段私有、以 builder 构造、以方法读取（CLAUDE.md「No `pub` fields on public
data types」）。下面只列出语义：

```text
PageModel
├── List   { items: [Section | Item], placeholder, loading, filtering, empty, detail_split }
├── Detail { markdown, metadata: [Label: Value], actions }
└── Form   { fields: [TextField | TextArea | Dropdown | Checkbox | DatePicker], actions }

Item   { id: ItemId, title, subtitle?, icon?, accessories: [Text | Tag | Date], keywords,
         detail?: Detail, actions: [Action] }

Action { id: ActionId, title, icon?, shortcut?, style: Default | Destructive, effect: Effect }

Effect
├── OpenUrl(url) · Open(path, application?) · Reveal(path)
├── Copy(text) · Paste(text)
├── Push(PageRequest) · Pop · PopToRoot · CloseWindow
├── ShowToast(style, title, message?) · ShowHud(text)
└── Run(CallbackRef)            ← 唯一回到扩展代码的 Effect
```

几条规则：

- **`ItemId` 必须稳定。** 选中状态由宿主按 `ItemId` 保存，扩展重新渲染（例如加载了
  更多结果）时，选中项不会跳走。
- **第一个 Action 是主操作（`Enter`），第二个是次要操作（`Cmd/Ctrl-Enter`）。**
  这是约定而不是字段，与 Raycast 相同，作者无需额外标注。
- **`Effect` 是宿主的权限边界的一部分。** `OpenUrl`、`Copy` 由宿主执行，扩展不需要
  `clipboard` 或 `execute` 权限；`Paste` 需要平台辅助功能权限，由宿主统一申请。
- `filtering` 为真时由宿主对条目做本地模糊过滤；为假时查询文本交给页面自己处理
  （例如远程搜索）。

## 4. 会话与渲染（`session`、`ui`）

```rust
/// 一个可以出现在导航栈中的页面。内置页面与扩展页面都实现它。
trait Page {
    fn model(&self, cx: &App) -> Rc<PageModel>;
    fn query_changed(&mut self, query: &str, cx: &mut Context<Self>);
    fn run(&mut self, callback: CallbackRef, window: &mut Window, cx: &mut Context<Self>);
}
```

- `Navigator` 持有页面栈与每页的查询文本、选中项；**根搜索是栈底的一个普通页面**。
- 窗口只有一个 `SearchBar`，属于宿主。切换页面时，它显示该页的占位符与查询文本。
- `ListView` 等视图只读当前页的 `PageModel`，把选择、滚动、快捷键全部留在宿主。
- `EffectRunner` 执行 `Effect`。`Run` 转交页面自身，其余全部在宿主完成。
- `Esc` 的顺序固定：关闭操作面板 → 清空查询 → `Pop` → 隐藏窗口。扩展不能拦截。

键盘：

| 按键                              | 行为                                    |
| --------------------------------- | --------------------------------------- |
| `↑` / `↓`、`Ctrl-P` / `Ctrl-N`    | 移动选中项                              |
| `Enter` / `Cmd-Enter`             | 主操作 / 次要操作                       |
| `Cmd-K`                           | 打开当前选中项的操作面板（可搜索）      |
| Action 上声明的快捷键             | 直接执行，并在操作面板中以 `Kbd` 显示   |
| `Tab`                             | 在命令参数之间移动                      |
| `Esc`                             | 见上                                    |

界面布局、状态与视觉规则见 §8。

## 5. 根搜索（`search`、`sources`）

```rust
trait CommandSource {
    fn commands(&self, cx: &App) -> Vec<Command>;   // 静态、可建立索引
}
```

- 来源：Applications、System（锁屏、设置、退出等）、Extensions（读取所有
  `launcher.json`，**不执行代码**）。
- 匹配：`nucleo-matcher`，对标题、别名、关键字、首字母缩写分别打分。
- 拼音：为中文标题预计算全拼与首字母，`wx` / `weixin` 都能匹配「微信」。
- 排序：匹配分与 frecency（指数衰减的使用记录）组合；并记住「查询 → 选中项」，
  让同一个查询的第一名保持稳定。
- 空查询时显示「最近使用」和「收藏」两个分组，不与搜索结果混排。
- 应用发现：macOS 读取 `/Applications`、`~/Applications`、`/System/Applications`
  下的 `.app`（`Info.plist` 名称、本地化名称、`.icns` 转 PNG 缓存）；Linux 读取 XDG
  `.desktop`。后台扫描，`notify` 监听增量更新。

`RootSearchPage` 实现 `Page`，把排序结果转换为 `PageModel::List`，
和扩展页面走同一套渲染。

## 6. 扩展

### 6.1 目录与清单

```text
github/
├── gpui-shell.json      # 运行时清单：id、name、entry、capabilities（权限）
├── launcher.json        # 启动器清单：命令、参数、偏好（贡献点）
├── commands/
│   ├── search-repos.js
│   └── create-issue.js
└── assets/icon.svg
```

```json
{
  "icon": "assets/icon.svg",
  "commands": [
    {
      "name": "search-repos",
      "title": "Search Repositories",
      "module": "commands/search-repos.js",
      "keywords": ["gh", "repo"],
      "arguments": [{ "name": "query", "placeholder": "Query", "required": false }]
    },
    { "name": "create-issue", "title": "Create Issue", "module": "commands/create-issue.js" }
  ],
  "preferences": [
    { "name": "token", "type": "password", "title": "Personal Access Token", "required": true }
  ]
}
```

- `gpui-shell.json` 管**权限**，`launcher.json` 管**贡献**。分成两个文件，
  是因为 `gpui-shell.json` 拒绝未知字段，且它属于运行时而不是某个宿主。
- 命令静态声明，是因为根搜索必须在不启动 VM 的前提下列出所有命令
  （与 GPUI Shell §18.2「Discovery executes nothing」同一要求）。
- 每个命令指向自己的模块，模块的默认导出就是该页面的 View，没有分发样板代码。
- 解析与校验沿用 `gpui-shell` 的风格：未知字段先报，错误指明字段路径与期望值；
  schema 由类型经 `schemars` 生成。

### 6.2 扩展作者看到的 API

两个模块：`launcher` 提供页面节点，`launcher/api` 提供命令上下文与少量命令式函数。
写法遵循 GPUI Shell 对已注册组件的约定：用 `new` 构造，绑定方法用 snake_case，
作者自己的方法用 camelCase。下面是 M0 已实现的 API（`Action.push` 与 `launch()`
的参数字段属于 M2）：

```js
import { View } from "gpui-kit";
import { Action, List, ListItem } from "launcher";
import { launch, show_toast } from "launcher/api";

export default class Checklist extends View {
  init() {
    this.command = launch().command; // 本次打开的命令，只在 init 中读取
    this.tasks = [{ id: "install", title: "Install Rust", done: false }];
  }

  toggle(task, cx) {
    task.done = !task.done;
    if (this.tasks.every((each) => each.done)) show_toast("Everything is done", "success");
    cx.notify(); // 请求宿主重新取模型
  }

  render() {
    return new List()
      .placeholder("Filter tasks…")
      .children(
        this.tasks.map((task) =>
          new ListItem(task.id, task.title) // id 跨渲染稳定，选中项才能跟随
            .action(new Action("Toggle").run((cx) => this.toggle(task, cx)))
            .action(new Action("Copy Title").shortcut("secondary-shift-c").copy(task.title)),
        ),
      );
  }
}
```

`render` 的返回值必须是 `List`、`Detail` 或 `Form` 之一；返回其他元素是错误，
宿主显示出错页面并指出这一点。扩展里没有颜色、间距或布局。

### 6.3 桥接：扩展如何产出 `PageModel`（`extensions/bridge`）

这是整个方案中最关键的机制，全部建立在 `gpui-shell` 已有的公开接口上，
没有修改 `gpui-shell`：

1. 启动器用 `ComponentRegistry` 注册 `List`、`ListSection`、`ListItem`、`Action`
   （与 `gpui-component-shell` 注册组件的方式相同），模块名为 `launcher`。参数由
   `ArgumentSchema` 校验，类型声明由同一份描述生成，二者不会漂移。
2. 这些节点的 materializer **不绘制任何东西**：它们把记录下来的方法调用重放成
   `Item`、`Section`、`Action`，装进一个只负责携带数据的元素（`Carrier`）交给父节点；
   `List` 最终携带整个 `ScriptModel`（`PageModel` 加上 `on_query_change` 回调）。
   这与 `gpui-component-shell` 中 `Menu` 把类型化子节点交给父节点是同一种模式。
3. **宿主直接渲染 `ScriptView`，不把它挂进窗口。** `ScriptPage::model` 在
   `ScriptView` 实体上调用 `render`，从返回的元素中取出 `ScriptModel`。模型是同步
   产生的，没有「发布—下一帧读取」的延迟，也不需要全局的发布通道。
4. **回调永远属于当前这一次渲染。** 模型只在脚本请求重绘（`cx.notify()` →
   `ScriptView` 通知 → `ScriptPage` 观察到后丢弃缓存）或 `ScriptView` 标记为脏时
   重建；重建后旧的回调随旧快照退役，宿主手中的回调始终来自最新快照。
   `ScriptView` 还会把上一代快照多保留一代，覆盖「事件落在两次重建之间」的情形
   （GPUI Shell §10.1）。
5. `render` 返回的不是 `List`，或脚本抛出异常时，页面变成 `PageModel::Failure`，
   由宿主显示原因。

结果是：扩展享有 `gpui-shell` 的全部运行时能力（View 状态、`cx.notify()`、异步、
热重载、沙箱、出错恢复），而界面完全由宿主绘制。

### 6.4 生命周期

1. 发现：读取 `gpui-shell.json` 与 `launcher.json`，建立命令索引，不执行代码。
   目录依次为 `launcher dev`、`LAUNCHER_EXTENSIONS`、设置中的目录、从 Git 安装的目录、
   随示例附带的目录；靠前的覆盖同 id 的扩展。
2. 打开命令时依次检查：权限（未决定时推入权限页）、必填偏好（推入偏好表单）、
   必填参数（推入参数表单）。每个页面回答后重新发起同一个 `LaunchRequest`，
   所以检查只写在 `ExtensionHost::open` 一处。
3. 每次打开都有自己的 `Policy`：`with_application(扩展 id)`、批准后的能力、
   独立的 `localStorage`，以及只属于这次打开的 `launcher/api` 与 `launcher/utils`
   （`Policy::with_host_module`，后者是 `HostModule::source` 提供的 JS 源码模块）。
   因此 `launch()` 在任何时候都只回答调用它的命令。
4. 以 `load_application_with_policy` + `mount_application` 挂载命令模块的 View，
   包装成 `ScriptPage` 压入页面栈；`no-view` 命令不压页面，在显示 HUD、关闭窗口或
   30 秒后释放。
5. `Action.push` 的回调返回一个新的 View 实例，`ComponentCallback::invoke_view`
   把它变成新的 `ScriptPage`，沿用同一个 policy。
6. 最后一个页面出栈后，这次打开保留 60 秒，便于立即再次进入；之后释放，
   其名下的调度任务一并取消（GPUI Shell §18.3）。

### 6.5 权限与偏好

- 权限完全沿用 `gpui-shell` 的能力模型（默认无权限）。启动器补上 GPUI Shell §18.4
  所缺的授权产品层：首次运行前显示权限单，决定保存在启动器配置里而不是扩展目录中，
  更新后新增的权限重新询问，`execute: "*"` 以警告级别单独展示。
- 偏好由 `launcher.json` 声明，由宿主用 `Form` 模型渲染（设置页也是一个内置 Form
  页面）；`password` 类型存入系统钥匙串（`keyring`），只通过 `launch()` 交给所属扩展。

### 6.6 SDK 路线

M0 只实现了证明架构所需的最小 SDK。完整的 SDK 分三层，边界与第 1 节的原则一致：
界面节点只产出模型，宿主能力按权限开放，纯 JS 辅助不含任何原生能力。

```text
launcher         页面节点 → PageModel（宿主渲染）
                 List/Section/Item/Dropdown · Detail · Form 与字段 · Grid · ActionPanel/Submenu
launcher/api     宿主能力（Rust 宿主模块，按权限开放）
                 context · feedback（Toast/HUD/confirm）· navigation · cache · clipboard
                 selection · oauth · commands（launch/updateMetadata）· environment
launcher/utils   纯 JS 辅助层
                 Query（加载/缓存/出错提示）· Paginator · FormState · Frecency 排序
```

- **反馈界面也归宿主**：Toast、确认框、HUD 都由宿主绘制，扩展只描述内容。
- **敏感能力在 `gpui-shell.json` 声明**：读取选中文字、读取剪贴板、AppleScript
  都走授权流程；复制、打开链接仍是宿主执行的 `Effect`，不需要权限。
- **没有 hooks，用对象代替**：GPUI Shell 的 View 是类，没有自动依赖追踪，
  因此提供面向类的辅助对象，例如 `this.repos = Query.new(this, () => fetch(...))`，
  负责加载状态、缓存和失败提示，并自动 `cx.notify()`。
- **命令模式**：`view`（默认，推入页面）与 `no-view`（后台执行后以 HUD/Toast 反馈，
  默认关闭窗口）；`menu-bar` 属于 M3。

每个 API 都要有使用者，由下面的参考扩展驱动：

| 参考扩展                                 | 驱动的能力                                                  |
| ---------------------------------------- | ----------------------------------------------------------- |
| GitHub（搜索仓库/PR，打开、复制、详情）   | OAuth、List 分页、筛选下拉、Detail、Query 缓存              |
| 翻译选中文字（no-view + 结果页）          | no-view、读取选中文字、可更新的 Toast、网络                 |
| 待办 / 快速笔记                           | Form 校验与草稿、本地存储、ActionPanel 二级菜单、确认删除   |
| 剪贴板历史或表情搜索                      | Grid、粘贴到前一个应用、Frecency 排序                       |
| 系统状态（M3）                            | 菜单栏命令、后台定时刷新                                    |

## 7. 窗口与唤起（`shell`）

- 一个常驻窗口，隐藏而非销毁；由 `gpui_kit::open_window` 创建。
- 无标题栏、居中、置顶、失焦即隐藏；尺寸约 `750 × 475`，以 `rem` 表达。
- 全局快捷键：GPUI 的 `KeyBinding` 只在应用获得焦点时生效，因此用 `global-hotkey`
  crate；Linux Wayland 退化为 Portal `GlobalShortcuts` 或 `launcher --toggle`。
- 单实例：本地 socket（Windows 上为命名管道）；承载 `toggle`、`show`、`hide`、
  `open <深度链接>`、`dev <dir>`；`types <dir>` 在本进程内写出类型声明。
- 窗口：macOS 与 Windows 用 `PopUp`、无标题栏；Linux X11 上 `PopUp` 是
  override-redirect 窗口、拿不到键盘焦点，所以用带客户端装饰的普通窗口。
  失焦即隐藏：macOS 用 `cx.hide()` 并把焦点还给之前的应用；GPUI 在其他平台无法隐藏
  窗口，因此关闭后下次再打开。macOS 上 Dock 图标通过 `objc2` 把激活策略设为 Accessory。
- 平台差异都在 `shell/platform/` 与 `sources/` 的 `cfg` 分支中，不向上泄漏。

## 8. 界面

```text
┌──────────────────────────────────────────────────────────┐
│ ‹  搜索应用和命令…                               [参数]    │  SearchBar
├──────────────────────────────────────────────────────────┤
│ 最近使用                                                   │
│ ▣ Visual Studio Code                            应用       │  ← 选中
│ ▣ Search Repositories        GitHub             命令       │
│ 收藏                                                       │
│ ▣ …                                                        │
├──────────────────────────────────────────────────────────┤
│ ▣ GitHub                           打开 ↵ │ 操作 ⌘K        │  Footer
└──────────────────────────────────────────────────────────┘
```

- 返回按钮只在页面栈非空时出现；带 `detail` 的 List 左右分栏。
- Footer 左侧是当前页面的来源或 Toast 状态，右侧只放主操作与 `Cmd-K`；不放与选中项
  无关的按钮（[Design Guides](../website/docs/design-guides.md)「Footer space is not a catch-all」）。
- 状态：加载中超过约 150 ms 才显示细进度条；无结果时提供「搜索网页」回退；
  扩展出错时显示脚本栈与「重试」；权限被拒绝时用 Toast 说明并提供「打开扩展设置」。
- 颜色、圆角、间距、字号全部来自主题 token，跟随系统浅色/深色；光标使用 `default`；
  动效只用于窗口出现/消失与页面推入/弹出，并遵守减少动态效果设置。
- 宿主界面文案走 `rust-i18n`（`en`、`zh-CN`、`zh-HK`）。

## 9. 原型结论（M0）

M0 实现了一条最窄的端到端路径：窗口 + 宿主搜索框 + 内置根页面 + JS 扩展返回的
`List` + 宿主执行的 `Effect` + `Action.run` 回调 + `Esc` 逐层返回。
`examples/launcher/src/ui/launcher_window.rs` 中的测试用真实的 JS 扩展和模拟按键
驱动整个窗口，覆盖了下表的前四个问题。

| # | 问题                                                   | 结论                                                                                                                                                  |
| - | ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1 | 扩展的描述能否变成宿主的模型，且不在渲染中重入          | 能。materializer 以 `Carrier` 逐级交出模型，宿主在自己的 `render` 中直接渲染 `ScriptView` 并取出结果，没有发布通道，也没有重入（§6.3）                |
| 2 | 不挂载的 `ScriptView` 能否响应 `cx.notify()`           | 能。不需要零尺寸挂载：`ScriptPage` 观察 `ScriptView` 实体，脚本 `cx.notify()` 后缓存失效，宿主重新取模型                                               |
| 3 | 重建模型后，宿主手中的回调是否总能调用成功              | 能。测试连续两次 `run` 同一个条目，第二次使用的是重建后的新回调；选中项按 id 跟随                                                                   |
| 4 | 能否用启动器自己的组件注册表，并以命令模块为入口创建 View | 能，且不修改 `gpui-shell`：`ShellRuntime::new_with_components` + `load_application(root, module)` + `mount_application`                               |
| 5 | 打开一个命令的延迟                                      | Linux 容器、release 构建、测试平台：加载并挂载一个命令约 5–10 ms（运行时在启动时已创建）。运行时创建本身与 macOS 上的数字尚未测量                      |

原型还暴露了两点，已处理：

- `InputState::set_value` 不发出 `Change` 事件，因此宿主改写搜索框（`Esc` 清空、
  切换页面恢复查询）时必须自己更新页面的查询，不能依赖输入事件。
- 扩展在运行时声明的快捷键无法注册为 GPUI 的 `KeyBinding`，改为在窗口的
  `on_key_down` 中与选中项的 action 匹配。

当时列出的三处 `gpui-shell` 缺口都已补上（均为通用的宿主嵌入接口，`gpui-base` 未修改）：

1. `ShellRuntime::load_application_with_policy`：以指定 policy 加载与挂载。
2. `ComponentCallback::invoke_view`：由脚本回调返回的 View 实例创建新的 `ScriptView`。
3. 调用者身份不需要新接口：per-policy 的 host module 优先于全局模块且不回退，
   每次打开命令各有一个 `launcher/api` 实例。另外新增 `HostModule::source`，用于提供
   JS 源码写成的模块（`launcher/utils`）。

## 10. 里程碑

| 阶段 | 内容                                                                                                   | 状态 / 完成标准                                      |
| ---- | ------------------------------------------------------------------------------------------------------ | ---------------------------------------------------- |
| M0   | §9 原型：`model`、`session`、`ui` 的 List、`bridge`、`launcher/api` 的 `launch`/`show_toast`、示例扩展 | **已完成**                                            |
| M1   | 根搜索：应用发现（macOS、Linux、Windows）、拼音、frecency、系统命令、计算器；全局快捷键、单实例与窗口行为；ActionPanel（`Cmd-K`） | **已完成**；macOS、Windows 分支未在真机验证            |
| M2   | SDK v1：Detail、Form、`no-view`、`Action.push`、偏好与钥匙串、权限单、`launcher/utils`、`launcher dev` 与 `launcher types` | **已完成**，热重载除外（见 §12）                      |
| M3   | 从 Git 安装、更新与卸载，扩展管理页，深度链接，Grid，fallback 命令                                      | **已完成**                                            |
| M4   | 对标 Raycast 的内置命令：剪贴板历史、文件搜索、快捷链接、片段（可选的输入时展开）、窗口管理、脚本命令（兼容 `@raycast.` 注释）、进程、书签、系统设置页；计算器的单位、百分比、进制、货币、时区与日期；根搜索的别名、收藏、全局快捷键与深度链接 | **已完成**；在 Windows 真机验证，窗口管理与片段展开仅 Windows |
| M5   | 补齐 Raycast 其余内置功能：窗口切换、Emoji 与符号、屏幕取色与颜色记录、悬浮笔记、日程（iCal 订阅与入会）、专注模式（屏蔽应用与网站）、截图搜索（OCR）、主题、设置导入导出、Hyper Key、`{selection}` 占位符；剪贴板按应用排除与图片 OCR；片段按应用禁用；窗口管理的循环尺寸、四等分、六等分、移动、缩放、间距与自定义布局 | **已完成**；系统相关部分仅 Windows（UI Automation、Windows.Media.Ocr、低级键鼠钩子） |
| M6   | 浏览器历史（Chromium 与 Firefox）与标签页切换、当前应用菜单项搜索、提醒事项（自然语言截止时间与到期提醒窗口）、计算器历史、翻译、查词（含中文与拼音）、系统监视器；文件预览的文件夹内容、系统缩略图与 Quick Look；紧凑窗口模式与“回到根搜索”的时机 | **已完成**；标签页、菜单项与缩略图仅 Windows |
| M7   | 扩展 SDK 对标 Raycast API：`menu-bar` 命令（`MenuBarExtra`，托盘图标与菜单，经 `tray-icon`）与 `interval` 后台定时运行（在不显示的后台窗口里加载，用户打开一次后启用）；`confirm_alert`、带按钮的 toast、`selected_text`/`selected_files`、前台应用与应用列表、OAuth（PKCE 与本机回环，令牌存系统钥匙串）、`launch_command` 的 `context` 与 deep link 的 `context`；`FilePicker`、`TagPicker`、带时间的 `DatePicker`、`FormSeparator`/`FormDescription`；网络图片、系统文件图标、图标色调与圆形裁剪、相对日期；`open_with`、`trash`、`quick_look`、`create_quicklink`/`create_snippet`、`pick_date`；`launcher dev` 热重载、`launcher new`/`lint`、`LAUNCHER_LOG` | **已完成**；托盘仅 Windows 与 macOS，`selected_files` 为 Explorer 与 Finder；第三方包沿用 GPUI Shell 的 Git `dependencies` |
| M8   | 扩展生态：Extension Store（GitHub 仓库存放，`index.json` 带每个文件的 SHA-256，下载校验后安装，可更新与卸载，来源可在设置中改为其他仓库或本地文件夹；`launcher store-index`）；首批商店扩展（开发者工具、网页搜索、网络工具、包搜索、最近项目、Docker、Obsidian、Bitwarden、Todoist、Spotify、Linear、Notion）与内置 GitHub 扩展的 PR、Issue、通知与托盘未读数；SDK 补充 `sql_query`、`fs` 授权的 `${homeDir}`/`${configDir}`、OAuth 固定回调端口 | **已完成** |

## 11. 测试

- `search`：打分、拼音、缩写、frecency 衰减与排序稳定性的单元测试。
- `model` / `bridge`：示例扩展渲染出的模型，经真实窗口与模拟按键验证（已实现）。
- `extensions`：`launcher.json` 的错误信息；权限决定到 grant 的映射；卸载时任务被取消。
- `session`：GPUI 交互测试覆盖键盘路径与 `Esc` 的逐层行为；扩展重新渲染后选中项保持不变。
- 应用发现：用临时目录中的伪 `.app` / `.desktop` 测试解析，不依赖真实系统。

## 12. 取舍与风险

1. **表现权收归宿主**，与 GPUI Shell「表现权归脚本」的原则相反。这是有意为之：
   对启动器而言，一致的键盘行为和外观就是产品本身。代价是扩展只能在 List、Detail、
   Form 中选择；需要新形态（如 Grid）时，由宿主扩充 `PageModel`，而不是开放绘制。
2. **不兼容 Raycast**，因此没有现成生态；换来的是没有构建步骤、没有 Node 依赖，
   扩展保存即生效。
3. **共享一个 VM**：单个扩展的内存失控会影响其他扩展（上限 256 MiB，GPUI Shell §19.3）。
   M2 时测量，必要时改为每个扩展一个隔离 runtime，`Page` 接口不受影响。
4. **Wayland** 上全局快捷键与置顶窗口依赖合成器，只能提供降级体验：需要在桌面的
   键盘设置里把快捷键绑定到 `launcher toggle`。
5. **平台代码的验证范围。** macOS 与 Windows 分支（`.app`/开始菜单扫描的入口、Dock
   图标、`PopUp` 焦点、粘贴、命名管道、系统命令）在 Linux 容器中无法编译；其中能写成
   纯逻辑的部分（`Info.plist`、`.strings`、`.icns`、`.lnk` 解析）在 Linux 上有测试。
6. **已知限制。** 窗口总在主显示器打开（GPUI 不提供光标所在显示器）；Linux 与 Windows
   上窗口失焦会关闭，打开中的页面随之丢失；`launcher dev` 不做热重载（需要为每次打开
   接入 GPUI Shell 的 `Watcher`）；无钥匙串时密码偏好存入仅本人可读、但未加密的
   `secrets.json`。
