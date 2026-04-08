# crabase

`crabase` is a Rust CLI and library for reading Obsidian `.base` files and running Base-style queries directly against a local Markdown vault.

Today it is best understood as a partial, scriptable implementation of Obsidian Bases semantics, with strongest support around querying table views and exporting CSV.

Documentation written by Claude Code.

## Current Status

The repository has moved beyond a single `base:query` workflow. The current CLI supports:

- `base:query`: run a view from a `.base` file and write CSV to stdout
- `base:views`: list the views defined in a `.base` file
- `bases`: list all `.base` files under a vault

The implementation is still intentionally partial. The codebase parses more of the Bases shape than it fully renders, and some features are currently used only to approximate Obsidian behavior rather than reproduce it exactly.

## What It Can Do

### Vault scanning

`crabase` can:

- walk a vault recursively and read `.md` notes
- discover `.base` files anywhere under the vault
- parse YAML frontmatter at the start of a note
- extract file metadata into `file.*` fields
- collect tags from both frontmatter and inline `#tags`
- collect wikilinks from note content into `file.links`

The file metadata currently exposed to expressions includes:

- `file.name`
- `file.path`
- `file.folder`
- `file.ext`
- `file.size`
- `file.ctime`
- `file.mtime`
- `file.tags`
- `file.links`

### `.base` parsing

The parser currently understands these root-level sections:

- `filters`
- `formulas`
- `properties`
- `views`

Within a view, it currently reads:

- `type`
- `name`
- `limit`
- `order`
- `filters`
- `groupBy`
- `sort`

`properties.<name>.displayName` is used for CSV headers.

### Query execution

For `base:query`, the engine currently:

- applies root-level filters and view-level filters
- evaluates formulas during filtering and projection
- sorts by `groupBy` first and then `sort`
- applies `limit`
- projects columns from `order`
- writes CSV output

The current output model is table-oriented. `groupBy` affects sort order, but does not yet produce grouped sections, aggregates, or non-tabular layouts.

### Supported filter shapes

Filters can be written as:

- a bare expression string
- `and:`
- `or:`
- `not:`
- a bare YAML sequence, treated as `and`

### Expression language

The expression engine supports:

- literals: numbers, strings, booleans, `null`
- arrays: `[a, b, c]`
- variables from note properties
- namespaced access via `file.*`, `note.*`, and `formula.*`
- bracket access such as `formula["name"]`
- unary operators: `!`, unary `-`
- binary operators: `+`, `-`, `*`, `/`, `%`, `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`
- member access, indexing, and function/method calls

Top-level functions currently implemented:

- `if(...)`
- `list(...)`
- `number(...)`
- `min(...)`
- `max(...)`
- `date(...)`
- `today()`
- `now()`
- `link(...)`

Implemented file methods:

- `file.inFolder(...)`
- `file.hasTag(...)`
- `file.hasLink(...)`
- `file.hasProperty(...)`
- `file.asLink(...)`

Implemented string methods:

- `.contains(...)`
- `.startsWith(...)`
- `.endsWith(...)`
- `.lower()`
- `.upper()`
- `.title()`
- `.trim()`
- `.reverse()`
- `.slice(...)`
- `.split(...)`
- `.repeat(...)`
- `.replace(...)`
- `.toFixed(...)`
- `.length`
- `.isEmpty()`
- `.toString()`
- `.isTruthy()`
- `.isType(...)`

Implemented number methods:

- `.abs()`
- `.ceil()`
- `.floor()`
- `.round(...)`
- `.toFixed(...)`
- `.days`
- `.isEmpty()`
- `.toString()`
- `.isTruthy()`
- `.isType(...)`

Implemented list methods:

- `.contains(...)`
- `.containsAll(...)`
- `.containsAny(...)`
- `.map(...)`
- `.join(...)`
- `.reverse()`
- `.sort()`
- `.unique()`
- `.flat()`
- `.slice(...)`
- `.length`
- `.isEmpty()`
- `.isTruthy()`
- `.isType(...)`

Implemented date properties and methods:

- `.year`
- `.month`
- `.day`
- `.hour`
- `.minute`
- `.second`
- `.millisecond`
- `.format(...)`
- `.date()`
- `.time()`
- `.relative()`
- `.isEmpty()`
- `.isTruthy()`
- `.toString()`
- `.isType(...)`

Date arithmetic currently supports:

- `date + "1d"` style duration addition
- `date - "2w"` style duration subtraction
- `date - date`, yielding a numeric millisecond difference
- `(date_a - date_b).days` to convert that difference into integer days

Supported duration units are:

- days: `d`, `day`, `days`
- weeks: `w`, `week`, `weeks`
- months: `M`, `month`, `months`
- years: `y`, `year`, `years`
- hours: `h`, `hour`, `hours`
- minutes: `m`, `minute`, `minutes`
- seconds: `s`, `second`, `seconds`

### Output behavior

CSV output currently has these semantics:

- headers come from `properties.<column>.displayName` when present
- `formula.` is stripped from header names
- other dotted names are rendered with spaces in headers
- `title` is rendered as an Obsidian wikilink using the note path and display title
- list values are flattened as comma-separated cells
- missing values render as blank CSV cells

## Important Limits

The current implementation does not aim for full Obsidian compatibility yet. Known limits include:

- only CSV output is supported
- query rendering is table-oriented even if other view `type` values are parsed
- `groupBy` is used for sorting, not grouped output
- sorting only reads note and `file.*` properties; sort keys on `formula.*` are currently ignored in practice
- YAML object values are not preserved as rich runtime values
- unknown methods generally resolve to `null` rather than producing strict compatibility errors
- feature coverage is driven by implemented evaluator/runtime behavior, not by all syntax described in `bases_docs/`

## CLI Usage

### `base:query`

```bash
crabase base:query file=<path-relative-to-vault> format=csv [vault=<vault_root>] [view=<view_name>]
```

- `file=`: path to the `.base` file, relative to the vault root
- `format=`: must be `csv`
- `vault=`: defaults to the current working directory
- `view=`: selects a named view; defaults to the first view in the file

Example:

```bash
cargo run -- \
  base:query \
  file=../test.base \
  format=csv \
  vault=tests/fixtures/vault
```

### `base:views`

```bash
crabase base:views file=<path-relative-to-vault> [vault=<vault_root>]
```

This prints one view name per line, using `(unnamed)` for unnamed views.

### `bases`

```bash
crabase bases [vault=<vault_root>]
```

This prints all `.base` paths under the vault, sorted lexicographically.

## Example

Given this `.base` file:

```yaml
filters:
  and:
    - "file.inFolder(\"Church/Sermons\")"
views:
  - type: table
    name: "Sermons"
    order:
      - date
      - title
      - session
      - church
      - series
      - testament
      - books
      - preacher
      - wc
    groupBy:
      property: date
      direction: ASC
```

`base:query` produces CSV like:

```csv
date,title,session,church,series,testament,books,preacher,wc
[[2025-04-27]],[[Church/Sermons/2025-04-27 All Souls 1130am -- House of Blood.md| House of Blood]],1130,[[All Souls Langham Place]],[[House of David]],[[Old Testament]],[[2 Samuel]],[[Charlie Skrine]],532
[[2025-05-04]],[[Church/Sermons/2025-05-04 All Souls 1130am -- Everlasting People.md| Everlasting People]],1130,[[All Souls Langham Place]],[[House of David]],[[Old Testament]],[[2 Samuel]],[[Charlie Skrine]],549
[[2025-05-11]],[[Church/Sermons/2025-05-11 All Souls 1130am -- The Shepherd King.md| The Shepherd King]],1130,[[All Souls Langham Place]],[[House of David]],[[Old Testament]],[[2 Samuel]],[[Charlie Skrine]],
```

## Library Structure

```text
src/
  main.rs         CLI entry point
  lib.rs          library entry point
  error.rs        shared error types
  base_file.rs    .base parsing and view/filter data structures
  vault.rs        vault scanning, frontmatter parsing, tag/link extraction
  filter.rs       filter evaluation
  query.rs        query execution, sorting, projection
  output.rs       CSV rendering
  expr/           expression lexer, parser, AST, and evaluator
tests/
  integration.rs  insta snapshot tests and parser/evaluator property tests
  fixtures/       sample vault and .base fixtures
  snapshots/      committed insta snapshots
bases_docs/       local reference material for Obsidian Bases
```

## Development

This project is set up for Nix-based development:

```bash
nix develop
```

Build it with Cargo:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

Review snapshot updates:

```bash
cargo insta review
```

Accept snapshot updates:

```bash
cargo insta accept
```

## Reference Material

- [CLAUDE.md](/Users/simonzeng/repos/crabase/CLAUDE.md)
- [REPRO.md](/Users/simonzeng/repos/crabase/REPRO.md)
- [bases_docs/](/Users/simonzeng/repos/crabase/bases_docs/)
