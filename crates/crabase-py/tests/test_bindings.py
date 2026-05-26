"""Snapshot tests for the crabase Python bindings.

Each test makes exactly one assertion against a syrupy snapshot, matching
the single-assertion / literal-snapshot convention used by the Rust insta
suite (see project CLAUDE.md).

Documentation written by Claude Code.
"""

from __future__ import annotations

import pytest

import crabase


# ---------- list_bases ----------


def test_list_bases(vault, snapshot):
    bases = crabase.list_bases(vault=vault)
    assert "\n".join(bases) == snapshot


# ---------- list_views ----------


def test_list_views_notes(vault, snapshot):
    views = crabase.list_views("queries/notes.base", vault=vault)
    assert views == snapshot


def test_list_views_projects(vault, snapshot):
    views = crabase.list_views("queries/projects.base", vault=vault)
    assert views == snapshot


# ---------- query ----------


def test_query_default_view_is_first(vault, df_to_snapshot, snapshot):
    df = crabase.query("queries/notes.base", vault=vault)
    assert df_to_snapshot(df) == snapshot


def test_query_by_score(vault, df_to_snapshot, snapshot):
    df = crabase.query("queries/notes.base", view="By Score", vault=vault)
    assert df_to_snapshot(df) == snapshot


def test_query_all_view(vault, df_to_snapshot, snapshot):
    df = crabase.query("queries/notes.base", view="All", vault=vault)
    assert df_to_snapshot(df) == snapshot


def test_query_projects(vault, df_to_snapshot, snapshot):
    df = crabase.query("queries/projects.base", view="Open", vault=vault)
    assert df_to_snapshot(df) == snapshot


# ---------- scan_vault ----------


def test_scan_vault_schema(vault, schema_to_snapshot, snapshot):
    df = crabase.scan_vault(vault=vault)
    assert schema_to_snapshot(df) == snapshot


def test_scan_vault_rows(vault, df_to_snapshot, snapshot):
    df = crabase.scan_vault(vault=vault).sort("file_path")
    assert df_to_snapshot(df) == snapshot


# ---------- error mapping ----------


def test_query_missing_base_raises(vault):
    with pytest.raises(FileNotFoundError):
        crabase.query("queries/does-not-exist.base", vault=vault)


def test_query_unknown_view_raises(vault):
    with pytest.raises(KeyError):
        crabase.query("queries/notes.base", view="Nope", vault=vault)
