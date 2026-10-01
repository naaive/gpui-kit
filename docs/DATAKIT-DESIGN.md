# DataKit 架构方案

> 状态：M0–M6 已实现（`examples/datakit`，运行 `cargo run -p datakit`）；与 DataGrip 的差距
> 与已知限制见 §7、§8。定位为继 Launcher 之后的第二个旗舰示例：一个功能
> 对标 JetBrains DataGrip 的数据库 IDE，用来验证 GPUI Kit 能否承载大表格、代码编辑器、Dock
> 布局与长时异步 IO 同时存在的专业桌面工具。使用说明见
> [examples/datakit/README.md](../examples/datakit/README.md)。

## 0. 已确定的决策

| 问题       | 决定                                                                                   |
| ---------- | -------------------------------------------------------------------------------------- |
| 定位       | 旗舰示例，放在 `examples/datakit`；按独立产品的标准设计                                |
| 数据库     | PostgreSQL 最先；MySQL/MariaDB、SQLite、SQL Server、ClickHouse 按同一 trait 接入        |
| 方言       | 内省 SQL、分页、DDL、ALTER、行修改与 EXPLAIN 都由 `Dialect` 生成，界面不分支到具体数据库 |
| SSH        | `datakit-tunnel` 用 `russh` 在 Tokio 上做本地转发；主机密钥首次信任后记入 `known_hosts` |
| 异步运行时 | 仓库统一使用 GPUI executor 与 smol；驱动需要的 Tokio 只存在于 `datakit-runtime` 一处     |
| 取值方式   | 走 simple query 协议，值保留服务器文本；先 `prepare` 一次取得列类型                     |
| 元数据     | 不可变的 `Catalog` 快照，按需懒加载；树、补全、以后的 Schema Diff 读同一份             |
| 会话       | 每个控制台独占一个会话；数据源另有一个元数据连接，长查询不会阻塞浏览器                  |
| 语句边界   | 自写容错词法器，不用 `sqlparser`：编辑中的文本大多无法完整解析                          |
| 密码       | 只进系统钥匙串；钥匙串不可用时只在内存中保留到退出，从不写入明文文件                    |

## 1. 核心思想：数据库知识与界面分离

```text
┌──────────────────────── examples/datakit（应用，依赖 GPUI）────────────────────────┐
│ workspace   窗口 · Dock 布局 · 菜单 · 状态栏 · 布局持久化                            │
├─────────────────────────────────────────────────────────────────────────────────────┤
│ explorer    数据库浏览器         console   SQL 控制台与会话      history  查询历史  │
│ datasource  数据源与属性对话框   results   结果网格、复制、导出                     │
├─────────────────────────────────────────────────────────────────────────────────────┤
│ services    IO 运行时 · 驱动注册表 · 钥匙串 · 数据目录（GPUI Global）               │
└─────────────────────────────────────────────────────────────────────────────────────┘
        │ 只经由下面这些 crate 认识数据库与 SQL；这些 crate 不依赖 GPUI
        ▼
┌─────────────┐  ┌─────────────┐  ┌──────────────────┐  ┌──────────────┐  ┌───────────┐
│ datakit-sql │  │ datakit-    │  │ datakit-driver-  │  │ datakit-     │  │ datakit-  │
│ 词法 · 语句 │  │ store       │  │ postgres         │  │ runtime      │  │ catalog   │
│ 边界 · 补全 │  │ 数据源文件  │  │ tokio-postgres   │  │ Tokio 边界   │  │ 元数据快照│
└──────┬──────┘  │ 钥匙串 · 历史│  └────────┬─────────┘  └──────────────┘  └─────▲─────┘
       │         └──────┬──────┘           │                                     │
       └───────────────┬┴──────────────────┘                                     │
                       ▼                                                         │
               ┌────────────────┐                                                │
               │ datakit-driver │  Driver · Connection · Dialect · Value ────────┘
               └────────────────┘
```

由此得到三个性质：

1. **数据库知识可以脱离 GPU 测试。** 语句切分、补全、导出、值的解析与排序都是纯函数，
   PostgreSQL 驱动有一套可连真实服务器的集成测试。
2. **驱动是插件，方言是数据。** `Driver`/`Connection` 只做 IO；引号、保留字、关键字、
   “选出前 N 行”怎么写都在 `Dialect` 里，补全和导出读它而不分支到具体数据库。
3. **Tokio 不进入界面。** 视图只见到 `RemoteTask`（一个 future）与 `RemoteStream`
   （一个 stream）；它们可被任何 executor 轮询，drop 即取消，与 GPUI `Task` 的规则一致。

### 1.1 目录

```text
examples/datakit/
  src/
    main.rs        启动：gpui_kit::init → Services → 各功能 init → 打开窗口
    services.rs    IoRuntime、DriverRegistry、SecretStore、数据目录
    datasource/    DataSource（元数据连接 + Catalog）、DataSources（列表与持久化）、属性对话框
    explorer/      数据库浏览器面板：懒加载的树、过滤、右键菜单、快速文档
    console/       控制台面板、执行与取消、事务模式、补全/悬停/跳转/修复 Provider、参数对话框、会话表
    results/       ResultGrid（TableDelegate）、ResultView、导出格式、PlanView（EXPLAIN 树）
    table_editor/  数据编辑器：分页、单元格编辑、ChangeSet、值编辑器、DDL 页
    designer/      Modify Table 对话框（列、索引、外键 → ALTER）
    compare/       Schema Diff 面板与迁移脚本
    diagram/       ER 图（分层布局、Mermaid 导出）
    import/ dump/  CSV 导入；调用数据库自带工具的 Dump/Restore
    files/         Files 工具窗口：附加的文件夹与其中的 .sql 文件
    search.rs      Search Everywhere（对象与命令）
    navigation.rs  跨面板的“显示/打开对象”事件
    settings.rs    外观、语言、SQL 格式化设置
    services_panel.rs  Services 工具窗口：每个控制台会话的状态、取消与断开
    history/       HistoryLog（SQLite）、历史面板
    workspace/     Workspace、StartPanel、菜单与快捷键、布局持久化
    format.rs      数字、时长、时间的显示
  crates/
    catalog/  driver/  driver-postgres/  driver-mysql/  driver-sqlite/  driver-mssql/
    driver-clickhouse/  runtime/  sql/  store/  tunnel/
  locales/ui.yml   en · zh-CN · zh-HK
```

与最初的方案相比，依赖方向做了一处调整：`datakit-catalog` 位于最底层、不依赖任何
crate，`datakit-driver` 依赖它。内省结果直接以 `Catalog` 的对象表达，驱动不必另造一套
中间类型。

## 2. Tokio 边界（`datakit-runtime`）

GPUI 有自己的 executor，仓库其余部分都在其上运行；成熟的 PostgreSQL 驱动却建立在
Tokio 之上，需要 Tokio 的 socket 与任务。`IoRuntime` 持有一个两线程的 Tokio
运行时，对外只给两样东西：

- `spawn(future) -> RemoteTask<T>`：结果经 oneshot 返回；drop 时 `abort`。界面在
  `cx.spawn` 里 `await` 它，于是“关闭控制台即取消查询”自然成立。
- `forward(stream, buffer) -> RemoteStream<T>`：在运行时上驱动 stream，最多领先
  读者 `buffer` 项；读者 drop 时先关闭通道，让源 stream 在运行时线程上被 drop，
  驱动的清理逻辑（见 §3.2）因此能做 IO。

这是本仓库第一次引入 Tokio，选择把它收敛在一个 crate、一个 `Global` 里，而不是在
各处 `block_on` 或另起线程。

## 3. 驱动（`datakit-driver`、`datakit-driver-postgres`）

### 3.1 取值

`Connection::execute` 在服务器描述完结果时就返回，行随后以 `RowStream` 流式到达。
PostgreSQL 驱动先 `prepare` 语句拿到列类型，再用 simple query 协议执行：每个值都是
服务器的文本，`numeric` 不丢精度、`timestamptz` 带着服务器时区、自定义类型也能显示。
`Value` 只为需要计算的类型解析（布尔、整数、浮点），其余保留文本；列的
`TypeCategory` 决定对齐、排序与导出时是否加引号。不能 `prepare` 的语句照常执行，
只是列没有类型。

### 3.2 取消

- 用户取消：`Connection::cancel` 通过 `CancelToken` 发出取消请求（与原连接相同的 TLS）。
- 放弃结果：`RowStream` 未读完就被 drop 时，驱动在 Tokio 上发出取消，否则会话要先把
  剩余的行全部收下并丢弃，才能执行下一条语句。集成测试
  `dropping_a_long_result_frees_the_session` 用 5000 万行验证这一点。

### 3.3 错误

服务器报告的错误转换为 `DatabaseError`（消息、SQLSTATE、detail、hint、位置）。
PostgreSQL 的位置以字符计、从 1 开始，驱动换算成语句内的字节偏移，控制台据此在编辑器
里划出波浪线。

### 3.4 内省

读 `pg_catalog` 而不是 `information_schema`：后者在大库上慢，并且会隐藏当前用户不拥有
的对象。模式列表与 `search_path` 一次读取；一个模式的内容（关系、列、索引、约束、触发器、
函数、序列、视图定义）用八条并发查询读取，`Connection::introspect_schema` 返回完整的
`Schema`。其他驱动读各自的系统表：MySQL 的 `information_schema`，SQLite 的
`pragma_*`，SQL Server 的 `sys.*`，ClickHouse 的 `system.*`。

### 3.5 方言生成的 SQL

`Dialect` 的默认实现给出标准 SQL，各驱动只覆盖不同之处：`select_page`（`LIMIT/OFFSET`
或 `OFFSET … FETCH`）、`row_change`（按主键或唯一键定位一行）、`insert_rows`、
`explain` 与 `parse_plan`、`create_relation`/`alter_relation`/`migrate` 等 DDL。数据
编辑器、Modify Table、Schema Diff 与导入都只拼装这些方法的结果。DDL 生成有往返测试：
在真实 PostgreSQL 上执行生成的 `CREATE`，再内省回来比较。

## 4. 元数据（`datakit-catalog`）

`Catalog` 是 `Arc` 共享的不可变树：模式 → 关系（表、分区表、外部表、视图、物化视图）
→ 列。更新总是生成新快照（`with_schemas`、`with_relations`、`without_relations`），
其他线程上的读者永远不会看到半个更新，复制只是增加引用计数。

一个模式的 `relations()` 为 `None` 表示“尚未读取”，空切片表示“确实为空”。浏览器展开
时、补全需要时才读取：补全引擎把缺失的模式放进 `Completions::missing_schemas`，控制台
请数据源去读，下一次按键就能补全。`DataSource::ensure` 不会重复请求已失败的内容，避免
每次按键都重试。

快照可序列化，`CatalogCache` 按数据源写入 `cache/<id>.json`：启动后立即可用，连接后再
后台刷新已读过的模式。`diff_schemas` 比较两个 `Schema` 得到 `SchemaDiff`，由方言的
`migrate` 写成迁移脚本。

## 5. SQL（`datakit-sql`）

### 5.1 词法

按 PostgreSQL 的规则切分 token：单引号串、`E''`/`B''`/`X''`/`U&''` 串、双引号标识符、
`$tag$…$tag$` 美元引用、嵌套块注释、`$1` 与 `:name` 参数。不会失败：结尾处未闭合的
串或注释成为 `Unterminated` token。

### 5.2 语句边界

`;` 结束语句（`BEGIN ATOMIC … END` 体内除外）。另外，行首出现 `SELECT`、`INSERT`、
`CREATE` 等语句关键字、且不在括号内时，如果前一条语句不能这样延续，也视为新语句：控制台里
常有人漏写最后的 `;`，DataGrip 也这样理解。`INSERT … SELECT`、`WITH … SELECT`、
`CREATE … AS SELECT`、`UNION` 与延续表达式的行仍属同一条语句。

“执行光标处语句”：光标在语句内或其 `;` 之后的同一行时取该语句；单独一行时取下一条。

### 5.3 补全

不依赖完整解析，而是像人一样读光标附近：

| 位置                         | 候选                                         |
| ---------------------------- | -------------------------------------------- |
| `FROM`、`JOIN`、`INTO` 之后  | `search_path` 中的关系，其余模式的关系，模式 |
| `别名.` 或 `表.` 之后        | 该关系的列                                   |
| `模式.` 之后                 | 该模式的关系（未加载则请求加载）             |
| 表达式中                     | 语句已引用关系的列、别名、函数、关键字       |

| `JOIN … ON` 之后             | 外键推出的连接条件，然后同“表达式中”         |

候选按“前缀匹配优先、包含匹配其次”排序，需要时自动加引号；关键字随输入的大小写插入。
Live Templates（`sel`、`ins`、`upd` 等）作为补全候选出现，插入后光标落在模板指定处。

### 5.4 分析与改写

同一套引用分析（语句中的关系、别名、CTE 及其位置）支撑：

- **Inspection**：未知模式/关系/列、歧义列、无 `WHERE` 的 `DELETE`/`UPDATE`，以编辑器
  诊断显示；`Alt-Enter` 给出快速修复（改为最接近的名称、补上 `WHERE`）。
- **跳转与文档**：`Cmd/Ctrl-B` 在浏览器中显示对象，或跳到语句内别名的声明；快速文档
  显示列、键与引用。
- **重命名别名**、**格式化**（`sqlformat`，关键字大小写取自设置）、**参数**（`$1`、
  `:name` 在执行前询问并替换为方言的字面量）。

## 6. 界面

窗口是文档工作区：左侧数据库浏览器，中间控制台标签，底部查询历史（默认收起）。没有
控制台时中间显示开始面板，给出下一步与快捷键。

- **浏览器**：数据源 → 模式 → 表/视图分组 → 关系 → 列。图标与固定宽度的展开槽让标签
  对齐；连接状态用图标颜色表达，失败原因作为子行显示。双击关系打开数据；右键菜单提供新建
  控制台、连接/断开、刷新、属性…、移除…（`AlertDialog` 确认，因为密码会被删除）。
- **控制台**：工具栏（执行、取消、会话状态、运行计时），编辑器，以及执行后出现的结果区
  （输出日志 + 每条返回行的语句一个结果标签）。`Cmd/Ctrl-Enter` 绑定在
  `Console > Input` 上下文，比编辑器自身的绑定更具体，因此不会插入换行。
- **结果**：虚拟化网格，紧凑密度；数值列右对齐，`NULL` 以弱化色显示；列宽按列名和首页
  数据估算。每页 500 行，滚动接近末尾时取下一页；排序只改变显示顺序，之后到达的行会按
  当前排序插入。复制选中的单元格、行或列；整体复制或导出为 TSV、CSV、JSON、SQL INSERT、
  Markdown。
- **历史**：SQLite 中保存最近一万条，带结果、行数、耗时；可搜索，确认后在新控制台中打开。
- **数据编辑器**：按主键排序、`LIMIT/OFFSET` 分页（不占住会话，因此刷新与提交不会被
  未读完的结果阻塞）；修改记入 `ChangeSet`，可预览 SQL，在一个事务中提交，失败即回滚；
  计数在另一个会话中进行。
- **工具**：Modify Table、Schema Diff、ER 图、EXPLAIN 树、CSV 导入、Dump/Restore 分别是
  对话框或中间面板，都从浏览器的右键菜单进入。
- **文件**：控制台可以编辑一个 `.sql` 文件（打开、另存为），Files 工具窗口列出附加文件夹
  里的脚本；控制台的数据源可在工具栏切换。
- **Services**：列出每个控制台会话的状态（空闲、事务中、运行第几条及时长），可取消或断开。
- **设置**：外观（跟随系统、浅色、深色）、界面语言、格式化时关键字大小写，保存为
  `settings.json`，修改立即生效（菜单随语言重建）。

布局在变化后 2 秒及退出时保存到数据目录；临时控制台的文字保存为 `consoles/<id>.sql`，
布局只记录控制台的 id、名称、数据源与文件路径。所有界面文字经 `rust-i18n`，提供 en、
zh-CN、zh-HK。

## 7. 里程碑（对标 DataGrip）

| 里程碑 | 内容                                                                                         | 状态                                      |
| ------ | -------------------------------------------------------------------------------------------- | ----------------------------------------- |
| M0     | crate 结构、Tokio 边界、数据源与测试连接、钥匙串、SSH 隧道、Dock 布局与持久化                 | 完成                                      |
| M1     | 执行光标处/选区语句、多结果、取消、计时、流式分页、排序、复制与导出、查询历史、Services       | 完成                                      |
| M2     | 完整内省（索引、约束、触发器、函数、序列）、元数据缓存、快速文档、DDL、Search Everywhere      | 完成（缓存为 JSON 而非 SQLite）           |
| M3     | 语义补全、Inspector 诊断与快速修复、跳转定义、重命名别名、格式化、Live Templates、参数化查询 | 完成                                      |
| M4     | 网格编辑、增删行、ChangeSet 预览与提交、事务模式、值编辑器                                   | 完成                                      |
| M5     | Modify Table、Schema Diff、ER 图、EXPLAIN 可视化、导入、Dump/Restore                          | 完成（新建对象向导由 Modify Table 兼任）  |
| M6     | MySQL/MariaDB、SQLite、SQL Server、ClickHouse；文件工作区；主题与语言设置                     | 完成（性能未做系统测量）                  |
| M7     | 补齐日常差距：`Ctrl-Space`、参数信息、Find Usages、表/列重命名、展开 `*`；结果服务器端筛选、区域选择与统计、记录视图、XLSX；粘贴到数据编辑器；数据源颜色与只读；文件监视；最近文件；Local History；用户与角色；表数据比较 | 完成                                      |

与 DataGrip 的明确差距（不假装对标）：JDBC 驱动生态（Oracle、MongoDB 等）、VCS 集成、插件市场；
授予与回收权限的界面、存储过程调试器、服务器会话管理、在多个数据源上同时执行、书签，以及可编辑的
ER 图，也都还没有做。

## 8. 已知限制与取舍

- **控制台结果占住会话。** 控制台的结果流在读完或被丢弃前持有会话与它取得的锁；在
  PostgreSQL 上，一个未读完的 `SELECT` 会挡住同一张表上的 `ALTER`。数据编辑器因此改用
  分页查询，控制台仍按 DataGrip 的方式流式读取。
- **点列头排序在本地。** 点列头只对已取回的行排序，并显示“N+ 行”；要在服务器端排序或筛选，
  用结果上方的 `WHERE`/`ORDER BY`：它把语句包成派生表重新执行（SQL Server 不允许派生表里
  有不带 `TOP` 的 `ORDER BY`，这类语句需要先去掉排序）。
- **部分驱动只经过单元测试。** PostgreSQL 在真实服务器上验证过，SQLite 用临时文件测试；
  MySQL、SQL Server 与 ClickHouse 的集成测试已写好，但验证环境中没有对应的服务器，尚未
  运行过。SSH 隧道同样只经过单元测试。
- **SQL Server 的 `[标识符]` 不作为一个 token。** 词法器认识双引号与反引号标识符，方括号
  会被拆开，补全与诊断在其中不准确。
- **参数是文本替换。** `$1`、`:name` 在客户端替换为字面量后执行，而不是服务器端绑定。
- **外部修改与未保存的编辑冲突时以控制台为准。** 控制台在打字后半秒自动保存；外部程序在这
  半秒内修改同一文件时，控制台的内容会覆盖它，而不是像 DataGrip 那样询问。
- **只读是客户端检查。** 只读数据源按语句的首词与其中的写操作关键字拒绝写语句，不在服务器
  端设置只读会话；存储过程里的写入检查不到。
- **重命名只改打开的控制台。** 重命名表、视图或列时，`ALTER` 之外只改同一数据源下打开的控
  制台里解析到该对象的名称；附加文件夹里未打开的脚本不会改。
- **区域选择只用键盘和 Shift 点击。** 结果网格不支持拖动鼠标框选。
- **用户与角色只在 PostgreSQL 上验证过。** MySQL（读 `mysql.user`）、SQL Server 与 ClickHouse 的
  查询没有在真实服务器上跑过；会话没有权限读取时显示为空。
- **数据比较一次读入内存。** 两张表各读前 10 万行，按源表的主键（没有主键时按全部列）匹配。

## 9. 测试

按层测试，越低越好：

| 层             | 测试                                                                                     |
| -------------- | ---------------------------------------------------------------------------------------- |
| 纯 Rust        | 词法、语句边界、补全、诊断、格式化、模板、参数、重命名（sql）；快照与 Diff（catalog）；值、方言与 DDL（driver）；ChangeSet；文件夹扫描；导出 |
| IO 边界        | `RemoteTask` 取消与 detach、`RemoteStream` 顺序与 drop（runtime）；数据源文件、历史与元数据缓存（store）|
| 真实服务器     | 各驱动 `tests/`，设置 `DATAKIT_TEST_PG_URL`、`_MYSQL_URL`、`_MSSQL_URL`、`_CLICKHOUSE_URL` 时运行；SQLite 用临时文件 |
| GPUI           | 结果网格翻页后保持排序、短页结束结果（`results::result_view` 中的 `#[gpui_kit::test]`）  |

PostgreSQL 真实服务器测试覆盖：值与类型、命令摘要、错误位置、放弃长结果后会话可用、取消
运行中的语句、完整内省，以及生成的 DDL 执行后再内省的往返。
