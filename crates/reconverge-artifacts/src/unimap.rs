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
    /// Which tool wrote this document, and its version.
    pub tool: ToolInfo,
    /// Name of the analyzed crate.
    #[serde(rename = "crate")]
    pub krate: String,
    /// Every analyzed function — kernels and local helpers alike.
    pub functions: Vec<Function>,
}

impl UnimapArtifact {
    /// A uniformity map for `krate` under the current tool identity.
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
    /// Where the function is defined.
    pub span: SourceSpan,
    /// Coverage honesty (docs/ARCHITECTURE.md): how much of the body was analyzed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    /// Every labeled value in the function.
    pub values: Vec<Value>,
    /// Def→use edges: `to` is derived from `from`.
    pub provenance: Vec<ProvenanceEdge>,
    /// Every basic block, with its divergent-control bit.
    pub blocks: Vec<Block>,
}

/// How much of a function's body the analysis could read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    /// Statements the model represents.
    pub analyzed_statements: usize,
    /// Statements it could not — inline asm, unmodeled intrinsics. Counted,
    /// never guessed at.
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
    /// Uniform or divergent across the lanes of a warp.
    pub uniformity: Uniformity,
    /// Why the value carries its label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ValueSource>,
    /// Where the value is defined.
    pub span: SourceSpan,
}

/// The two-point lattice the analysis computes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Uniformity {
    /// Every active lane holds the same value.
    Uniform,
    /// Lanes may hold different values.
    Divergent,
}

/// Divergence sources and uniform origins (docs/ARCHITECTURE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueSource {
    /// A thread-index witness (`index_1d()` and its kin) or a lane id.
    ThreadIndex,
    /// A kernel parameter: the same for every thread.
    KernelParam,
    /// Derived from `blockIdx`, `blockDim` or `gridDim`: uniform within a block.
    BlockIndex,
    /// A literal.
    Constant,
    /// A load through a thread-dependent address.
    DivergentLoad,
    /// The previous value an atomic read-modify-write returns: it differs per
    /// thread by construction.
    AtomicReturn,
    /// A merge of values arriving from paths under thread-divergent control.
    DivergentPhi,
    /// Computed from inputs that already carry a label.
    Derived,
}

/// One def→use edge of the provenance graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceEdge {
    /// The value derived from — a [`Value::id`].
    pub from: String,
    /// The value derived — a [`Value::id`].
    pub to: String,
    /// What the derivation is, in prose, when the analysis recorded one.
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
    /// Where the block's terminator is, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
    /// The [`Value::id`]s written in this block.
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
