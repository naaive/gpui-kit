# DataKit

A database IDE in the spirit of JetBrains DataGrip, built on GPUI Kit. It
connects to PostgreSQL, MySQL and MariaDB, SQLite, SQL Server and ClickHouse,
directly or through an SSH tunnel.

```bash
cargo run -p datakit
```

The architecture, milestones and known limitations are described in
[`docs/DATAKIT-DESIGN.md`](../../docs/DATAKIT-DESIGN.md).

## What it does

- **Data sources.** Add one from the explorer's `+` or **File › New data
  source…**: choose the database, fill in the address or pick the SQLite file,
  optionally reach it through SSH, test the connection, and save. Passwords
  go to the system keychain, never to a file. What a data source contains is
  cached, so the explorer and completion work before it reconnects.
- **Database explorer.** Schemas, tables, views, columns, keys, indexes,
  triggers, routines and sequences, read as you expand them and filtered by
  name. Double-click or `F4` opens an object; the context menu opens consoles,
  shows the DDL, edits the table, draws an ER diagram, compares schemas,
  imports CSV, dumps or restores, and drops objects after confirmation.
- **Consoles.** A SQL editor that knows the catalog: completion of schemas,
  tables, columns, aliases, join conditions, functions, keywords and live
  templates; inspections for unknown names and risky statements with quick
  fixes; go to declaration, quick documentation, alias renaming and
  formatting. `:name` and `$1` parameters are asked for before running. Each
  console has its own session, runs in auto-commit or manual transaction mode,
  and can switch to another data source from its toolbar.
- **Files.** **File › Open SQL file…** edits a script in a console that saves
  back to the file; **File › Attach folder…** lists a folder's scripts in the
  Files window. **Save as SQL file…** turns a scratch console into a file.
- **Execution.** `Ctrl-Enter` (`Cmd-Enter` on macOS) runs the statement at the
  caret, or every statement in the selection. A failed statement stops the
  run, and the server's error is underlined where it points. `Ctrl-F2`
  cancels the running statement on the server. Explain shows the plan as a
  tree with costs.
- **Results.** Rows stream in 500 at a time as you scroll. Sort by any column,
  copy a cell, row or column, or copy and export everything as TSV, CSV, JSON,
  SQL `INSERT` statements or a Markdown table.
- **Data editor.** A table's rows, paged and ordered by its key. Edit cells,
  add and delete rows, preview the SQL and submit it in one transaction, or
  revert. A value editor handles long text and JSON; the DDL tab shows the
  table's definition.
- **Structure tools.** Modify Table generates the `ALTER` statements for
  columns, indexes and foreign keys and previews them before running. Schema
  compare produces a migration script, and the ER diagram exports to Mermaid.
- **Query history and Services.** Every statement with its outcome and
  duration, searchable; the Services window shows what each console's session
  is doing and stops or disconnects it.
- **Settings.** **File › Settings…** chooses light, dark or the system's
  appearance, the interface language, and how SQL is formatted.

## Keys

| Key                          | Does                                       |
| ---------------------------- | ------------------------------------------ |
| `Cmd/Ctrl-Shift-L`           | New console for the selected data source   |
| `Cmd/Ctrl-O`                 | Open SQL files                             |
| `Cmd/Ctrl-Shift-S`           | Save the console as a SQL file             |
| `Cmd/Ctrl-Enter`             | Execute statement; submit changes in data  |
| `Ctrl-F2`                    | Cancel execution                           |
| `Cmd/Ctrl-Shift-E`           | Explain plan                               |
| `Cmd/Ctrl-Alt-L`             | Format SQL                                 |
| `Alt-Enter`                  | Show quick fixes                           |
| `Cmd/Ctrl-B`                 | Go to declaration                          |
| `Ctrl-Q` (`F1` on macOS)     | Quick documentation                        |
| `Shift-F6`                   | Rename alias                               |
| `F4`                         | Open the selected object or file           |
| `Cmd/Ctrl-K`                 | Search everywhere                          |
| `Cmd/Ctrl-N`                 | Go to object                               |
| `Cmd/Ctrl-Shift-A`           | Find action                                |
| `Cmd/Ctrl-,`                 | Settings                                   |
| `Alt-Insert`                 | Add a row in the data editor               |
| `F2`, `Enter`                | Edit the cell in the data editor           |
| `F5`, `Cmd/Ctrl-R`           | Reload the data editor                     |
| `Cmd/Ctrl-C` in results      | Copy the selected cell, row or column      |
| `Alt-1`                      | Show or hide the database explorer         |
| `Alt-8`                      | Show or hide the query history             |
| `Shift-Esc`                  | Zoom the focused tab                       |
| `Cmd/Ctrl-W`                 | Close the tab                              |

## Where things are kept

Everything lives in the platform's data directory under `DataKit`
(`%APPDATA%\DataKit` on Windows, `~/Library/Application Support/DataKit` on
macOS, `~/.local/share/DataKit` on Linux), or in `DATAKIT_DATA_DIR` when it is
set:

| File                | Holds                                         |
| ------------------- | --------------------------------------------- |
| `data-sources.json` | Data sources, without passwords               |
| `cache/*.json`      | What each data source contained when last read |
| `history.sqlite3`   | Query history                                 |
| `layout.json`       | Where the tool windows and consoles are       |
| `consoles/*.sql`    | The text of each scratch console              |
| `folders.json`      | The folders attached to the Files window      |
| `settings.json`     | Appearance, language and SQL formatting       |
| `known_hosts`       | SSH host keys, trusted on first use           |

## Testing

```bash
cargo test -p datakit -p datakit-catalog -p datakit-driver -p datakit-driver-postgres \
  -p datakit-driver-mysql -p datakit-driver-sqlite -p datakit-driver-mssql \
  -p datakit-driver-clickhouse -p datakit-runtime -p datakit-sql -p datakit-store \
  -p datakit-tunnel
```

The server drivers' integration tests need a server and run only when their
variable is set; each test works in a schema or database of its own:

| Driver     | Variable                       |
| ---------- | ------------------------------ |
| PostgreSQL | `DATAKIT_TEST_PG_URL`          |
| MySQL      | `DATAKIT_TEST_MYSQL_URL`       |
| SQL Server | `DATAKIT_TEST_MSSQL_URL`       |
| ClickHouse | `DATAKIT_TEST_CLICKHOUSE_URL`  |

```bash
DATAKIT_TEST_PG_URL=postgres://postgres:secret@localhost:5432/postgres \
  cargo test -p datakit-driver-postgres
```
