use crate::error::{CrabaseError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A filter node in the .base file filter tree
#[derive(Debug, Clone)]
pub enum FilterNode {
    And(Vec<FilterNode>),
    Or(Vec<FilterNode>),
    /// Not: none of the children are true (equivalent to NOT OR)
    Not(Vec<FilterNode>),
    Expr(String),
}

impl FilterNode {
    /// Parse a FilterNode from a serde_yaml::Value
    pub fn from_yaml(value: &serde_yaml::Value) -> Result<FilterNode> {
        match value {
            serde_yaml::Value::String(s) => Ok(FilterNode::Expr(s.clone())),
            serde_yaml::Value::Mapping(map) => {
                if let Some(and_val) = map.get("and") {
                    let children = parse_filter_list(and_val)?;
                    Ok(FilterNode::And(children))
                } else if let Some(or_val) = map.get("or") {
                    let children = parse_filter_list(or_val)?;
                    Ok(FilterNode::Or(children))
                } else if let Some(not_val) = map.get("not") {
                    let children = parse_filter_list(not_val)?;
                    Ok(FilterNode::Not(children))
                } else {
                    Err(CrabaseError::BaseFile(format!(
                        "Filter mapping must have 'and', 'or', or 'not' key, got: {:?}",
                        map.keys().collect::<Vec<_>>()
                    )))
                }
            }
            serde_yaml::Value::Sequence(seq) => {
                // A bare sequence is treated as And
                let children = seq
                    .iter()
                    .map(FilterNode::from_yaml)
                    .collect::<Result<Vec<_>>>()?;
                Ok(FilterNode::And(children))
            }
            _ => Err(CrabaseError::BaseFile(format!(
                "Unexpected filter value type: {value:?}"
            ))),
        }
    }
}

fn parse_filter_list(value: &serde_yaml::Value) -> Result<Vec<FilterNode>> {
    match value {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .map(FilterNode::from_yaml)
            .collect::<Result<Vec<_>>>(),
        other => {
            // Single value - treat as a list of one
            Ok(vec![FilterNode::from_yaml(other)?])
        }
    }
}

/// Sort direction
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum SortDirection {
    #[serde(rename = "ASC")]
    #[default]
    Asc,
    #[serde(rename = "DESC")]
    Desc,
}

/// Sort key specification
#[derive(Debug, Clone, Deserialize)]
pub struct SortKey {
    pub property: String,
    #[serde(default)]
    pub direction: SortDirection,
}

/// GroupBy specification
#[derive(Debug, Clone, Deserialize)]
pub struct GroupBy {
    pub property: String,
    #[serde(default)]
    pub direction: SortDirection,
}

/// Property configuration
#[derive(Debug, Clone, Deserialize)]
pub struct PropertyConfig {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// A view in the .base file
#[derive(Debug, Clone)]
pub struct View {
    pub view_type: String,
    pub name: Option<String>,
    pub limit: Option<usize>,
    pub order: Option<Vec<String>>,
    pub filters: Option<FilterNode>,
    pub group_by: Option<GroupBy>,
    pub sort: Option<Vec<SortKey>>,
}

impl View {
    pub fn from_yaml(value: &serde_yaml::Value) -> Result<View> {
        let map = value
            .as_mapping()
            .ok_or_else(|| CrabaseError::BaseFile("View must be a YAML mapping".to_string()))?;

        let view_type = map
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("table")
            .to_string();

        let name = map.get("name").and_then(|v| v.as_str()).map(str::to_string);

        let limit = map
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let order = map.get("order").and_then(|v| v.as_sequence()).map(|seq| {
            seq.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });

        let filters = map.get("filters").map(FilterNode::from_yaml).transpose()?;

        let group_by = map
            .get("groupBy")
            .map(|v| serde_yaml::from_value::<GroupBy>(v.clone()))
            .transpose()
            .map_err(|e| CrabaseError::BaseFile(format!("groupBy parse error: {e}")))?;

        let sort = map
            .get("sort")
            .map(|v| serde_yaml::from_value::<Vec<SortKey>>(v.clone()))
            .transpose()
            .map_err(|e| CrabaseError::BaseFile(format!("sort parse error: {e}")))?;

        Ok(View {
            view_type,
            name,
            limit,
            order,
            filters,
            group_by,
            sort,
        })
    }
}

/// The top-level .base file structure
#[derive(Debug)]
pub struct BaseFile {
    pub filters: Option<FilterNode>,
    pub formulas: HashMap<String, String>,
    pub properties: HashMap<String, PropertyConfig>,
    pub views: Vec<View>,
}

impl BaseFile {
    pub fn parse(content: &str) -> Result<BaseFile> {
        let yaml: serde_yaml::Value = serde_yaml::from_str(content)?;
        let map = yaml.as_mapping().ok_or_else(|| {
            CrabaseError::BaseFile("Base file must be a YAML mapping at root".to_string())
        })?;

        let filters = map.get("filters").map(FilterNode::from_yaml).transpose()?;

        let formulas = map
            .get("formulas")
            .and_then(|v| v.as_mapping())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        let key = k.as_str()?.to_string();
                        let val = v.as_str()?.to_string();
                        Some((key, val))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        let properties = map
            .get("properties")
            .and_then(|v| v.as_mapping())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        let key = k.as_str()?.to_string();
                        let config: PropertyConfig = serde_yaml::from_value(v.clone()).ok()?;
                        Some((key, config))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        let views = map
            .get("views")
            .and_then(|v| v.as_sequence())
            .map(|seq| seq.iter().map(View::from_yaml).collect::<Result<Vec<_>>>())
            .transpose()?
            .unwrap_or_default();

        Ok(BaseFile {
            filters,
            formulas,
            properties,
            views,
        })
    }

    /// Get a view by name, or the first view if name is None
    pub fn get_view(&self, name: Option<&str>) -> Result<&View> {
        match name {
            Some(n) => self
                .views
                .iter()
                .find(|v| v.name.as_deref() == Some(n))
                .ok_or_else(|| CrabaseError::ViewNotFound(n.to_string())),
            None => self.views.first().ok_or(CrabaseError::NoViews),
        }
    }
}
