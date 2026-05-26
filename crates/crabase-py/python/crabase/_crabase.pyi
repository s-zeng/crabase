from typing import Literal, TypedDict

import polars as pl

class SortKey(TypedDict):
    property: str
    direction: Literal["ASC", "DESC"]

class ViewInfo(TypedDict):
    name: str | None
    type: str
    limit: int | None
    order: list[str] | None
    group_by: SortKey | None
    sort: list[SortKey] | None

def list_bases(vault: str | None = None) -> list[str]: ...
def list_views(base_file: str, vault: str | None = None) -> list[ViewInfo]: ...
def query(
    base_file: str,
    view: str | None = None,
    vault: str | None = None,
) -> pl.DataFrame: ...
def scan_vault(vault: str | None = None) -> pl.DataFrame: ...
