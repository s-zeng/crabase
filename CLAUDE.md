# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with 
code in this repository.

## Style

Try to keep the style as functional as possible ("Ocaml with manual garbage 
collection", as opposed to "C++ with borrow checker"). Use features like 
Algebraic Data Types and Traits liberally, with an algebra-oriented design 
mindset

When writing new documentation files, ensure to clarify that "Documentation written 
by Claude Code" somewhere in the file.

ALL tests should be in the `tests/` directory, and should follow the snapshot 
testing instructions in the `## Testing` section.

This project is in heavy development. Whenever you make a change, make sure to 
check `CLAUDE.md` and update it if necessary to reflect any newly added/changed 
features or structures

## Error Handling & Safety Guidelines

### Never Use `unwrap()` in Production Code
- **NEVER** use `.unwrap()` on `Option` or `Result` types in production paths
- Use proper error handling with `?`, `.ok_or()`, `.map_err()`, or pattern matching
- Example: Replace `tag_name.chars().nth(1).unwrap()` with proper error handling
- Exception: Only use `unwrap()` in tests or when preceded by explicit checks that guarantee safety

### Error Message Quality
- Include contextual information in error messages
- Use structured error types instead of plain strings where possible
- Provide actionable information for debugging

 `ollama`: Uses local Ollama installation

## Development Environment

This project uses Nix for reproducible builds and development environments. The
`flake.nix` provides all necessary dependencies. You are always running in the relevant nix environment.

## Testing

The project uses **snapshot testing** via the `insta` crate for all test assertions. This testing paradigm provides deterministic, maintainable tests that capture expected behavior through literal value snapshots.

### Snapshot Testing Approach

All tests follow these principles:
- **Single assertion per test**: Each test has exactly one `insta::assert_snapshot!()` or `insta::assert_json_snapshot!()` call
- **Deterministic snapshots**: Dynamic data (timestamps, file sizes, temp paths) is normalized to ensure reproducible results
- **Literal value snapshots**: Snapshots contain only concrete, expected values without variables
- **Offline resilience**: All tests must pass in offline environments (CI, restricted networks) by using dual-snapshot patterns or graceful degradation

 in `tests/golden_output/`

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test file
cargo test --test <test_name>

# Review and accept snapshot changes
cargo insta review

# Auto-accept all snapshot changes (use carefully)
cargo insta accept
```

### Snapshot Management

- Snapshots are stored in `src/snapshots/` (unit tests) and `tests/snapshots/` (integration tests)
- When test behavior changes, run `cargo insta review` to inspect differences
- Accept valid changes with `cargo insta accept` or reject with `cargo insta reject`
- Never commit `.snap.new` files - these are pending snapshot updates

## Version control

This project uses jujutsu `jj` for version control

## Project Structure

The project is organized as a Rust workspace with both a binary and library crate.
Polars (`LazyFrame` / `Expr`) is the core primitive: the entire vault is ingested
into a single LazyFrame, and the expression language compiles to polars `Expr` at
query-build time — there is no per-row runtime evaluator.

```
src/
  lib.rs          - Library entry point (exposes all modules as `crabase_lib`)
  main.rs         - Binary entry point (CLI parsing + orchestration)
  error.rs        - `CrabaseError` enum using thiserror (includes Polars variant)
  base_file.rs    - `.base` file YAML parsing: FilterNode, View, BaseFile
  vault.rs        - Walks the vault, parses frontmatter, infers a column dtype
                    for each frontmatter key (Int64 / Float64 / Boolean / Date /
                    Datetime / List[String] / String), and assembles every file
                    into a single LazyFrame plus a VaultSchema describing it
  filter.rs       - Compiles a FilterNode tree into a polars `Expr` predicate
  query.rs        - Orchestrator: filter → sort_by_exprs → limit → select → collect
  output.rs       - DataFrame → CSV (`write_csv`) and TOON (`write_toon`) writers.
                    CSV uses custom row iteration matching the legacy quoting
                    rules (Polars's built-in CsvWriter is NOT used; preserves
                    "5" vs "5.0", list comma-joining, etc.). TOON builds a
                    serde_json::Value array of flat row objects and hands it to
                    `toon_format::encode_default`; list cells are joined to
                    strings so the encoder picks the compact tabular header
                    form `[N]{col1,col2,...}:`.
  expr/
    mod.rs        - Re-exports
    lexer.rs      - Spanned tokenizer for expression language
    ast.rs        - Typed AST with spans, literals, and identifiers
    parser.rs     - Pratt parser for expressions and postfix chains
    translate.rs  - AST → polars Expr translator. Tracks an InferredType through
                    every node so methods route to the right namespace
                    (str/list/dt), formulas inline at compile time with cycle
                    detection, .map() callbacks become list.eval with the `value`
                    binding mapped to col(""), file.hasTag/inFolder/hasLink
                    expand into list.eval-based polars predicates.
tests/
  integration.rs  - Integration tests using insta snapshot testing. The
                    `eval_expr_with_inputs` helper evaluates an expression on a
                    one-row LazyFrame so per-expression tests are still concise.
                    Property tests use proptest against the same helper.
  fixtures/
    vault/        - Small test vault with .md files in Church/Sermons/ and Notes/
    test.base     - Test .base file with folder filter and table view
  snapshots/      - Insta snapshot files (committed)
```

## CLI Usage

```
crabase base:query file=<path-relative-to-vault> format=csv|toon [vault=<vault_root>] [view=<view_name>]
```

- `file=` is the path to the `.base` file, relative to the vault root
- `vault=` defaults to the current working directory
- `view=` selects which view; defaults to the first view in the file
- `format=csv` (default) and `format=toon` are supported

## Key Design Decisions

- `FilterNode` is an ADT (And/Or/Not/Expr) parsed from YAML, compiled into a
  single polars `Expr` predicate by `filter::filter_node_to_expr`.
- Expression language uses a Pratt parser and a typed AST with source spans.
  The runtime is **polars Expr**, not a custom `Value`-walking interpreter —
  `expr::translate::translate` rewrites the AST into a polars expression once,
  and polars handles evaluation against the LazyFrame.
- The vault becomes a single LazyFrame. Each frontmatter key gets its own
  typed column; the inferred dtype is the union-of-observations.
- Reserved metadata columns are `file_path`, `file_name` (stem, no extension),
  `file_folder`, `file_ext`, `file_size`, `file_ctime`, `file_mtime`,
  `file_tags`, `file_links`. A frontmatter key colliding with any of these is
  remapped to `note_<key>`.
- The `title` column is special: outputs `[[path/to/file.md| Display Text]]`
  (note the space after `|`). Built via `concat_str` in `query::column_to_expr`.
- YAML frontmatter wikilinks (e.g., `[[All Souls]]`) stay as strings. The
  dtype inference deliberately does NOT strip `[[ ]]` wrappers when probing
  for date-like strings; only bare `YYYY-MM-DD` / `YYYY-MM-DD HH:MM:SS`
  becomes Date / Datetime. The `date()` *function* (when explicitly called)
  does strip wikilink wrappers.
- Formula references resolve at translate time: `formula.X` inlines the
  formula's AST into the caller's expression. Cycle detection happens during
  this inlining via a `formula_stack` on `TranslateCtx`.
- Truthiness: polars's null-as-null semantics are coerced to the
  expression-language's null-as-false by wrapping every consumed boolean in
  `.fill_null(false)`. The `truthy()` helper in `translate.rs` does this
  based on the receiver's `InferredType`.
- `value == null` and `value != null` translate to `.is_null()` /
  `.is_not_null()` rather than polars equality (which yields null on null
  inputs and silently misbehaves).
- The custom CSV writer (`output::write_csv`) iterates DataFrame rows and
  matches the legacy formatting: empty cells for null, integer-valued floats
  printed as integers, list columns joined with `", "`, quoting only when a
  cell contains `, " \n \r`.
