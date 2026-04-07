Documentation written by Claude Code.

# Review Findings

1. High: Frontmatter date fields never become `Value::Date`, so the new date functionality does not apply to actual note metadata.

   In [`src/vault.rs:73`](/Users/simonzeng/repos/crabase/src/vault.rs#L73), `yaml_to_value()` converts YAML scalars to `Value::Str`/`Value::Number`/etc., but never recognizes date-like frontmatter values as `Value::Date`. That means a filter like `published.year == 2025` evaluates `published` as a string, `published.year` becomes `Null`, and the note is excluded. I reproduced this end-to-end with a note containing `published: 2025-04-27`; the query returned only the CSV header instead of matching the note. This is the main integration path for date fields, so the current implementation makes the feature largely unavailable in real vault data.

2. High: `date()` turns missing optional properties into hard query failures instead of `Null`.

   In [`src/expr/eval.rs:828`](/Users/simonzeng/repos/crabase/src/expr/eval.rs#L828), `date()` accepts only `Value::Str` or `Value::Date`; `Null` produces `date() expects a string, got Null`. In practice formulas over frontmatter commonly run on partially-populated notes, so `date(published).year` now aborts the entire query when `published` is absent instead of behaving like the rest of the evaluator’s null-propagating paths. I reproduced this with a `.base` file containing `formulas: { pub_year: date(published).year }` against a note with no `published` property; the CLI exited with that error. This needs either null-tolerant coercion in `date()` or explicit tests covering sparse note data.

3. Medium: Overloading `Date + String` as duration arithmetic breaks the pre-existing string concatenation behavior for dates.

   In [`src/expr/eval.rs:949`](/Users/simonzeng/repos/crabase/src/expr/eval.rs#L949), any `Value::Date + Value::Str` now goes through `parse_duration_string()`. If the RHS is not a duration literal, evaluation errors before the generic concatenation cases at lines 958-960 can run. I reproduced this with `date("2025-01-01") + " suffix"`, which now fails with `Cannot parse duration: " suffix"`. Before this change, `<any value> + <string>` concatenated via `to_display()`. If duration syntax is meant to be additive sugar, it should be narrower than “all date/string additions,” or existing concatenation expressions will regress.

# Verification

- `cargo test`
- Reproduced the three cases above with temporary vaults via `cargo run -- base:query ...`
