//! Inspector inputs: parsed artifacts plus the source files they span.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use reconverge_artifacts::findings::{Finding, FindingsArtifact, SourceSpan};
use reconverge_artifacts::unimap::{self, UnimapArtifact};

use crate::load::{display_name, nfc};

/// Everything the Inspector shows, loaded once up front.
#[derive(Debug, Default)]
pub struct InspectorData {
    pub functions: Vec<FunctionData>,
    /// All findings across the loaded findings artifacts, in file order.
    pub findings: Vec<Finding>,
    /// Load problems, rendered in-frame.
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub struct FunctionData {
    pub function: unimap::Function,
    /// The source file the function's span points at, when readable.
    pub source: Option<SourceFile>,
    /// Indices into `function.values` worth listing: named values and
    /// divergence sources, in source order.
    pub listed: Vec<usize>,
    /// Incoming provenance edges per value id (`to` == key), in stable
    /// order — following the first edge walks toward the source.
    pub incoming: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug)]
pub struct SourceFile {
    /// Redacted display name (basename only).
    pub name: String,
    pub lines: Vec<String>,
}

/// Load a mix of `unimap.v1` and `findings.v1` files.
pub fn load(paths: &[std::path::PathBuf]) -> InspectorData {
    let mut data = InspectorData::default();
    for path in paths {
        let name = display_name(path);
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                data.errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        let schema = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["schema"].as_str().map(str::to_string))
            .unwrap_or_default();
        match schema.as_str() {
            "unimap.v1" => match serde_json::from_str::<UnimapArtifact>(&text) {
                Ok(artifact) => {
                    for function in artifact.functions {
                        data.functions.push(function_data(function));
                    }
                }
                Err(e) => data.errors.push(format!("{name}: {e}")),
            },
            "findings.v1" => match serde_json::from_str::<FindingsArtifact>(&text) {
                Ok(artifact) => data.findings.extend(artifact.findings),
                Err(e) => data.errors.push(format!("{name}: {e}")),
            },
            other => data
                .errors
                .push(format!("{name}: unsupported schema `{other}`")),
        }
    }
    data.functions
        .sort_by(|a, b| a.function.name.cmp(&b.function.name));
    data
}

fn function_data(function: unimap::Function) -> FunctionData {
    let source = fs::read_to_string(&function.span.file)
        .ok()
        .map(|text| SourceFile {
            name: display_name(Path::new(&function.span.file)),
            lines: text.lines().map(nfc).collect(),
        });

    let mut listed: Vec<usize> = function
        .values
        .iter()
        .enumerate()
        .filter(|(_, value)| {
            value.name.is_some()
                || matches!(
                    value.source,
                    Some(unimap::ValueSource::ThreadIndex)
                        | Some(unimap::ValueSource::AtomicReturn)
                )
        })
        .map(|(i, _)| i)
        .collect();
    listed.sort_by_key(|&i| {
        let span = &function.values[i].span;
        (span.line_start, span.column_start, i)
    });

    let mut incoming: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, edge) in function.provenance.iter().enumerate() {
        incoming.entry(edge.to.clone()).or_default().push(i);
    }

    FunctionData {
        function,
        source,
        listed,
        incoming,
    }
}

impl FunctionData {
    /// Index into `function.values` for a value id.
    #[must_use]
    pub fn value_index(&self, id: &str) -> Option<usize> {
        self.function.values.iter().position(|v| v.id == id)
    }

    /// The provenance chain from `id` toward its source: at each hop the
    /// first incoming edge is followed. Returns (edge description, from-id)
    /// pairs; empty when the value has no incoming edges.
    #[must_use]
    pub fn chain_from(&self, id: &str) -> Vec<(String, String)> {
        let mut chain = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut current = id.to_string();
        while seen.insert(current.clone()) {
            let Some(edges) = self.incoming.get(&current) else {
                break;
            };
            let Some(&edge_index) = edges.first() else {
                break;
            };
            let edge = &self.function.provenance[edge_index];
            chain.push((
                edge.what.clone().unwrap_or_else(|| "derived".to_string()),
                edge.from.clone(),
            ));
            current = edge.from.clone();
            if chain.len() >= 32 {
                break;
            }
        }
        chain
    }

    /// The listed value whose span starts where `span` starts, if any.
    #[must_use]
    pub fn value_at(&self, span: &SourceSpan) -> Option<&str> {
        self.function
            .values
            .iter()
            .find(|v| {
                v.span.line_start == span.line_start && v.span.column_start == span.column_start
            })
            .map(|v| v.id.as_str())
    }
}
