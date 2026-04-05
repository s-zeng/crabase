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
    let base_file = BaseFile::from_str(&content).expect("parse base file");
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
    // The random note in Notes/ should not be in the output
    assert!(!output.contains("random-note"), "random-note should be excluded by folder filter");
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
    use crabase_lib::expr::{eval, parse, EvalContext};
    use std::collections::HashMap;

    let ctx = EvalContext::new(HashMap::new(), {
        let mut m = HashMap::new();
        m.insert("session".to_string(), crabase_lib::expr::eval::Value::Number(1130.0));
        m
    }, HashMap::new());

    let ast = parse("session == 1130").expect("parse");
    let val = eval(&ast, &ctx).expect("eval");
    insta::assert_snapshot!(val.to_display());
}

#[test]
fn test_expression_string_concat() {
    use crabase_lib::expr::{eval, parse, EvalContext};
    use std::collections::HashMap;

    let ctx = EvalContext::new(HashMap::new(), {
        let mut m = HashMap::new();
        m.insert("title".to_string(), crabase_lib::expr::eval::Value::Str("Hello".to_string()));
        m
    }, HashMap::new());

    let ast = parse("title + \" World\"").expect("parse");
    let val = eval(&ast, &ctx).expect("eval");
    insta::assert_snapshot!(val.to_display());
}
