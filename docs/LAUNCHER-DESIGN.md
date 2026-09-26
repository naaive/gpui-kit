# Launcher：基于 GPUI Kit 的可扩展应用启动器设计

> 状态：设计草案，尚未实现。本文所引用的现有能力均以
> `crates/shell`、`crates/component`、`crates/component-shell` 的当前源码与
> [GPUI Shell](gpui-shell.md) 为准；标注"待验证"的条目需要在实现前用代码确认。

## 1. 目标

做一个类似 Raycast 的桌面启动器：按下全局快捷键，屏幕中央出现一个搜索窗口，
用户输入几个字符即可打开应用、执行命令，或进入由扩展（Extension）提供的界面，
全程只用键盘。

1. **一个核心任务**：输入 → 找到 → 执行，从唤起到执行在 1 秒内完成。
2. **扩展由 JavaScript 编写**，运行在 `gpui-shell` 的 QuickJS 沙箱中；扩展的界面
   由宿主以原生 GPUI 组件渲染，外观和键盘行为与内置命令一致。
3. **跨平台**：macOS、Windows、Linux（X11 优先，Wayland 见 §11）。
4. **不修改 `gpui-base`**；对 `gpui-shell` 的需求优先用其现有公开接口满足，
   确需补充的列在 §10。

### 非目标（第一阶段）

- 扩展商店、在线分发与签名；
- 原生（Rust 动态库）插件：沿用 GPUI Shell §3 的立场，不 `dlopen`；
- Node.js / React 兼容层：扩展 API 不追求与 `@raycast/api` 源码兼容；
- AI 对话、剪贴板历史、窗口管理等 Raycast 内置产品功能（可作为后续内置命令）。

## 2. 从任务出发

按 [Design Guides](../website/docs/design-guides.md)「Start from the task」逐项回答：

| 问题               | 回答                                                                                                     |
| ------------------ | -------------------------------------------------------------------------------------------------------- |
| 主要任务           | 用关键字定位一个对象（应用、命令、扩展结果）并执行它的主操作                                             |
| 被查看或改变的对象 | 根搜索中的 **Command**（应用也是一种 Command）；进入扩展后是扩展列表中的 **Item**                        |
| 必须随时可用的操作 | 主操作（`Enter`）、操作面板（`Cmd/Ctrl-K`）、返回（`Esc`）                                               |
| 决策所需信息       | 标题、副标题（来源/路径）、图标、类别，必要时右侧 Detail 预览                                            |
| 需要设计的状态     | 空查询（最近使用）、无结果、加载中、扩展出错、权限被拒绝、离线、扩展正在更新                             |
| 键盘路径           | 唤起 → 输入 → `↑/↓`（`Ctrl-N/P`）选择 → `Enter` 执行 / `Cmd-K` 选其他操作 → `Esc` 逐层返回，最终隐藏窗口 |

## 3. 总体架构

```text
┌──────────────────────── launcher（应用） ────────────────────────┐
│  app shell：窗口 · 全局快捷键 · 单实例/IPC · 托盘 · 组合各功能      │
│                                                                    │
│  search        applications       extensions        settings       │
│  CommandSource  平台应用发现/启动   安装/发现/授权/运行   偏好与存储   │
│  排序与 frecency                   launch 上下文                      │
│                                        │                            │
│                           launcher-shell（适配层）                  │
│              注册 List/Detail/Form/ActionPanel 组件与 `launcher` 模块 │
└────────────────────────────────────────┼───────────────────────────┘
                                         ▼
   gpui-component ◀── gpui-component-shell ──▶ gpui-shell（QuickJS、Policy、PluginManager）
          │                                           │
          └──────────────── gpui-base ◀───────────────┘
                               │
                          gpui / platform
```

职责划分遵循 GPUI Shell 的原则「宿主提供能力，脚本负责组合」，但在启动器这个
嵌入场景下，**宿主同时拥有产品的视觉系统**：

- **Rust 宿主拥有**：窗口、搜索框、根搜索、排序、键盘导航、操作面板、导航栈、
  权限与偏好界面、所有平台能力。
- **扩展拥有**：自己的数据、业务逻辑、条目与操作的内容；通过宿主注册的组件
  描述界面，而不是自己绘制。
- 这相当于 Raycast 的模型（React 组件由宿主原生渲染），也与
  `gpui-component-shell` 的思路一致：组件由 Rust 适配层注册进
  `ComponentRegistry`，脚本只写描述。

### 3.1 代码组织

按 [Coding Guides](../website/docs/coding-guides.md)「Organize large applications
by capability」按能力拆分，而不是按 `models/`、`views/` 分目录：

```text
apps/launcher/
├── crates/
│   ├── app/              # 二进制：窗口、全局快捷键、单实例、托盘，组合各功能
│   ├── search/           # CommandSource trait、索引、模糊匹配、frecency；纯 Rust，无 UI
│   ├── applications/     # 平台应用发现、图标提取、启动；平台差异封装在此
│   ├── extensions/       # launcher.json、安装、发现、授权、运行会话
│   ├── launcher-shell/   # gpui-shell 适配层：组件注册、`launcher` 宿主模块、d.ts
│   └── settings/         # 偏好设置视图与持久化
└── extensions/           # 随应用附带的示例扩展（JS）
```

第一版可以只有 `app`、`search`、`launcher-shell` 三个 crate，其余先作为 `app`
内的模块，等边界稳定后再拆。`search` 和 `launcher-shell` 的公共边界最清晰，
应最先独立。

依赖：`gpui-kit`、`gpui-component`、`gpui-shell`、`gpui-component-shell`。
`gpui-shell` 尚未发布，在工作区内以 path 依赖使用即可。

## 4. 窗口与唤起

### 4.1 窗口

- 一个常驻的主窗口，隐藏而非销毁，保证再次唤起时无需重建视图和 VM。
- 通过 `gpui_kit::open_window` 创建（Root 为 `gpui_base::Root`，对话框、
  Toast、Tooltip 等覆盖层自动挂载）。
- 无标题栏、居中、置顶、不可调整大小；默认尺寸约 `750 × 475`（以 `rem` 表达，
  随基础字号缩放）。
- 失去焦点时隐藏（订阅窗口激活状态）；macOS 上隐藏后把焦点还给之前的应用。
- 背景使用主题 token，可选毛玻璃（`WindowBackgroundAppearance::Blurred`，
  各平台效果不同，需提供不透明回退）。

待验证（GPUI 当前版本）：

- `WindowKind::PopUp` / `Floating` 在三平台上的层级与全屏空间行为；
- macOS 上以 accessory 激活策略运行（不显示 Dock 图标）的接口是否暴露；
- 隐藏窗口后再次 `activate` 的焦点恢复。

以上若 GPUI 未提供，封装在 `app` 内的平台适配模块中，不向上泄漏。

### 4.2 全局快捷键

GPUI 的 `KeyBinding` 只在应用获得焦点时生效，无法用于全局唤起。方案：

- macOS / Windows / X11：使用 `global-hotkey` crate（与 Tauri 同源），
  在后台线程接收事件，通过 `AsyncApp` 切回主线程执行 `Toggle`。
- Wayland：没有通用的全局快捷键协议。优先尝试 XDG Desktop Portal 的
  `GlobalShortcuts`；不可用时，引导用户在桌面环境中把快捷键绑定到
  `launcher --toggle`，由单实例 IPC 转发给正在运行的进程。

### 4.3 单实例与 IPC

启动时通过本地 socket（Unix domain socket / Windows named pipe）检测已有实例。
第二个进程只发送命令后退出。IPC 同时承载：

- `--toggle`、`--show`；
- 深度链接 `launcher://extensions/<extension-id>/<command>?arguments=...`；
- 开发模式下 `launcher dev <dir>` 注册本地扩展。

## 5. 根搜索

### 5.1 数据模型

```rust
/// 根搜索中可执行的一个对象。
pub struct Command { /* 私有字段，builder 构造，方法读取 */ }

/// 提供 Command 的来源：应用、内置命令、扩展命令、快捷链接……
pub trait CommandSource: 'static {
    fn id(&self) -> SourceId;
    /// 静态条目：可预先建立索引。
    fn commands(&self, cx: &App) -> Vec<Command>;
}
```

`Command` 按项目约定不暴露 `pub` 字段（CLAUDE.md「No `pub` fields on public data
types」）。其内容：稳定 ID（`<source>/<name>`）、标题、副标题、图标、关键字、
别名、类别（Application / Command / Extension）、主操作与次要操作列表。

内置来源：

| 来源            | 内容                                                     |
| --------------- | -------------------------------------------------------- |
| Applications    | 已安装应用（§6）                                         |
| System          | 锁屏、睡眠、清空废纸篓、退出 Launcher、打开设置等         |
| Extensions      | 每个已安装扩展在 `launcher.json` 中声明的命令（§7.2）     |
| Quicklinks      | 用户定义的带参数 URL / 路径                              |
| Calculator      | 查询可解析为表达式时，在列表顶部给出一条结果（动态来源） |

动态来源（计算器、未来的扩展 fallback）不进入静态索引，另以
`fn results(&self, query: &str) -> Vec<Command>` 在每次输入时查询，并有时间预算。

### 5.2 匹配与排序

- 模糊匹配：使用 `nucleo-matcher`（Helix 使用的实现），对标题、别名、关键字分别打分，
  取加权最大值；首字母缩写（`vsc` → Visual Studio Code）单独加分。
- 中文：为标题预计算拼音全拼与首字母（`pinyin` crate），`wx` / `weixin` 都能匹配「微信」。
  这对中文用户是基本要求，不是锦上添花。
- Frecency：记录每个 Command 的使用时间戳，按指数衰减计算分数，与匹配分数组合。
  同时记录「查询前缀 → 选中项」的映射，使同一个查询的第一名稳定。
- 空查询：显示「最近使用」与「收藏」两个分组，而不是把所有条目混在一起
  （Design Guides：Search results、recent items、favorites 是不同集合，分别标注）。
- 整个 `search` crate 不依赖 UI，匹配与排序逻辑有单元测试覆盖。

### 5.3 数据持久化

用户数据目录下（macOS `~/Library/Application Support/<app>`，Linux
`$XDG_DATA_HOME/<app>`，Windows `%APPDATA%\<app>`）：

- `usage.json`：frecency 数据；
- `settings.json`：快捷键、主题、各扩展偏好（非敏感部分）；
- `extensions/<id>/`：已安装扩展；扩展自身的数据仍由 `gpui-shell` 放在
  `<data-home>/gpui-shell/plugins/<id>`，不混用。

## 6. 应用发现与启动

`applications` crate 定义平台无关的接口，平台实现放在 `cfg` 分支中：

| 平台    | 发现                                                                                   | 启动                                         | 图标                                         |
| ------- | -------------------------------------------------------------------------------------- | -------------------------------------------- | -------------------------------------------- |
| macOS   | `/Applications`、`~/Applications`、`/System/Applications` 下的 `.app`；读取 `Info.plist` 的显示名 | `NSWorkspace` / `open -a`                    | `.icns` 转 PNG，按 bundle 版本缓存           |
| Linux   | `$XDG_DATA_DIRS/applications` 与 `~/.local/share/applications` 的 `.desktop`，遵守 `NoDisplay`、`OnlyShowIn` | 解析 `Exec` 字段代码后启动，或 `gtk-launch` | 按 freedesktop 图标主题规范查找              |
| Windows | 开始菜单中的 `.lnk`（用户与公共），后续加入 UWP 应用                                  | `ShellExecuteW`                              | `SHGetFileInfo` 提取后缓存                   |

- 首次启动在后台线程扫描，完成后替换索引；之后用 `notify` 监听目录变化增量更新。
- 本地化应用名（如 macOS 上的「系统设置」）同时索引本地化名与原名。
- 次要操作：在访达/文件管理器中显示、复制路径、退出应用（仅在运行中时出现）。

## 7. 扩展（Extension）

### 7.1 用词

对用户与扩展作者统一使用 **Extension（扩展）** 与 **Command（命令）**：
一个扩展包含一个或多个命令。底层仍是 `gpui-shell` 的 Plugin，但这个词不出现在
启动器的界面和文档中。

### 7.2 目录与清单

```text
github/
├── gpui-shell.json      # gpui-shell 清单：id、name、entry、capabilities（权限）
├── launcher.json        # 启动器清单：命令、参数、偏好（贡献点）
├── main.js              # 入口：只做分发
├── commands/
│   ├── search-repos.js
│   └── create-issue.js
└── assets/icon.svg
```

`launcher.json`：

```json
{
  "icon": "assets/icon.svg",
  "commands": [
    {
      "name": "search-repos",
      "title": "Search Repositories",
      "subtitle": "GitHub",
      "mode": "view",
      "keywords": ["gh", "repo"],
      "arguments": [{ "name": "query", "type": "text", "placeholder": "Query", "required": false }]
    },
    { "name": "create-issue", "title": "Create Issue", "mode": "view" },
    { "name": "open-notifications", "title": "Open Notifications", "mode": "no-view" }
  ],
  "preferences": [
    { "name": "token", "type": "password", "title": "Personal Access Token", "required": true }
  ]
}
```

**为什么命令要在清单中声明，而不是由脚本注册。** GPUI Shell §18.1 的原则是
「权限属于数据，贡献属于代码」。但启动器的根搜索必须在**不执行任何扩展代码**的情况下
列出所有命令：三十个扩展不能为了出现在搜索结果里而启动三十个 VM，这与 §18.2
「Discovery executes nothing」是同一个要求。因此命令列表是启动器的数据，
行为仍在代码中。

**为什么是单独的文件。** `gpui-shell.json` 拒绝未知字段（§18.1），且它属于运行时
而不是某个宿主。把启动器的贡献点放在 `launcher.json` 中，不需要修改 `gpui-shell`
的清单格式，两者的 schema 各自由类型生成（`schemars`），互不干扰。

`launcher.json` 的校验沿用 `gpui-shell` 的风格：未知字段先于缺失字段报告，
错误指明字段路径与期望值；`name` 与 `id` 采用相同的字符集规则。

### 7.3 命令模式

| `mode`     | 行为                                                                                     |
| ---------- | ---------------------------------------------------------------------------------------- |
| `view`     | 推入一个由扩展描述的视图（List / Detail / Form）                                          |
| `no-view`  | 在后台执行，完成后通过 HUD 或 Toast 反馈，默认关闭窗口                                    |
| `menu-bar` | （第三阶段）在系统菜单栏常驻，按间隔刷新                                                 |

### 7.4 运行模型

命令被执行时：

1. `extensions` 在 `PluginManager` 中查找扩展；若未加载，调用 `load` 并在授权回调中
   应用已持久化的权限决定（§8）。
2. 宿主写入本次运行的 **launch 上下文**：命令名、参数、偏好值、启动来源。
3. 扩展入口读取上下文，按需动态 `import()` 对应命令模块。入口只负责分发，
   这正是 GPUI Shell §18.2 推荐的「entry 只注册，实现延迟加载」模式：

```js
// main.js
import { launch } from "launcher";

const { command } = launch();
const module = await import(`./commands/${command}.js`);
export default module.default;
```

```js
// commands/search-repos.js
import { launch, List, ActionPanel, Action, showToast } from "launcher";

export default function SearchRepos(cx) {
  const { arguments: args, preferences } = launch();
  // ... 用 fetch（受 capabilities.network 约束）请求数据，存入 entity 状态
  return List.new("repos")
    .searchBarPlaceholder("Search repositories")
    .onSearchTextChange((text, cx) => search(text, cx))
    .isLoading(state.loading)
    .children(state.repos.map((repo) =>
      List.Item.new(repo.id)
        .title(repo.full_name)
        .subtitle(repo.description)
        .accessory(`★ ${repo.stars}`)
        .actions(
          ActionPanel.new()
            .child(Action.openInBrowser(repo.html_url))
            .child(Action.copyToClipboard(repo.clone_url).title("Copy Clone URL")),
        ),
    ));
}
```

这条路径只使用 `gpui-shell` 已有的公开能力：宿主模块（`export_module`，返回纯数据）
提供 `launch()`；`Plugin::view()` 返回的 `Entity<ScriptView>` 可以直接嵌入宿主视图；
组件回调由 `ComponentRegistry` 的 `ComponentCallback` 调回脚本。

**VM 粒度。** 所有扩展共享一个 `ShellRuntime`，每个扩展持有自己的 `Policy`
（GPUI Shell §18.3 已保证两个插件同时加载时权限互不可见）。一个命令关闭后，
其扩展保持加载一段时间（默认 60 秒）以便快速再次进入，之后 `unload`，
取消该扩展名下所有调度任务。若测量表明共享 VM 的内存上限（256 MiB，§19.3）
对多个扩展过紧，再改为每个扩展一个 `new_isolated_runtime`。

### 7.5 扩展界面组件

由 `launcher-shell` 注册到 `ComponentRegistry`，以模块 `"launcher"` 暴露：

| 组件                | 用途                                                                                     | 基于                                   |
| ------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------- |
| `List` / `List.Item` / `List.Section` | 主要形态：可搜索、分组、支持右侧 `List.Item.Detail` 预览                        | `gpui-component` 的 list / virtual list |
| `Grid`              | 图片类结果（图标、表情、壁纸）；第二阶段                                                  | virtual list                           |
| `Detail`            | 单个对象的详情：Markdown 正文 + 元数据                                                   | `TextView` / markdown、`DescriptionList` |
| `Form` 与字段       | 收集输入：TextField、TextArea、Dropdown、Checkbox、DatePicker                            | `Form`、`Input`、`Select`……            |
| `ActionPanel` / `Action` | 条目的操作集合；第一个是主操作（`Enter`），第二个是次要操作（`Cmd/Ctrl-Enter`）     | `Menu` / `Command`                     |
| `EmptyView`         | 自定义空状态                                                                             | `Empty`                                |

内置 Action：`openInBrowser`、`open`（文件/应用）、`copyToClipboard`、`paste`（粘贴到
前一个应用，需辅助功能权限）、`push`（推入新视图）、`pop`、`submitForm`，
以及带回调的通用 `Action.new(title).onAction(...)`。

关键设计：**搜索框属于宿主，不属于扩展。** 窗口顶部永远只有一个输入框；进入扩展的
List 后，输入框的占位符和内容切换为该 List 的，文本变化通过 `onSearchTextChange`
回调给脚本（或由宿主在 `filtering(true)` 时直接本地过滤）。同样，底部操作栏与
`Cmd-K` 操作面板也由宿主绘制，内容来自当前选中条目的 `ActionPanel`。
这样扩展之间、扩展与内置命令之间的键盘行为完全一致。

为此适配层维护一个宿主侧的 `LauncherSession` 实体：导航栈、当前查询、当前选中项的
操作列表。适配层的组件在 materialize 时读写它；脚本只看到组件与回调。

**逃生口。** 需要完全自定义界面的扩展（颜色选择器、图表）可以在 `Detail` 中放入
`gpui-component-shell` 已绑定的任意组件。它们仍使用主题 token，因此不会破坏整体外观，
但失去统一的键盘约定，文档中应将其定位为例外。

### 7.6 `launcher` 宿主模块的其他函数

均为纯数据接口，不持有脚本句柄（GPUI Shell §17.6）：

- `launch()`：命令名、参数、偏好、启动来源；
- `closeMainWindow({ clearRootSearch })`、`popToRoot()`；
- `showHUD(text)`：窗口关闭后在屏幕上短暂显示的提示；
- `showToast({ style, title, message })`：窗口内反馈，映射到 `ShellRoot` / `Root` 的 Toast；
- `open(target, application?)`：经宿主打开 URL、文件或应用，不需要 `execute` 权限；
- `getSelectedText()`、`getFrontmostApplication()`：第二阶段，需要平台权限；
- `environment()`：启动器版本、外观（light/dark）、语言、是否开发模式。

`HostModule::declarations` 同时生成 `launcher.d.ts`，与注册表的实际导出由
`validate` 校验一致。

## 8. 权限与安全

扩展的系统访问完全沿用 `gpui-shell` 的能力模型：默认无权限，`fs`、`network`、
`process`、`clipboard`、`storage` 都来自 `gpui-shell.json` 的 `capabilities`。
启动器补上 GPUI Shell §18.4 所列「尚未构建」的授权产品层：

1. **安装时**展示权限单（Dialog）：列出网络主机、可读写目录、可执行命令；
   `execute: "*"` 以警告级别单独显示。
2. 用户的决定保存在启动器配置中，而不是扩展目录里（扩展不能自己改写授权）。
3. `PluginManager::load` 的授权回调读取该决定，只把批准的部分变成 grant。
4. 更新后若请求了新权限，再次询问；拒绝时继续以旧的权限集运行，受限调用失败时
   显示 `gpui-shell` 自带的、指明清单字段的错误说明。
5. `password` 类型的偏好保存在系统钥匙串（`keyring` crate），只通过 `launch()`
   交给所属扩展，不写入 `settings.json`。

沙箱本身（冻结原型、禁止 `eval`、模块路径限制、中断与内存上限）由 `gpui-shell`
提供，启动器不重复实现。

## 9. 交互与界面

### 9.1 布局

```text
┌──────────────────────────────────────────────────────────┐
│ ‹  搜索应用和命令…                                   [参数] │  ← 宿主输入框
├──────────────────────────────────────────────────────────┤
│ 最近使用                                                   │
│ ▣ Visual Studio Code        应用                          │  ← 选中项
│ ▣ Search Repositories       GitHub          命令           │
│ 建议                                                       │
│ ▣ …                                                        │
├──────────────────────────────────────────────────────────┤
│ ▣ GitHub                         打开 ↵  │  操作 ⌘K        │  ← 宿主操作栏
└──────────────────────────────────────────────────────────┘
```

- 顶部：返回按钮只在导航栈非空时出现；命令带参数时，在输入框右侧以内联字段显示，
  `Tab` 在参数间移动。
- 中部：列表按集合分组并标注；带 Detail 的 List 采用左右分栏。
- 底部：左侧是当前上下文（扩展图标与名称、或 Toast 状态）；右侧是主操作与
  `Cmd-K` 提示。底部每一项都必须回答「作用对象是什么、为何此时可用」，
  不放与当前选中项无关的按钮（Design Guides「Footer space is not a catch-all」）。

### 9.2 键盘

| 按键                       | 行为                                                         |
| -------------------------- | ------------------------------------------------------------ |
| `↑` / `↓`、`Ctrl-P` / `Ctrl-N` | 移动选中项                                                    |
| `Enter`                    | 主操作                                                       |
| `Cmd/Ctrl-Enter`           | 次要操作                                                     |
| `Cmd/Ctrl-K`               | 打开操作面板（可搜索的菜单），`Esc` 关闭                      |
| 操作自带快捷键             | 直接触发；在面板中以 `Kbd` 显示                               |
| `Esc`                      | 依次：关闭最上层覆盖层 → 清空查询 → 返回上一视图 → 隐藏窗口 |
| `Cmd/Ctrl-,`               | 打开设置                                                     |

`Esc` 的逐层行为与 Design Guides「Escape should dismiss the topmost dismissible layer」
一致，由 `LauncherSession` 的导航栈统一实现，扩展不能拦截最后一层。

### 9.3 状态

| 状态       | 表现                                                                         |
| ---------- | ---------------------------------------------------------------------------- |
| 加载中     | 输入框下方细进度条；超过约 150 ms 才显示，避免闪烁                           |
| 无结果     | 列表区显示「没有匹配的结果」，并提供以当前查询搜索网页的回退命令             |
| 扩展出错   | 列表区显示 `gpui-shell` 的 failure surface 与脚本栈；操作：重试、查看日志   |
| 权限被拒绝 | Toast 说明被拒绝的能力，操作：打开扩展设置                                   |
| 缺少必填偏好 | 进入命令前先显示该扩展的偏好表单，填写完成后继续执行                       |

### 9.4 视觉

- 全部颜色、圆角、间距、字号来自主题 token；支持浅色/深色，跟随系统。
- 列表行高、图标尺寸、分组标题遵循 Design Guides 的密度层级；鼠标光标使用 `default`。
- 动效只用于窗口出现/消失和视图推入/弹出（短时淡入与轻微位移），遵守减少动态效果设置。
- 界面文案走 `rust-i18n`，默认提供 `en`、`zh-CN`、`zh-HK`；扩展标题暂不本地化，
  后续可在 `launcher.json` 中支持按语言的标题表。

## 10. 需要其他 crate 补充的能力

以下是实现中可能遇到的缺口，按「先用现有接口、确需时再补」排列。
**`gpui-base` 不在其中。**

| 缺口                                             | 处理方式                                                                                         |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------------ |
| 宿主向脚本传递本次运行的参数                     | 不需要修改：用 `launch()` 宿主模块                                                               |
| 权限授权界面与持久化                             | 不需要修改：启动器实现，走 `PluginManager::load` 的授权回调                                      |
| `PluginManager` 加载时指定组件注册表             | 待验证；若当前只能用默认注册表，在 `gpui-shell` 增加带 `FrozenComponentRegistry` 的构造方式       |
| 宿主向脚本推送事件（如窗口被重新唤起、外观变化） | 第一版以 `launch()` 轮询 + 重新渲染替代；之后在 `gpui-shell` 增加通用的宿主事件订阅               |
| 组件描述之外的动态根搜索结果（fallback 命令）    | 需要 GPUI Shell §18.4 的贡献注册表；放在第三阶段                                                 |
| 全局快捷键、accessory 激活策略                   | 应用层依赖第三方 crate 或平台 API；若 GPUI 需要补接口，向上游提出                                |

## 11. 平台差异

| 能力          | macOS                        | Windows                | Linux X11          | Linux Wayland                                   |
| ------------- | ---------------------------- | ---------------------- | ------------------ | ----------------------------------------------- |
| 全局快捷键    | `global-hotkey`              | `global-hotkey`        | `global-hotkey`    | Portal `GlobalShortcuts`，否则 `launcher --toggle` |
| 置顶浮动窗口  | PopUp/Floating（待验证）     | 待验证                 | 待验证             | 依赖合成器；可考虑 layer-shell（待验证）        |
| 粘贴到前一应用 | 需要辅助功能权限             | `SendInput`            | `xdotool` 类方案   | 通常不可行，功能隐藏而非禁用                    |
| 选中文本读取  | 辅助功能 API                 | UI Automation          | PRIMARY 选区       | 受限                                            |

平台不支持的功能按 Coding Guides「Platform and capability boundaries」处理：
能力检查放在窄接口后面，界面上隐藏不可用的操作，并在文档中写明。

## 12. 开发体验

- `launcher dev <dir>`：通过 IPC 把本地目录注册为开发扩展，启用 `gpui-shell` 的热重载
  （GPUI Shell §21.2），保存即刷新当前视图；开发扩展在根搜索中带「开发」标记。
- `launcher types <dir>`：生成 `gpui-kit.d.ts` 与 `launcher.d.ts`，配合 `jsconfig.json`
  获得编辑器补全。
- 扩展的 `console` 输出进入启动器日志，在出错视图和设置页中可以查看。
- 模板：`launcher new <name>` 生成包含一个 `view` 命令的最小扩展。

## 13. 测试

遵循 `.claude/COMPONENT_TEST_RULES.md`，重点测逻辑而非像素：

- `search`：匹配打分、拼音、缩写、frecency 衰减与排序稳定性的单元测试。
- `extensions`：`launcher.json` 解析与错误信息；授权决定到 grant 的映射；卸载时任务取消。
- `launcher-shell`：组件描述快照测试（GPUI Shell §22.1，无需 GPU）；
  `launch()` 与 `validate` 声明一致性。
- `app`：GPUI 交互测试覆盖键盘路径：输入 → 选择 → `Enter`；`Cmd-K`；`Esc` 的逐层行为。
- 应用发现：用临时目录中的伪 `.app` / `.desktop` 文件测试解析，不依赖真实系统。

## 14. 里程碑

| 阶段 | 内容                                                                                                             | 完成标准                                            |
| ---- | ---------------------------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| M0   | 窗口、全局快捷键、单实例；应用发现与启动；匹配、拼音、frecency；系统命令；设置页（快捷键、主题）                | 三平台可用 `Alt-Space` 唤起并启动应用，冷启动后首次唤起 < 100 ms |
| M1   | `launcher.json`；`launcher-shell`：List、Detail、ActionPanel、`launch()`、Toast/HUD；`no-view`；开发模式与热重载；2 个示例扩展 | 示例扩展只用文档化的 API 完成，从根搜索进入 < 150 ms  |
| M2   | Form 与偏好（含钥匙串）；权限单与持久化；从 Git 安装与更新（复用 `gpui-shell` 的 Git 依赖机制）；Grid；参数输入 | 未授权扩展无法访问任何未批准的能力（有测试）        |
| M3   | `menu-bar` 命令与后台刷新；深度链接；fallback 命令与动态根结果（需要贡献注册表）；扩展索引/商店                | —                                                   |

## 15. 风险与待定问题

1. **VM 启动成本未测量。** GPUI Shell §20.8 的启动预算还没有数字。M1 前必须测量
   「首次进入某扩展命令」的延迟；若超过预算，考虑在唤起窗口时预热 runtime，
   或缓存 QuickJS 字节码。
2. **共享 VM 还是每扩展一个 VM**（§7.4）。先共享，以测量结果决定。
3. **搜索框归属宿主**会限制扩展做完全自定义的输入体验。这是有意的取舍：
   一致的键盘行为是启动器的核心价值。
4. **Wayland** 上全局快捷键与置顶窗口都依赖合成器，可能只能提供降级体验。
5. **与 Raycast 扩展的兼容性**：不做源码兼容。若日后有需求，可以写一个把
   `@raycast/api` 的 React 树转换为 `launcher` 组件描述的适配库，但那需要 JSX 编译，
   与 GPUI Shell 的「无构建步骤」原则冲突，因此不在本设计范围内。
6. **扩展的界面组件是否应并入 `gpui-component`？** List + ActionPanel + 宿主搜索框
   这一组合对其他应用（命令面板、快速切换器）也有价值。先在 `launcher-shell` 中实现，
   有第二个使用者后再考虑下沉。
