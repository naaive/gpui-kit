# Launcher 架构方案

> 状态：方案，尚未实现。定位为 GPUI Kit 的旗舰示例（`examples/launcher`），
> 架构按独立产品的标准设计。引用的现有能力以 `crates/shell`、
> `crates/component-shell` 的源码与 [GPUI Shell](gpui-shell.md) 为准；
> 标注「待验证」的条目由 §9 的原型确认。

## 0. 已确定的决策

| 问题         | 决定                                                                 |
| ------------ | -------------------------------------------------------------------- |
| 定位         | 旗舰示例，放在 `examples/launcher`；平台先做好 macOS，其次 Linux     |
| 扩展界面     | 完全由宿主渲染：扩展只描述 List / Detail / Form，不绘制任何界面      |
| Raycast 兼容 | 不兼容；API 形状借鉴 Raycast，降低学习成本                           |
| 命令声明     | 静态写在 `launcher.json`，根搜索不执行扩展代码                       |
| VM           | 所有扩展共用一个 `ShellRuntime`，每个扩展持有自己的 `Policy`         |
| 第一步       | 端到端原型，先验证扩展路径，再做应用搜索与平台细节                   |

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
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── shell/          # window.rs · hotkey.rs · single_instance.rs · platform/
│   ├── model/          # page.rs · item.rs · action.rs · effect.rs
│   ├── session/        # navigator.rs · selection.rs · effect_runner.rs
│   ├── ui/             # search_bar.rs · list_view.rs · detail_view.rs · form_view.rs
│   │                   # action_panel.rs · footer.rs · launcher_window.rs
│   ├── pages/          # root_search.rs · settings.rs · script_page.rs
│   ├── search/         # index.rs · matcher.rs · pinyin.rs · frecency.rs
│   ├── sources/        # applications/{macos,linux}.rs · system.rs · extensions.rs
│   └── extensions/     # manifest.rs · catalog.rs · permissions.rs · preferences.rs
│                       # bridge/（组件注册与宿主模块）
└── extensions/         # 随示例附带的 JS 扩展：github、color、…
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

两个模块：`launcher` 提供页面节点，`launcher/host` 提供命令上下文与少量命令式函数
（模块名以原型验证为准）。方法名遵循 GPUI Shell 的约定：绑定方法用 snake_case，
作者自己的方法用 camelCase。

```js
import { View } from "gpui-kit";
import { List, ListItem, Action } from "launcher";
import { launch } from "launcher/host";

export default class SearchRepos extends View {
  init() {
    this.repos = [];
    this.loading = false;
    this.search(launch().arguments.query ?? "");
  }

  search(text, cx) {
    // fetch 受 gpui-shell.json 的 network 权限约束；完成后更新 this.repos 并 cx.notify()
  }

  render(cx) {
    return List.new()
      .placeholder("Search repositories")
      .loading(this.loading)
      .on_query_change((text, cx) => this.search(text, cx))
      .children(
        this.repos.map((repo) =>
          ListItem.new(repo.full_name)
            .title(repo.full_name)
            .subtitle(repo.description ?? "")
            .accessory(`★ ${repo.stargazers_count}`)
            .action(Action.open_url("Open in Browser", repo.html_url))
            .action(Action.copy("Copy Clone URL", repo.clone_url).shortcut("cmd-shift-c"))
            .action(Action.push("Show Details", () => new RepoDetail({ repo }))),
        ),
      );
  }
}
```

`render` 的返回值必须是 `List`、`Detail` 或 `Form` 之一；返回其他元素是错误，
宿主显示出错页面并指出这一点。扩展里没有颜色、间距或布局。

### 6.3 桥接：扩展如何产出 `PageModel`（`extensions/bridge`）

这是整个方案中最关键的机制，全部建立在 `gpui-shell` 已有的扩展点上：

1. 启动器用 `ComponentRegistry` 注册 `List`、`ListItem`、`Section`、`Detail`、`Form`、
   字段和 `Action` 等节点（与 `gpui-component-shell` 注册组件的方式相同）。参数由
   `ArgumentSchema` 校验，`launcher.d.ts` 由同一份描述生成，二者不会漂移。
2. 这些节点的 materializer **不绘制任何东西**：`List` 的 materializer 把收到的子节点、
   参数与回调组装成 `PageModel`，发布给宿主，自身返回一个空元素。
   这与 `MenuBar` 通过 `app_effects` 把脚本描述安装到原生菜单栏是同一种模式。
3. 扩展的 `ScriptView` 以零尺寸挂在启动器窗口里，只为参与 GPUI 的渲染循环；
   `ScriptPage` 把它发布的模型交给 `Navigator`。
4. 发布走两条通道：**内容**按哈希版本号去重，只有内容变化才让宿主重绘；
   **回调表**每次渲染都刷新，因此宿主手中的 `CallbackRef` 永远指向最新一代
   （回调是按渲染代次失效的，见 GPUI Shell §10.1）。

结果是：扩展享有 `gpui-shell` 的全部运行时能力（View 状态、`cx.notify()`、异步、
热重载、沙箱、出错恢复），而界面完全由宿主绘制。

### 6.4 生命周期

1. 发现：读取 `gpui-shell.json` 与 `launcher.json`，建立命令索引，不执行代码。
2. 执行命令：若扩展未加载，经 `PluginManager::load` 加载，授权回调应用已保存的
   权限决定；缺少必填偏好时先推入偏好表单。
3. 以命令模块为入口创建 `ScriptView`，包装成 `ScriptPage` 压入页面栈。
4. `Action.push` 的回调返回一个新的 View 实例，宿主为它创建新的 `ScriptPage`。
5. 最后一个页面出栈后，扩展保持加载 60 秒，便于再次进入；之后 `unload`，
   其名下的调度任务一并取消（GPUI Shell §18.3）。

### 6.5 权限与偏好

- 权限完全沿用 `gpui-shell` 的能力模型（默认无权限）。启动器补上 GPUI Shell §18.4
  所缺的授权产品层：首次运行前显示权限单，决定保存在启动器配置里而不是扩展目录中，
  更新后新增的权限重新询问，`execute: "*"` 以警告级别单独展示。
- 偏好由 `launcher.json` 声明，由宿主用 `Form` 模型渲染（设置页也是一个内置 Form
  页面）；`password` 类型存入系统钥匙串（`keyring`），只通过 `launch()` 交给所属扩展。

## 7. 窗口与唤起（`shell`）

- 一个常驻窗口，隐藏而非销毁；由 `gpui_kit::open_window` 创建。
- 无标题栏、居中、置顶、失焦即隐藏；尺寸约 `750 × 475`，以 `rem` 表达。
- 全局快捷键：GPUI 的 `KeyBinding` 只在应用获得焦点时生效，因此用 `global-hotkey`
  crate；Linux Wayland 退化为 Portal `GlobalShortcuts` 或 `launcher --toggle`。
- 单实例：本地 socket；承载 `--toggle`、`launcher dev <dir>`、深度链接
  `launcher://extensions/<id>/<command>`。
- 待验证：`WindowKind::PopUp` / `Floating` 的层级与全屏空间行为；macOS 不显示 Dock
  图标的接口；隐藏后再次唤起的焦点恢复。平台差异封装在 `shell/platform/`，不向上泄漏。

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

## 9. 原型：先证明扩展路径

第一步不是应用搜索，而是一条最窄的端到端路径：

> 窗口 + SearchBar + 内置根页面（写死几条命令）+ 一个 JS 扩展返回 `List`
> + `Enter` 执行 `OpenUrl` + `Action.run` 回调 + `Esc` 返回。

它回答下面五个问题，任何一个的答案为否，§6.3 都要调整：

| # | 问题                                                                     | 为否时的替代方案                                      |
| - | ------------------------------------------------------------------------ | ----------------------------------------------------- |
| 1 | materializer 能否把模型发布给宿主实体，且不在渲染中重入                    | 改用 `app_effects` 的延迟安装，或读取 `RenderSnapshot` |
| 2 | 零尺寸挂载的 `ScriptView` 是否持续参与渲染、`cx.notify()` 是否生效        | 在 `gpui-shell` 增加不依赖挂载的「无界面视图」驱动    |
| 3 | 回调表按代次刷新后，宿主持有的 `CallbackRef` 是否总能调用成功             | 回调改为按 `ActionId` 分发给页面的单一入口            |
| 4 | `PluginManager` 能否使用启动器自己的 `FrozenComponentRegistry`，并以命令模块为入口创建 View | 在 `gpui-shell` 增加对应构造方法                     |
| 5 | 首次进入扩展的延迟（GPUI Shell §20.8 尚无数据）                            | 唤起窗口时预热 runtime，或缓存字节码                  |

需要改动 `gpui-shell` 的部分（第 2、4 行，以及 `Action.push` 所需的「由脚本 View 实例
创建 `ScriptView`」）都是通用的宿主嵌入接口，不含启动器语义，适合回馈到 `gpui-shell`。
`gpui-base` 不需要修改。

## 10. 里程碑

| 阶段 | 内容                                                                                   | 完成标准                                             |
| ---- | -------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| M0   | §9 原型：`model`、`session`、`ui` 的 List 部分、`bridge` 最小实现、一个示例扩展           | 五个问题都有结论；`PageModel` 有快照测试              |
| M1   | 根搜索：应用发现（macOS、Linux）、匹配、拼音、frecency、系统命令；全局快捷键与窗口行为     | macOS 上 `Alt-Space` 唤起后 100 ms 内可输入           |
| M2   | 扩展平台：`launcher.json` 全量校验、Detail、Form、偏好与钥匙串、权限单、开发模式与热重载  | 两个示例扩展只用文档化的 API 完成；未授权能力有测试    |
| M3   | 从 Git 安装与更新、深度链接、Grid、菜单栏命令                                           | —                                                    |

## 11. 测试

- `search`：打分、拼音、缩写、frecency 衰减与排序稳定性的单元测试。
- `model` / `bridge`：示例扩展渲染出的 `PageModel` 快照测试，无需 GPU（GPUI Shell §22.1）。
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
4. **Wayland** 上全局快捷键与置顶窗口依赖合成器，只能提供降级体验。
