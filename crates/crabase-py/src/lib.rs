use std::path::{Path, PathBuf};

use pyo3::exceptions::{PyFileNotFoundError, PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pyo3_polars::PyDataFrame;

use crabase::base_file::{BaseFile, GroupBy, SortDirection, SortKey, View};
use crabase::error::CrabaseError;
use crabase::query::execute_query;
use crabase::vault::{scan_bases, scan_vault_to_lazyframe};

fn to_py_err(e: CrabaseError) -> PyErr {
    match e {
        CrabaseError::Io(io) => PyFileNotFoundError::new_err(io.to_string()),
        CrabaseError::ViewNotFound(name) => PyKeyError::new_err(format!("view not found: {name}")),
        CrabaseError::NoViews => PyKeyError::new_err("no views found in base file"),
        other => PyValueError::new_err(other.to_string()),
    }
}

fn resolve_vault(vault: Option<PathBuf>) -> PyResult<PathBuf> {
    match vault {
        Some(p) => Ok(p),
        None => std::env::current_dir()
            .map_err(|e| PyValueError::new_err(format!("cannot resolve cwd: {e}"))),
    }
}

fn read_base(vault: &Path, base_file: &str) -> PyResult<BaseFile> {
    let path = vault.join(base_file);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| PyFileNotFoundError::new_err(format!("{}: {e}", path.display())))?;
    BaseFile::parse(&content).map_err(to_py_err)
}

fn direction_str(d: &SortDirection) -> &'static str {
    match d {
        SortDirection::Asc => "ASC",
        SortDirection::Desc => "DESC",
    }
}

fn sort_key_to_dict<'py>(py: Python<'py>, key: &SortKey) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("property", &key.property)?;
    d.set_item("direction", direction_str(&key.direction))?;
    Ok(d)
}

fn group_by_to_dict<'py>(py: Python<'py>, gb: &GroupBy) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("property", &gb.property)?;
    d.set_item("direction", direction_str(&gb.direction))?;
    Ok(d)
}

fn view_to_dict<'py>(py: Python<'py>, v: &View) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("name", v.name.clone())?;
    d.set_item("type", v.view_type.clone())?;
    d.set_item("limit", v.limit)?;
    d.set_item("order", v.order.clone())?;
    match &v.group_by {
        Some(gb) => d.set_item("group_by", group_by_to_dict(py, gb)?)?,
        None => d.set_item("group_by", py.None())?,
    }
    match &v.sort {
        Some(keys) => {
            let list = PyList::empty(py);
            for k in keys {
                list.append(sort_key_to_dict(py, k)?)?;
            }
            d.set_item("sort", list)?;
        }
        None => d.set_item("sort", py.None())?,
    }
    Ok(d)
}

#[pyfunction]
#[pyo3(signature = (vault=None))]
fn list_bases(py: Python<'_>, vault: Option<PathBuf>) -> PyResult<Vec<String>> {
    let vault = resolve_vault(vault)?;
    py.allow_threads(|| scan_bases(&vault)).map_err(to_py_err)
}

#[pyfunction]
#[pyo3(signature = (base_file, vault=None))]
fn list_views<'py>(
    py: Python<'py>,
    base_file: &str,
    vault: Option<PathBuf>,
) -> PyResult<Bound<'py, PyList>> {
    let vault = resolve_vault(vault)?;
    let bf = read_base(&vault, base_file)?;
    let out = PyList::empty(py);
    for v in &bf.views {
        out.append(view_to_dict(py, v)?)?;
    }
    Ok(out)
}

#[pyfunction]
#[pyo3(signature = (base_file, view=None, vault=None))]
fn query(
    py: Python<'_>,
    base_file: &str,
    view: Option<&str>,
    vault: Option<PathBuf>,
) -> PyResult<PyDataFrame> {
    let vault = resolve_vault(vault)?;
    let bf = read_base(&vault, base_file)?;
    let view_name = view.map(str::to_string);
    // Release the GIL during the vault scan + query execution so Python
    // threads (including the signal handler for Ctrl-C) keep running.
    let df = py
        .allow_threads(move || {
            let v = bf.get_view(view_name.as_deref())?;
            execute_query(&vault, &bf, v)
        })
        .map_err(to_py_err)?;
    Ok(PyDataFrame(df))
}

#[pyfunction]
#[pyo3(signature = (vault=None))]
fn scan_vault(py: Python<'_>, vault: Option<PathBuf>) -> PyResult<PyDataFrame> {
    let vault = resolve_vault(vault)?;
    let df = py
        .allow_threads(|| {
            let (lf, _schema) = scan_vault_to_lazyframe(&vault)?;
            lf.collect().map_err(crabase::error::CrabaseError::from)
        })
        .map_err(to_py_err)?;
    Ok(PyDataFrame(df))
}

#[pymodule]
fn _crabase(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(list_bases, m)?)?;
    m.add_function(wrap_pyfunction!(list_views, m)?)?;
    m.add_function(wrap_pyfunction!(query, m)?)?;
    m.add_function(wrap_pyfunction!(scan_vault, m)?)?;
    Ok(())
}
