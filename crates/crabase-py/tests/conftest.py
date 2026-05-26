"""pytest fixtures for the crabase Python bindings test suite.

Documentation written by Claude Code.
"""

from __future__ import annotations

from pathlib import Path
from typing import Callable

import polars as pl
import pytest

VAULT = Path(__file__).parent / "fixtures" / "vault"

# Columns whose values depend on the filesystem state of the checkout (size
# in bytes, ctime/mtime). They are dropped before snapshotting so snapshots
# stay deterministic across machines and re-clones.
VOLATILE_COLUMNS = ("file_size", "file_ctime", "file_mtime")


@pytest.fixture
def vault() -> str:
    return str(VAULT)


@pytest.fixture
def df_to_snapshot() -> Callable[[pl.DataFrame], str]:
    """Render a DataFrame as a deterministic, snapshot-friendly string.

    Drops filesystem-dependent columns, then formats the frame as CSV with
    a leading `# schema:` line so snapshots also exercise schema stability.
    """

    def render(df: pl.DataFrame) -> str:
        keep = [c for c in df.columns if c not in VOLATILE_COLUMNS]
        df = df.select(keep)
        types = ", ".join(f"{c}: {df.schema[c]}" for c in df.columns)
        # CSV doesn't support nested data — join list cells with ", " so
        # they render as a single string column. The original dtype is
        # preserved in the schema line above.
        list_cols = [c for c, t in df.schema.items() if isinstance(t, pl.List)]
        if list_cols:
            df = df.with_columns(
                pl.col(c).cast(pl.List(pl.String)).list.join(", ").alias(c)
                for c in list_cols
            )
        return f"# schema: {types}\n{df.write_csv()}"

    return render


@pytest.fixture
def schema_to_snapshot() -> Callable[[pl.DataFrame], str]:
    """Render just the column->dtype map, sorted by column name."""

    def render(df: pl.DataFrame) -> str:
        items = sorted((c, str(t)) for c, t in df.schema.items())
        return "\n".join(f"{c}: {t}" for c, t in items)

    return render
