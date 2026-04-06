# crabase

`crabase` is a standalone Rust CLI for querying [Obsidian Bases](./bases_docs/) data without launching Obsidian.

Today the project is focused on one workflow: reproducing `obsidian base:query ... format=csv` against a local vault of Markdown notes.

Documentation written by Claude Code.

## Status

This project is in active development and currently implements a useful subset of Obsidian Bases rather than full compatibility.

What works today:

- Scan an Obsidian-style vault for `.md` files
- Parse YAML frontmatter from notes
- Read `.base` files
- Apply root-level and view-level filters
- Evaluate expressions against `file.*`, note properties, and formulas
- Sort, group-sort, limit, and project selected columns
- Emit CSV output

Important current limitations:

- Only the `base:query` subcommand exists
- Only `format=csv` is supported
- `groupBy` is currently used for sorting, not grouped output sections or aggregates
- Compatibility with the full Obsidian Bases feature set is incomplete

## Why This Exists

Obsidian’s Bases feature is useful, but the official CLI workflow depends on Obsidian itself. `crabase` exists to make Base queries scriptable and reproducible in a normal shell environment.

That makes it useful for:

- exporting vault data into other tools
- automating reporting tasks
- running Base queries in CI or other headless environments
- experimenting with Bases semantics outside the Obsidian app

## Installation

### Nix development shell

This repository is set up for Nix-based development:

```bash
nix develop
```

### Build with Cargo

```bash
cargo build
```

Run the CLI directly:

```bash
cargo run -- base:query file=../test.base format=csv vault=tests/fixtures/vault
```

## Usage

The CLI surface is intentionally small:

```bash
crabase base:query file=<path-relative-to-vault> format=csv [vault=<vault_root>] [view=<view_name>]
```

Arguments:

- `file`: path to the `.base` file, relative to the vault root
- `format`: must be `csv`
- `vault`: vault root directory; defaults to the current working directory
- `view`: optional view name; defaults to the first view in the `.base` file

Example:

```bash
cargo run -- \
  base:query \
  file=../test.base \
  format=csv \
  vault=tests/fixtures/vault
```

This produces:

```csv
date,title,session,church,series,testament,books,preacher,wc
[[2025-04-27]],[[Church/Sermons/2025-04-27 All Souls 1130am -- House of Blood.md| House of Blood]],1130,[[All Souls Langham Place]],[[House of David]],[[Old Testament]],[[2 Samuel]],[[Charlie Skrine]],532
[[2025-05-04]],[[Church/Sermons/2025-05-04 All Souls 1130am -- Everlasting People.md| Everlasting People]],1130,[[All Souls Langham Place]],[[House of David]],[[Old Testament]],[[2 Samuel]],[[Charlie Skrine]],549
```

## Supported `.base` Features

The parser and evaluator currently support these top-level concepts:

- `filters`
- `formulas`
- `properties` parsing
- `views`

Supported filter tree forms:

- bare expression strings
- `and`
- `or`
- `not`
- bare YAML sequences, treated as `and`

Supported view fields:

- `type`
- `name`
- `limit`
- `order`
- `filters`
- `groupBy`
- `sort`

## Expression Model

Expressions are evaluated against three namespaces:

- `file.*`: derived file metadata such as `name`, `path`, `folder`, `ext`, `size`, `tags`, and `links`
- note properties: values from YAML frontmatter
- `formula.*` or formula identifiers: expressions defined in the `.base` file

Examples of supported expression shapes in the current implementation:

- comparisons such as `session == 1130`
- string concatenation such as `title + " World"`
- file predicates such as `file.inFolder("Church/Sermons")`
- common string methods such as `.contains(...)`, `.startsWith(...)`, `.endsWith(...)`, `.lower()`, `.upper()`, `.trim()`, and `.reverse()`

## Example `.base` File

The fixture used by the integration tests looks like this:

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

## Repository Layout

```text
src/
  main.rs         CLI entry point
  lib.rs          library entry point
  base_file.rs    .base parsing
  vault.rs        vault scanning and frontmatter extraction
  filter.rs       filter evaluation
  query.rs        query execution, sorting, projection
  output.rs       CSV rendering
  expr/           expression lexer, parser, AST, evaluator
tests/
  integration.rs  snapshot and property tests
  fixtures/       sample vault and .base files
  snapshots/      committed insta snapshots
bases_docs/       local reference material for Obsidian Bases
```

## Development

Run the test suite:

```bash
cargo test
```

Run a specific integration test:

```bash
cargo test --test integration
```

Review snapshot changes:

```bash
cargo insta review
```

Accept snapshot changes:

```bash
cargo insta accept
```

## Notes on Output Semantics

Some output behavior is intentionally opinionated to match the current implementation:

- The `title` column is rendered as an Obsidian wikilink using the note path and display title
- Frontmatter values are emitted directly into CSV
- List values are flattened into comma-separated strings in CSV output
- Wikilinks found in note content are collected into `file.links`

## Reference Material

- [CLAUDE.md](/Users/simonzeng/repos/crabase/CLAUDE.md)
- [REPRO.md](/Users/simonzeng/repos/crabase/REPRO.md)
- [bases_docs/](/Users/simonzeng/repos/crabase/bases_docs/)
