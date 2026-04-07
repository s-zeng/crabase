use proptest::prelude::*;
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
    let rows = execute_query(vault, &base_file, view_obj).expect("execute query");
    let mut out = Vec::new();
    write_csv(&mut out, &columns, &rows, &base_file).expect("write csv");
    String::from_utf8(out).expect("utf8 output")
}

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
fn test_filter_node_and() {
    use crabase_lib::base_file::FilterNode;
    use crabase_lib::filter::eval_filter;
    use std::collections::HashMap;

    let node = FilterNode::And(vec![
        FilterNode::Expr("session == 1130".to_string()),
        FilterNode::Expr("wc > 500".to_string()),
    ]);

    let vault = fixtures_vault();
    let files = crabase_lib::vault::scan_vault(&vault).expect("scan vault");
    let sermon = files
        .iter()
        .find(|f| f.name.contains("House of Blood"))
        .expect("find sermon");

    let result = eval_filter(&node, sermon, &HashMap::new()).expect("eval filter");
    insta::assert_snapshot!(result.to_string());
}

#[test]
fn test_expression_comparison() {
    use crabase_lib::expr::{EvalContext, eval, parse};
    use std::collections::HashMap;

    let ctx = EvalContext::new(
        HashMap::new(),
        {
            let mut m = HashMap::new();
            m.insert(
                "session".to_string(),
                crabase_lib::expr::eval::Value::Number(1130.0),
            );
            m
        },
        HashMap::new(),
    );

    let ast = parse("session == 1130").expect("parse");
    let val = eval(&ast, &ctx).expect("eval");
    insta::assert_snapshot!(val.to_display());
}

#[test]
fn test_expression_string_concat() {
    use crabase_lib::expr::{EvalContext, eval, parse};
    use std::collections::HashMap;

    let ctx = EvalContext::new(
        HashMap::new(),
        {
            let mut m = HashMap::new();
            m.insert(
                "title".to_string(),
                crabase_lib::expr::eval::Value::Str("Hello".to_string()),
            );
            m
        },
        HashMap::new(),
    );

    let ast = parse("title + \" World\"").expect("parse");
    let val = eval(&ast, &ctx).expect("eval");
    insta::assert_snapshot!(val.to_display());
}

#[test]
fn test_null_arithmetic_propagates_null() {
    // Reproduces: "Cannot subtract Number(7.0) and Null"
    // When a note is missing a numeric property and a formula does arithmetic on it,
    // the result should be Null (not an error).
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

fn eval_expr(expr_str: &str) -> String {
    use crabase_lib::expr::{EvalContext, eval, parse};
    use std::collections::HashMap;
    let ctx = EvalContext::new(HashMap::new(), HashMap::new(), HashMap::new());
    let ast = parse(expr_str).expect("parse");
    eval(&ast, &ctx).expect("eval").to_display()
}

fn eval_expr_result(
    expr_str: &str,
    formulas: Vec<(&str, &str)>,
    note_props: Vec<(&str, crabase_lib::expr::eval::Value)>,
) -> Result<String, String> {
    use crabase_lib::expr::{EvalContext, eval, parse};
    use std::collections::HashMap;

    let formula_map: HashMap<String, String> = formulas
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let note_map: HashMap<String, crabase_lib::expr::eval::Value> = note_props
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let ctx = EvalContext::new(HashMap::new(), note_map, formula_map);
    let ast = parse(expr_str).map_err(|error| error.to_string())?;
    eval(&ast, &ctx)
        .map(|value| value.to_display())
        .map_err(|error| error.to_string())
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

#[test]
fn test_date_datetime_parse() {
    insta::assert_snapshot!(eval_expr("date(\"2025-04-27 15:30:00\").hour"));
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

fn eval_expr_with_formulas(
    expr_str: &str,
    formulas: Vec<(&str, &str)>,
    note_props: Vec<(&str, crabase_lib::expr::eval::Value)>,
) -> String {
    use crabase_lib::expr::{EvalContext, eval, parse};
    use std::collections::HashMap;
    let formula_map: HashMap<String, String> = formulas
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let note_map: HashMap<String, crabase_lib::expr::eval::Value> = note_props
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let ctx = EvalContext::new(HashMap::new(), note_map, formula_map);
    let ast = parse(expr_str).expect("parse");
    eval(&ast, &ctx).expect("eval").to_display()
}

#[test]
fn test_date_wikilink_parse() {
    // date() must strip [[...]] Obsidian wikilink brackets
    insta::assert_snapshot!(eval_expr("date(\"[[2025-01-15]]\").day"));
}

#[test]
fn test_date_diff_days_property() {
    // (date - date).days should convert ms to integer days
    insta::assert_snapshot!(eval_expr(
        "(date(\"2025-01-11\") - date(\"2025-01-01\")).days"
    ));
}

#[test]
fn test_formula_bracket_access() {
    // formula["name"] must evaluate the named formula
    let result = eval_expr_with_formulas("formula[\"double\"]", vec![("double", "6 * 7")], vec![]);
    insta::assert_snapshot!(result);
}

#[test]
fn test_formula_cycle_detection() {
    let result = eval_expr_result(
        "formula.a",
        vec![("a", "formula.b"), ("b", "formula.a")],
        vec![],
    )
    .unwrap_or_else(|error| error);
    insta::assert_snapshot!(result, @"Expression eval error: Formula cycle detected: a -> b -> a");
}

#[test]
fn test_list_map_value_variable() {
    // [x].map(if(value==null, null, value.toString() + " days"))
    use crabase_lib::expr::eval::Value;
    let result = eval_expr_with_formulas(
        "[n].map(value.toString() + \" days\")",
        vec![],
        vec![("n", Value::Number(5.0))],
    );
    insta::assert_snapshot!(result);
}

#[test]
fn test_list_map_null_passthrough() {
    use crabase_lib::expr::eval::Value;
    let result = eval_expr_with_formulas(
        "[n].map(if(value==null, null, value.toString() + \" days\"))",
        vec![],
        vec![("n", Value::Null)],
    );
    insta::assert_snapshot!(result);
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
    let rows = execute_query(&vault, &base_file, view).expect("execute query");
    let mut out = Vec::new();
    write_csv(&mut out, &columns, &rows, &base_file).expect("write csv");
    let csv = String::from_utf8(out).expect("utf8");
    // Header line must not contain "formula." or "file." prefixes
    let header = csv.lines().next().unwrap_or("");
    insta::assert_snapshot!(
        (!header.contains("formula.") && !header.contains("file.")).to_string()
    );
}

#[test]
fn test_file_name_no_extension() {
    let vault = fixtures_vault();
    let files = crabase_lib::vault::scan_vault(&vault).expect("scan vault");
    let sermon = files
        .iter()
        .find(|f| f.stem.contains("House of Blood"))
        .expect("find sermon");
    let props = sermon.file_props();
    let name = props
        .get("name")
        .cloned()
        .unwrap_or(crabase_lib::expr::eval::Value::Null);
    // file.name should be stem (no .md extension)
    insta::assert_snapshot!(name.to_display());
}

proptest! {
    #[test]
    fn prop_addition_respects_precedence(
        a in -500i32..500,
        b in -500i32..500,
        c in -500i32..500,
    ) {
        use crabase_lib::expr::{eval, parse, EvalContext};
        use std::collections::HashMap;

        let ctx = EvalContext::new(HashMap::new(), HashMap::new(), HashMap::new());
        let ast = parse(&format!("{a} + {b} * {c}")).expect("parse");
        let val = eval(&ast, &ctx).expect("eval");

        prop_assert_eq!(val, crabase_lib::expr::eval::Value::Number((a + b * c) as f64));
    }

    #[test]
    fn prop_string_reverse_is_involution(input in ".*") {
        use crabase_lib::expr::{eval, parse, EvalContext};
        use std::collections::HashMap;

        let ctx = EvalContext::new(
            HashMap::new(),
            [(
                "value".to_string(),
                crabase_lib::expr::eval::Value::Str(input.clone()),
            )]
            .into_iter()
            .collect(),
            HashMap::new(),
        );
        let ast = parse("value.reverse().reverse()").expect("parse");
        let val = eval(&ast, &ctx).expect("eval");

        prop_assert_eq!(val, crabase_lib::expr::eval::Value::Str(input));
    }

    #[test]
    fn prop_subtraction_is_left_associative(
        a in -500i32..500,
        b in -500i32..500,
        c in -500i32..500,
    ) {
        use crabase_lib::expr::{eval, parse, EvalContext};
        use std::collections::HashMap;

        let ctx = EvalContext::new(HashMap::new(), HashMap::new(), HashMap::new());
        let ast = parse(&format!("{a} - {b} - {c}")).expect("parse");
        let val = eval(&ast, &ctx).expect("eval");

        prop_assert_eq!(val, crabase_lib::expr::eval::Value::Number((a - b - c) as f64));
    }
}
