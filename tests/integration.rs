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
        let series = Series::from_any_values(name.into(), &[val], true).map_err(|e| e.to_string())?;
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
        .filter(col("file_name").str().contains_literal(lit("House of Blood")))
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
        .filter(col("file_name").str().contains_literal(lit("House of Blood")))
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
