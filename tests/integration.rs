use polars::prelude::*;
use proptest::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

fn fixtures_vault() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("vault")
}

fn fixtures_base(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn run_query(vault: &std::path::Path, base_path: &std::path::Path, view: Option<&str>) -> String {
    use crabase_lib::{base_file::BaseFile, output::write_csv, query::execute_query};
    let content = std::fs::read_to_string(base_path).expect("read base file");
    let base_file = BaseFile::parse(&content).expect("parse base file");
    let view_obj = base_file.get_view(view).expect("get view");
    let columns = view_obj.order.clone().unwrap_or_default();
    let df = execute_query(vault, &base_file, view_obj).expect("execute query");
    let mut out = Vec::new();
    write_csv(&mut out, &columns, &df, &base_file).expect("write csv");
    String::from_utf8(out).expect("utf8 output")
}

fn run_query_toon(
    vault: &std::path::Path,
    base_path: &std::path::Path,
    view: Option<&str>,
) -> String {
    use crabase_lib::{base_file::BaseFile, output::write_toon, query::execute_query};
    let content = std::fs::read_to_string(base_path).expect("read base file");
    let base_file = BaseFile::parse(&content).expect("parse base file");
    let view_obj = base_file.get_view(view).expect("get view");
    let columns = view_obj.order.clone().unwrap_or_default();
    let df = execute_query(vault, &base_file, view_obj).expect("execute query");
    let mut out = Vec::new();
    write_toon(&mut out, &columns, &df, &base_file).expect("write toon");
    String::from_utf8(out).expect("utf8 output")
}

/// Evaluate a single expression against a 1-row LazyFrame containing the given
/// named values. Returns the result rendered with the same display rules used
/// in the CSV writer.
fn eval_expr_with_inputs(expr_str: &str, inputs: Vec<(&str, AnyValue<'static>)>) -> String {
    use crabase_lib::expr::{TranslateCtx, parse, translate};
    use crabase_lib::vault::VaultSchema;

    let mut columns: Vec<Column> = Vec::new();
    let mut frontmatter_keys: HashMap<String, String> = HashMap::new();
    for (name, val) in inputs {
        let series = Series::from_any_values(name.into(), &[val], true).expect("series");
        columns.push(series.into_column());
        frontmatter_keys.insert(name.to_string(), name.to_string());
    }
    // Polars requires at least one column with a known length.
    if columns.is_empty() {
        columns.push(Column::new("__crabase_anchor__".into(), &[0i64]));
    }
    let df = DataFrame::new(columns).expect("dataframe");
    let schema_ref = df.schema().clone();
    let schema = VaultSchema {
        schema: schema_ref,
        frontmatter_keys,
        df: std::sync::Arc::new(df.clone()),
    };
    let formulas: HashMap<String, String> = HashMap::new();
    let ctx = TranslateCtx::new(&schema, &formulas);
    let ast = parse(expr_str).expect("parse");
    let translated = translate(&ast, &ctx).expect("translate");
    let result = df
        .lazy()
        .select([translated.expr.alias("__crabase_result__")])
        .collect()
        .expect("collect");
    let col = result.column("__crabase_result__").expect("result col");
    let series = col.as_materialized_series();
    let v = series.get(0).expect("row 0");
    format_for_test(&v)
}

fn eval_expr_with_formulas(
    expr_str: &str,
    formulas_vec: Vec<(&str, &str)>,
    inputs: Vec<(&str, AnyValue<'static>)>,
) -> Result<String, String> {
    use crabase_lib::expr::{TranslateCtx, parse, translate};
    use crabase_lib::vault::VaultSchema;

    let mut columns: Vec<Column> = Vec::new();
    let mut frontmatter_keys: HashMap<String, String> = HashMap::new();
    for (name, val) in inputs {
        let series =
            Series::from_any_values(name.into(), &[val], true).map_err(|e| e.to_string())?;
        columns.push(series.into_column());
        frontmatter_keys.insert(name.to_string(), name.to_string());
    }
    if columns.is_empty() {
        columns.push(Column::new("__crabase_anchor__".into(), &[0i64]));
    }
    let df = DataFrame::new(columns).map_err(|e| e.to_string())?;
    let schema_ref = df.schema().clone();
    let schema = VaultSchema {
        schema: schema_ref,
        frontmatter_keys,
        df: std::sync::Arc::new(df.clone()),
    };
    let formulas: HashMap<String, String> = formulas_vec
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let ctx = TranslateCtx::new(&schema, &formulas);
    let ast = parse(expr_str).map_err(|e| e.to_string())?;
    let translated = translate(&ast, &ctx).map_err(|e| e.to_string())?;
    let result = df
        .lazy()
        .select([translated.expr.alias("__crabase_result__")])
        .collect()
        .map_err(|e| e.to_string())?;
    let col = result
        .column("__crabase_result__")
        .map_err(|e| e.to_string())?;
    let series = col.as_materialized_series();
    let v = series.get(0).map_err(|e| e.to_string())?;
    Ok(format_for_test(&v))
}

fn eval_expr(expr_str: &str) -> String {
    eval_expr_with_inputs(expr_str, vec![])
}

fn format_for_test(v: &AnyValue<'_>) -> String {
    match v {
        AnyValue::Null => String::new(),
        AnyValue::Boolean(b) => b.to_string(),
        AnyValue::String(s) => s.to_string(),
        AnyValue::StringOwned(s) => s.to_string(),
        AnyValue::Int8(n) => n.to_string(),
        AnyValue::Int16(n) => n.to_string(),
        AnyValue::Int32(n) => n.to_string(),
        AnyValue::Int64(n) => n.to_string(),
        AnyValue::UInt8(n) => n.to_string(),
        AnyValue::UInt16(n) => n.to_string(),
        AnyValue::UInt32(n) => n.to_string(),
        AnyValue::UInt64(n) => n.to_string(),
        AnyValue::Float32(f) => format_float(*f as f64),
        AnyValue::Float64(f) => format_float(*f),
        AnyValue::Date(days) => {
            let base = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            (base + chrono::Duration::days(*days as i64))
                .format("%Y-%m-%d")
                .to_string()
        }
        AnyValue::Datetime(micros, tu, _) => {
            let (secs, nsec) = micros_split(*micros, *tu);
            chrono::DateTime::from_timestamp(secs, nsec)
                .map(|dt| dt.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default()
        }
        AnyValue::DatetimeOwned(micros, tu, _) => {
            let (secs, nsec) = micros_split(*micros, *tu);
            chrono::DateTime::from_timestamp(secs, nsec)
                .map(|dt| dt.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default()
        }
        AnyValue::Duration(n, _) => n.to_string(),
        AnyValue::List(s) => (0..s.len())
            .map(|i| s.get(i).map(|v| format_for_test(&v)).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", "),
        other => format!("{other}"),
    }
}

fn micros_split(value: i64, tu: TimeUnit) -> (i64, u32) {
    match tu {
        TimeUnit::Nanoseconds => (value / 1_000_000_000, (value % 1_000_000_000) as u32),
        TimeUnit::Microseconds => (value / 1_000_000, ((value % 1_000_000) * 1_000) as u32),
        TimeUnit::Milliseconds => (value / 1_000, ((value % 1_000) * 1_000_000) as u32),
    }
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return String::new();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

// ---------- CSV-output snapshot tests (operate end-to-end) ----------

#[test]
fn test_sermons_query_csv() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("test.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_in_folder_filter_excludes_notes() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("test.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!((!output.contains("random-note")).to_string());
}

#[test]
fn test_null_arithmetic_propagates_null() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("null_arith.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_formula_sort_is_ignored_and_null_formula_csv_cell_is_blank() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("formula_sort_ignored.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_sort_ties_fall_back_to_file_name() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("name_tiebreak.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_is_empty_includes_null_cells() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("is_empty_null.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_neq_literal_includes_null_cells() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("neq_null.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

// ---------- TOON-output snapshot tests ----------

#[test]
fn test_sermons_query_toon() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("test.base");
    let output = run_query_toon(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_toon_null_arithmetic_propagates_null() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("null_arith.base");
    let output = run_query_toon(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_toon_formula_columns_have_stripped_headers() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("formula_sort_ignored.base");
    let output = run_query_toon(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

#[test]
fn test_base_views() {
    use crabase_lib::base_file::BaseFile;
    let base_path = fixtures_base("test.base");
    let content = std::fs::read_to_string(&base_path).expect("read base file");
    let base_file = BaseFile::parse(&content).expect("parse base file");
    let output: Vec<String> = base_file
        .views
        .iter()
        .map(|v| v.name.clone().unwrap_or_else(|| "(unnamed)".to_string()))
        .collect();
    insta::assert_snapshot!(output.join("\n"));
}

#[test]
fn test_column_header_formula_prefix_stripped() {
    use crabase_lib::base_file::BaseFile;
    use crabase_lib::output::write_csv;
    use crabase_lib::query::execute_query;
    let vault = fixtures_vault();
    let base_path = fixtures_base("test.base");
    let content = std::fs::read_to_string(&base_path).expect("read base file");
    let base_file = BaseFile::parse(&content).expect("parse base file");
    let view = base_file.get_view(None).expect("get view");
    let columns = view.order.clone().unwrap_or_default();
    let df = execute_query(&vault, &base_file, view).expect("execute query");
    let mut out = Vec::new();
    write_csv(&mut out, &columns, &df, &base_file).expect("write csv");
    let csv = String::from_utf8(out).expect("utf8");
    let header = csv.lines().next().unwrap_or("");
    insta::assert_snapshot!(
        (!header.contains("formula.") && !header.contains("file.")).to_string()
    );
}

// ---------- Filter / vault unit tests ----------

#[test]
fn test_filter_node_and() {
    use crabase_lib::base_file::FilterNode;
    use crabase_lib::expr::TranslateCtx;
    use crabase_lib::filter::filter_node_to_expr;
    use crabase_lib::vault::scan_vault_to_lazyframe;

    let vault = fixtures_vault();
    let (lf, schema) = scan_vault_to_lazyframe(&vault).expect("scan");
    let formulas: HashMap<String, String> = HashMap::new();
    let ctx = TranslateCtx::new(&schema, &formulas);

    let node = FilterNode::And(vec![
        FilterNode::Expr("session == 1130".to_string()),
        FilterNode::Expr("wc > 500".to_string()),
    ]);
    let pred = filter_node_to_expr(&node, &ctx).expect("compile");
    let df = lf
        .filter(pred)
        .filter(
            col("file_name")
                .str()
                .contains_literal(lit("House of Blood")),
        )
        .collect()
        .expect("collect");
    let count = df.height();
    insta::assert_snapshot!((count == 1).to_string());
}

#[test]
fn test_file_name_no_extension() {
    use crabase_lib::vault::scan_vault_to_lazyframe;
    let vault = fixtures_vault();
    let (lf, _) = scan_vault_to_lazyframe(&vault).expect("scan");
    let df = lf
        .filter(
            col("file_name")
                .str()
                .contains_literal(lit("House of Blood")),
        )
        .select([col("file_name")])
        .collect()
        .expect("collect");
    let s = df.column("file_name").unwrap().as_materialized_series();
    insta::assert_snapshot!(s.get(0).unwrap().get_str().unwrap_or("").to_string());
}

// ---------- Expression-language tests (evaluate on a 1-row LazyFrame) ----------

#[test]
fn test_expression_comparison() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "session == 1130",
        vec![("session", AnyValue::Int64(1130))]
    ));
}

#[test]
fn test_expression_string_concat() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "title + \" World\"",
        vec![("title", AnyValue::StringOwned("Hello".into()))]
    ));
}

#[test]
fn test_date_parse_year() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27\").year"));
}

#[test]
fn test_date_parse_month() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27\").month"));
}

#[test]
fn test_date_parse_day() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27\").day"));
}

#[test]
fn test_date_add_days() {
    insta::assert_snapshot!(eval_expr("date(\"2025-01-01\") + \"1d\""));
}

#[test]
fn test_date_add_months() {
    insta::assert_snapshot!(eval_expr("date(\"2025-01-31\") + \"1M\""));
}

#[test]
fn test_date_sub_duration() {
    insta::assert_snapshot!(eval_expr("date(\"2025-01-03\") - \"2d\""));
}

#[test]
fn test_date_sub_date() {
    insta::assert_snapshot!(eval_expr("date(\"2025-01-02\") - date(\"2025-01-01\")"));
}

#[test]
fn test_date_comparison() {
    insta::assert_snapshot!(eval_expr("date(\"2025-01-02\") > date(\"2025-01-01\")"));
}

#[test]
fn test_date_format() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27\").format(\"YYYY/MM/DD\")"));
}

#[test]
fn test_date_strip_time() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27 15:30:00\").date().time()"));
}

#[test]
fn test_date_is_empty() {
    insta::assert_snapshot!(eval_expr("date(\"2025-01-01\").isEmpty()"));
}

#[test]
fn test_today_is_date_type() {
    insta::assert_snapshot!(eval_expr("today().isType(\"date\")"));
}

#[test]
fn test_date_datetime_parse() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27 15:30:00\").hour"));
}

#[test]
fn test_date_wikilink_parse() {
    insta::assert_snapshot!(eval_expr("date(\"[[2025-01-15]]\").day"));
}

#[test]
fn test_date_diff_days_property() {
    insta::assert_snapshot!(eval_expr(
        "(date(\"2025-01-11\") - date(\"2025-01-01\")).days"
    ));
}

#[test]
fn test_formula_bracket_access() {
    let result = eval_expr_with_formulas("formula[\"double\"]", vec![("double", "6 * 7")], vec![])
        .expect("eval");
    insta::assert_snapshot!(result);
}

#[test]
fn test_formula_cycle_detection() {
    let err = eval_expr_with_formulas(
        "formula.a",
        vec![("a", "formula.b"), ("b", "formula.a")],
        vec![],
    )
    .unwrap_err();
    insta::assert_snapshot!(err, @"Expression eval error: Formula cycle detected: a -> b -> a");
}

#[test]
fn test_list_map_value_variable() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "[n].map(value.toString() + \" days\")",
        vec![("n", AnyValue::Int64(5))]
    ));
}

#[test]
fn test_list_map_null_passthrough() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "[n].map(if(value==null, null, value.toString() + \" days\"))",
        vec![("n", AnyValue::Null)]
    ));
}

#[test]
fn test_parser_ast_includes_spans() {
    let ast = crabase_lib::expr::parse("title + author.upper()").expect("parse");
    insta::assert_snapshot!(format!("{ast:#?}"), @r#"
    Expr {
        kind: Binary {
            op: Add,
            left: Expr {
                kind: Variable(
                    Ident(
                        "title",
                    ),
                ),
                span: Span {
                    start: 0,
                    end: 5,
                },
            },
            right: Expr {
                kind: Call {
                    callee: Expr {
                        kind: Member {
                            object: Expr {
                                kind: Variable(
                                    Ident(
                                        "author",
                                    ),
                                ),
                                span: Span {
                                    start: 8,
                                    end: 14,
                                },
                            },
                            field: Ident(
                                "upper",
                            ),
                        },
                        span: Span {
                            start: 8,
                            end: 20,
                        },
                    },
                    args: [],
                },
                span: Span {
                    start: 8,
                    end: 22,
                },
            },
        },
        span: Span {
            start: 0,
            end: 22,
        },
    }
    "#);
}

#[test]
fn test_parser_reports_precise_error_positions() {
    let error = crabase_lib::expr::parse("1.2.3")
        .map(|_| String::new())
        .unwrap_or_else(|error| error.to_string());
    insta::assert_snapshot!(error, @"Expression parse error: Expected identifier after '.' at 4, got Number(3.0)");
}

// ---------- Tests covering the `q` (fixes) changeset ----------

// Regex literal `/pattern/` routes `replace()` to regex mode; the same
// expression with a string pattern stays literal (so `.` doesn't act as a
// wildcard).

#[test]
fn test_replace_regex_literal_uses_regex_mode() {
    insta::assert_snapshot!(eval_expr("\"abc123def\".replace(/\\d+/, \"X\")"));
}

#[test]
fn test_replace_string_pattern_is_literal() {
    // The `.` is a literal dot, not a wildcard, when the pattern is a string.
    insta::assert_snapshot!(eval_expr("\"a.b.c\".replace(\".\", \"-\")"));
}

#[test]
fn test_division_still_works_after_identifier_or_number() {
    // The lookbehind rule must keep `/` as division when the previous token
    // is a number or identifier (not the start of a regex literal).
    insta::assert_snapshot!(eval_expr("10 / 2"));
}

#[test]
fn test_division_always_returns_float() {
    // Obsidian Bases: `5/2` is `2.5`, not `2`.
    insta::assert_snapshot!(eval_expr("5 / 2"));
}

// `if(cond, a, b)` should promote numeric branches to a common Float64, not
// fall through to string-casting both sides.

#[test]
fn test_if_numeric_branches_remain_numeric() {
    // If the result were string-cast, the trailing `+ 0.5` would yield a
    // string concat or polars error rather than 0.5.
    insta::assert_snapshot!(eval_expr("if(true, 0, 1.5) + 0.5"));
}

// Case-insensitive string predicates (Obsidian semantics).

#[test]
fn test_contains_is_case_insensitive() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "title.contains(\"hello\")",
        vec![("title", AnyValue::StringOwned("Hello World".into()))],
    ));
}

#[test]
fn test_starts_with_is_case_insensitive() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "title.startsWith(\"HELLO\")",
        vec![("title", AnyValue::StringOwned("hello world".into()))],
    ));
}

#[test]
fn test_ends_with_is_case_insensitive() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "title.endsWith(\"WORLD\")",
        vec![("title", AnyValue::StringOwned("hello world".into()))],
    ));
}

#[test]
fn test_contains_any_matches_any_argument() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "title.containsAny(\"baz\", \"WORLD\")",
        vec![("title", AnyValue::StringOwned("hello world".into()))],
    ));
}

#[test]
fn test_contains_any_returns_false_when_no_match() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "title.containsAny(\"baz\", \"qux\")",
        vec![("title", AnyValue::StringOwned("hello world".into()))],
    ));
}

// `.title()` titlecases each word and strips wikilink wrappers first.

#[test]
fn test_title_case_basic() {
    insta::assert_snapshot!(eval_expr("\"hello world\".title()"));
}

#[test]
fn test_title_case_strips_wikilink_wrapper() {
    insta::assert_snapshot!(eval_expr("\"[[foo bar baz]]\".title()"));
}

#[test]
fn test_title_case_treats_digits_as_word_break() {
    // "25-19 to" titles as "25-19 To" because non-alpha chars (digits, dashes)
    // are word separators.
    insta::assert_snapshot!(eval_expr("\"25-19 to\".title()"));
}

// `link()` is idempotent on already-bracketed input.

#[test]
fn test_link_is_idempotent_on_bracketed_input() {
    insta::assert_snapshot!(eval_expr("link(\"[[Foo]]\")"));
}

#[test]
fn test_link_with_display_idempotent_on_bracketed_input() {
    // Even with a display arg, an already-linked path stays verbatim.
    insta::assert_snapshot!(eval_expr("link(\"[[Foo]]\", \"Bar\")"));
}

#[test]
fn test_link_with_display_wraps_plain_path() {
    insta::assert_snapshot!(eval_expr("link(\"Foo\", \"Bar\")"));
}

// `date()` now accepts ISO datetime (T-separated), ISO date, or
// space-separated datetime — first match wins.

#[test]
fn test_date_iso_t_separated_datetime() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27T15:30:00\").hour"));
}

// List operations: concatenation via `+`, slice with negative end, null
// propagation through `.length`.

#[test]
fn test_list_plus_list_concatenates() {
    insta::assert_snapshot!(eval_expr("[1, 2] + [3, 4]"));
}

#[test]
fn test_list_slice_positive_bounds() {
    insta::assert_snapshot!(eval_expr("[1, 2, 3, 4, 5].slice(1, 3)"));
}

#[test]
fn test_list_slice_negative_end_drops_tail() {
    insta::assert_snapshot!(eval_expr("[1, 2, 3, 4].slice(0, -1)"));
}

#[test]
fn test_list_length_on_empty_list_is_zero() {
    // Empty list length is 0 (the null-propagation case for an absent list
    // column is covered by the vault-level backlinks fixture).
    insta::assert_snapshot!(eval_expr_with_inputs(
        "attendees.length",
        vec![(
            "attendees",
            AnyValue::List(Series::new_empty("".into(), &DataType::String)),
        )],
    ));
}

// Bare `file` resolves to the file_path column; `file.basename` aliases
// `file.name`.

#[test]
fn test_bare_file_resolves_to_file_path() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "file",
        vec![("file_path", AnyValue::StringOwned("Notes/foo.md".into()),)],
    ));
}

#[test]
fn test_file_basename_aliases_file_name() {
    insta::assert_snapshot!(eval_expr_with_inputs(
        "file.basename",
        vec![("file_name", AnyValue::StringOwned("foo".into()))],
    ));
}

// Wikilink anchor in target is dropped during link extraction.

#[test]
fn test_obsidian_sort_key_numeric_runs_sort_numerically() {
    // The natural sort key must zero-pad digit runs so "Exodus 9" sorts
    // before "Exodus 19" (without padding, "19" sorts before "9").
    use crabase_lib::vault::obsidian_sort_key;
    let mut items = vec![
        "Exodus 19".to_string(),
        "Exodus 9".to_string(),
        "Exodus 11".to_string(),
    ];
    items.sort_by_key(|s| obsidian_sort_key(s));
    insta::assert_snapshot!(items.join("\n"));
}

#[test]
fn test_obsidian_sort_key_collapses_punctuation() {
    // Apostrophes/periods become whitespace so they don't fragment the order.
    use crabase_lib::vault::obsidian_sort_key;
    let mut items = vec![
        "D. E. Shaw".to_string(),
        "d'Vijff Vlieghen".to_string(),
        "Daffy Duck".to_string(),
    ];
    items.sort_by_key(|s| obsidian_sort_key(s));
    insta::assert_snapshot!(items.join("\n"));
}

#[test]
fn test_natural_sort_key_preserves_punctuation() {
    // Path-style key keeps punctuation distinctions: "Study Notes.md" vs
    // "Study.md" — space (0x20) < period (0x2E), so Notes sorts first.
    use crabase_lib::vault::natural_sort_key;
    let mut items = vec!["Study.md".to_string(), "Study Notes.md".to_string()];
    items.sort_by_key(|s| natural_sort_key(s));
    insta::assert_snapshot!(items.join("\n"));
}

// ---------- Vault-level tests covering the `q` (fixes) changeset ----------

// Backlinks exercise multiple link-resolution rules at once:
//   - body wikilinks (LinkerBody)
//   - pure-frontmatter wikilinks counted as both links and backlinks (LinkerFrontmatter)
//   - inline frontmatter wikilinks counted only as backlinks (LinkerInlineFrontmatter)
//   - wikilink with `#anchor` / `|alias` resolves to the target file (LinkerWithAnchor)
//   - wikilinks inside fenced code regions are skipped (LinkerInCode → NOT in list)
//   - canvas-file mentions contribute (diagram.canvas)

#[test]
fn test_backlinks_includes_body_frontmatter_inline_anchor_and_canvas() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("backlinks.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

// `file.links` only contains *pure* frontmatter wikilinks and body wikilinks;
// inline frontmatter wikilinks and code-fenced ones are omitted.

#[test]
fn test_file_links_excludes_inline_frontmatter_and_code_regions() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("backlinks_links.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

// Numeric runs in file names sort numerically (Exodus 1 → 9 → 11, not 1 → 11 → 9).

#[test]
fn test_natural_sort_orders_numeric_filenames() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("natural_sort.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

// When no `sort` or `groupBy` is set, the query falls back to sorting by the
// first column of `view.order`. Here `timeEstimate` ascending: 1, 1, 2.

#[test]
fn test_implicit_sort_uses_first_order_column() {
    let vault = fixtures_vault();
    let base_path = fixtures_base("implicit_sort.base");
    let output = run_query(&vault, &base_path, None);
    insta::assert_snapshot!(output);
}

// Header transformation aliases each `file.*` reserved column to Obsidian's
// friendly name. We only check the header row so test stays insensitive to
// mtime/ctime/size values.

#[test]
fn test_file_metadata_column_headers_use_friendly_aliases() {
    use crabase_lib::base_file::BaseFile;
    use crabase_lib::output::write_csv;
    use crabase_lib::query::execute_query;
    let vault = fixtures_vault();
    let base_path = fixtures_base("header_aliases.base");
    let content = std::fs::read_to_string(&base_path).expect("read base file");
    let base_file = BaseFile::parse(&content).expect("parse base file");
    let view = base_file.get_view(None).expect("get view");
    let columns = view.order.clone().unwrap_or_default();
    let df = execute_query(&vault, &base_file, view).expect("execute query");
    let mut out = Vec::new();
    write_csv(&mut out, &columns, &df, &base_file).expect("write csv");
    let csv = String::from_utf8(out).expect("utf8");
    let header = csv.lines().next().unwrap_or("").to_string();
    insta::assert_snapshot!(header);
}

// CSV datetime cells now use ISO `T`-separator instead of a space. Build a
// 1-row DataFrame containing a Datetime column and inspect the CSV cell.

#[test]
fn test_csv_datetime_cell_uses_iso_t_separator() {
    use crabase_lib::base_file::BaseFile;
    use crabase_lib::output::write_csv;
    let base_file =
        BaseFile::parse("views:\n  - type: table\n    name: \"X\"\n    order:\n      - when\n")
            .expect("parse base");
    // 2025-04-27T15:30:00 UTC, in microseconds since epoch.
    let micros: i64 = chrono::NaiveDate::from_ymd_opt(2025, 4, 27)
        .and_then(|d| d.and_hms_opt(15, 30, 0))
        .map(|dt| dt.and_utc().timestamp_micros())
        .expect("timestamp");
    let series = Series::new("when".into(), &[micros])
        .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
        .expect("cast datetime");
    let df = DataFrame::new(vec![series.into_column()]).expect("dataframe");
    let mut out = Vec::new();
    write_csv(&mut out, &["when".to_string()], &df, &base_file).expect("write csv");
    let csv = String::from_utf8(out).expect("utf8");
    insta::assert_snapshot!(csv);
}

proptest! {
    #[test]
    fn prop_addition_respects_precedence(
        a in -500i32..500,
        b in -500i32..500,
        c in -500i32..500,
    ) {
        let result = eval_expr(&format!("{a} + {b} * {c}"));
        prop_assert_eq!(result, (a + b * c).to_string());
    }

    #[test]
    fn prop_string_reverse_is_involution(input in "[a-zA-Z0-9 ]*") {
        let result = eval_expr_with_inputs(
            "value.reverse().reverse()",
            vec![("value", AnyValue::StringOwned(input.clone().into()))],
        );
        prop_assert_eq!(result, input);
    }

    #[test]
    fn prop_subtraction_is_left_associative(
        a in -500i32..500,
        b in -500i32..500,
        c in -500i32..500,
    ) {
        let result = eval_expr(&format!("{a} - {b} - {c}"));
        prop_assert_eq!(result, (a - b - c).to_string());
    }
}
