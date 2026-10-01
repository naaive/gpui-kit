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
  cached, so the explorer and completion work before it reconnects. A color
  marks a data source wherever its data shows, and a read-only data source
  runs only statements that read and cannot be edited.
- **Database explorer.** Schemas, tables, views, columns, keys, indexes,
  triggers, routines and sequences, and the server's users and roles with
  their memberships and DDL, read as you expand them and filtered by
  name. Double-click or `F4` opens an object; the context menu opens consoles,
  shows the DDL, edits the table, draws an ER diagram, compares schemas,
  imports CSV, compares a table's rows with another's, dumps or restores,
  renames tables, views and columns, and drops objects after confirmation.
- **Consoles.** A SQL editor that knows the catalog: completion of schemas,
  tables, columns, aliases, join conditions, functions, keywords and live
  templates, as you type or on `Ctrl-Space`; parameter info for routine
  calls; inspections for unknown names and risky statements with quick
  fixes, and expanding `*` to its columns; go to declaration, find usages,
  quick documentation and formatting. Renaming an alias changes its
  statement; renaming a table, view or column changes the database and every
  open console that names it. `:name` and `$1` parameters are asked for
  before running. Each console has its own session, runs in auto-commit or
  manual transaction mode, and can switch to another data source from its
  toolbar.
- **Files.** **File › Open SQL file…** edits a script in a console that saves
  back to the file, and shows changes other programs make to it; **File ›
  Attach folder…** lists a folder's scripts in the Files window, which
  follows the folder as it changes. **Save as SQL file…** turns a scratch
  console into a file. **Navigate › Recent files…** reopens the files and
  tables opened lately, and **File › Local history…** shows the earlier
  versions of a console's text — every version that ran, and one every few
  minutes while typing — and reverts to one.
- **Execution.** `Ctrl-Enter` (`Cmd-Enter` on macOS) runs the statement at the
  caret, or every statement in the selection. A failed statement stops the
  run, and the server's error is underlined where it points. `Ctrl-F2`
  cancels the running statement on the server. Explain shows the plan as a
  tree with costs.
- **Results.** Rows stream in 500 at a time as you scroll. A `WHERE` and
  `ORDER BY` above the grid read the statement again on the server through
  them. Sort what is fetched by any column; select a cell, row, column or a
  block of cells to copy it and see its count, sum, average, minimum and
  maximum; show the selected row one column a line in the record view; copy
  and export everything as TSV, CSV, JSON, SQL `INSERT` statements or a
  Markdown table, or export it as an Excel workbook.
- **Data editor.** A table's rows, paged and ordered by its key. Edit cells,
  paste rows copied from a spreadsheet or a result, add and delete rows,
  preview the SQL and submit it in one transaction, or revert. A value editor handles long text and JSON; the DDL tab shows the
  table's definition.
- **Structure tools.** Modify Table generates the `ALTER` statements for
  columns, indexes and foreign keys and previews them before running. Schema
  compare produces a migration script, data compare the `INSERT`, `UPDATE`
  and `DELETE` statements that make one table's rows another's, and the ER
  diagram exports to Mermaid.
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
| `Ctrl-Space`                 | Complete the word at the caret             |
| `Cmd/Ctrl-P`                 | Parameter info                             |
| `Alt-Enter`                  | Show quick fixes and intentions            |
| `Cmd/Ctrl-B`                 | Go to declaration                          |
| `Alt-F7`                     | Find usages in the open consoles           |
| `Ctrl-Q` (`F1` on macOS)     | Quick documentation                        |
| `Shift-F6`                   | Rename the alias, table, view or column    |
| `F4`                         | Open the selected object or file           |
| `Cmd/Ctrl-K`                 | Search everywhere                          |
| `Cmd/Ctrl-N`                 | Go to object                               |
| `Cmd/Ctrl-E`                 | Recent files                               |
| `Cmd/Ctrl-Shift-A`           | Find action                                |
| `Cmd/Ctrl-,`                 | Settings                                   |
| `Alt-Insert`                 | Add a row in the data editor               |
| `F2`, `Enter`                | Edit the cell in the data editor           |
| `F5`, `Cmd/Ctrl-R`           | Reload the data editor                     |
| `Shift` with arrows or click | Select a block of cells                    |
| `Cmd/Ctrl-C` in results      | Copy the selected cells, row or column     |
| `Cmd/Ctrl-V` in data         | Paste rows from the selected cell on       |
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
| `recent.json`       | The files and tables opened lately            |
| `local-history/`    | Earlier versions of each console's text       |
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
