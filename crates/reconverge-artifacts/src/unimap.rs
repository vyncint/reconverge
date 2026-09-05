//! `unimap.v1` — the uniformity-map artifact (`schemas/unimap.v1.json`).
//!
//! One document per analyzed crate: per function, values with uniformity
//! labels and divergence sources, provenance edges, and CFG blocks with
//! divergent-control bits. The Inspector is a pure reader of this.
//! Additive-only within v1.

use serde::{Deserialize, Serialize};

use crate::findings::{SourceSpan, ToolInfo};
use crate::read::Artifact;
use crate::schema;

/// Top-level uniformity-map artifact for one analyzed crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnimapArtifact {
    /// Always [`schema::UNIMAP`].
    pub schema: String,
    pub tool: ToolInfo,
    #[serde(rename = "crate")]
    pub krate: String,
    pub functions: Vec<Function>,
}

impl UnimapArtifact {
    pub fn new(krate: impl Into<String>, functions: Vec<Function>) -> Self {
        UnimapArtifact {
            schema: schema::UNIMAP.to_string(),
            tool: ToolInfo::current(),
            krate: krate.into(),
            functions,
        }
    }
}

impl Artifact for UnimapArtifact {
    const SCHEMA: &'static str = schema::UNIMAP;

    fn declared_schema(&self) -> &str {
        &self.schema
    }
}

/// Uniformity facts for one analyzed function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    /// User-facing name (kernel base name for kernels).
    pub name: String,
    /// Fully qualified item path.
    pub item: String,
    pub span: SourceSpan,
    /// Coverage honesty (docs/ARCHITECTURE.md): how much of the body was analyzed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    pub values: Vec<Value>,
    /// Def→use edges: `to` is derived from `from`.
    pub provenance: Vec<ProvenanceEdge>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub analyzed_statements: usize,
    pub opaque_statements: usize,
}

/// A labeled value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    /// Stable value id within the function, e.g. `"v3"`.
    pub id: String,
    /// Source-level name, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub uniformity: Uniformity,
    /// Why the value carries its label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ValueSource>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Uniformity {
    Uniform,
    Divergent,
}

/// Divergence sources and uniform origins (docs/ARCHITECTURE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueSource {
    ThreadIndex,
    KernelParam,
    BlockIndex,
    Constant,
    DivergentLoad,
    AtomicReturn,
    DivergentPhi,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceEdge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
}

/// A CFG block with its divergent-control bit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Basic-block id, e.g. `"bb0"`.
    pub id: String,
    /// True when the block executes under thread-divergent control.
    pub divergent_control: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::round_trip_fixtures;

    #[test]
    fn unimap_fixtures_round_trip() {
        round_trip_fixtures("unimap", |text| {
            let parsed: UnimapArtifact = serde_json::from_str(text)?;
            assert_eq!(parsed.schema, crate::schema::UNIMAP);
            serde_json::to_value(&parsed)
        });
    }
}
