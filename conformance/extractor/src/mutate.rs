//! Mechanically inject labeled bug classes into conformance kernels.
//!
//! Every mutant is one labeled, single-site edit of an extracted corpus
//! crate: wrap a barrier in an index-derived `if`; delete a
//! barrier; wrap a warp collective the same way; shrink a full mask; swap a
//! `DisjointSlice<T>` parameter to `&mut [T]`. Mutants that do not compile
//! are discarded — and counted — by the runner; nothing is capped silently
//! (`mutation-report.tsv` accounts for every site seen and every site
//! skipped, with the reason).
//!
//! The same site collection runs on any source file, so hardware session #2
//! can apply identical mutations to the *full* upstream examples (host side
//! included) and compare real-GPU behavior against these labels.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::ops::Range;
use std::path::Path;

use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::util::{LineOffsets, has_attr};

/// The mutation operators. `expected_code` is the diagnostic the injected
/// bug *is* — whether the tool catches it is what the corpus measures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    WrapBarrier,
    DeleteBarrier,
    WrapCollective,
    ShrinkMask,
    SwapMutSlice,
}

impl Class {
    pub fn slug(self) -> &'static str {
        match self {
            Class::WrapBarrier => "wrapbar",
            Class::DeleteBarrier => "delbar",
            Class::WrapCollective => "wrapcol",
            Class::ShrinkMask => "shrinkmask",
            Class::SwapMutSlice => "mutslice",
        }
    }

    /// `-` = no static diagnostic exists for this class by design (a
    /// deleted barrier is a data race, observable only dynamically).
    pub fn expected_code(self) -> &'static str {
        match self {
            Class::WrapBarrier => "RC001",
            Class::DeleteBarrier => "-",
            Class::WrapCollective | Class::ShrinkMask => "RC002",
            Class::SwapMutSlice => "RC003",
        }
    }
}

pub const ALL_CLASSES: &[Class] = &[
    Class::WrapBarrier,
    Class::DeleteBarrier,
    Class::WrapCollective,
    Class::ShrinkMask,
    Class::SwapMutSlice,
];

/// One generated mutant: a full mutated copy of the source file plus its
/// label row.
pub struct Mutant {
    pub class: Class,
    pub kernel: String,
    pub line: usize,
    pub detail: String,
    pub source: String,
}

/// Collectives the shipped dialect classifies (RC002's surface) — asked of
/// the dialect itself, so this predicate can never drift from what the
/// analyzer actually recognizes. (It drifted once: a hand-kept copy matched
/// CUDA C spellings like `shfl_sync` that cuda-device never exports, and
/// the corpus silently skipped every real shuffle site.)
fn is_classified_collective(name: &str) -> bool {
    reconverge_dialect_oxide::simt::classify_call(&format!("cuda_device::warp::{name}"))
        == reconverge_core::dialect::CallKind::WarpCollective
}

/// Warp-collective-backed helpers the dialect does not classify: the
/// unmasked convenience wrappers, which hide the collective (and an
/// implicit full mask) inside `cuda_device` where the analysis cannot see
/// the mask. Sites using them are skipped *and counted*, so the published
/// table names the gap. (`active_mask` is deliberately absent from both
/// lists: it takes no mask and is legal under divergence, so a site using
/// it is not a maskable hazard at all.)
fn is_unclassified_collective(name: &str) -> bool {
    matches!(name, "shuffle" | "ballot" | "all" | "any" | "live_lanes_1d")
        || (name.starts_with("shuffle_") && !name.ends_with("_sync"))
        || name.starts_with("reduce_")
}

/// The index-derived guard every wrap operator injects: the canonical
/// RC001/RC002 shape (`threadIdx_x` is a thread-index witness in the
/// dialect, and lane-computable in the witness interpreter).
const GUARD: &str = "cuda_device::thread::threadIdx_x() % 2 == 0";

#[derive(Default)]
pub struct SkipCounts {
    pub sites_outside_kernels: usize,
    pub unclassified_collectives: usize,
    pub tail_expression_collectives: usize,
    pub extra_disjoint_params: usize,
}

pub struct FileMutants {
    pub mutants: Vec<Mutant>,
    pub skips: SkipCounts,
}

struct BarrierSite {
    kernel: String,
    line: usize,
    stmt: Range<usize>,
}

enum WrapShape {
    /// `let pat = EXPR;` — wrap the initializer in `if GUARD { EXPR } else
    /// { Default::default() }`, preserving the binding for later uses.
    InitExpr(Range<usize>),
    /// A `…;` statement — wrap the whole statement in `if GUARD { … }`.
    WholeStmt(Range<usize>),
}

struct CollectiveSite {
    kernel: String,
    line: usize,
    callee: String,
    shape: WrapShape,
}

struct MaskSite {
    kernel: String,
    line: usize,
    callee: String,
    arg: Range<usize>,
    mask_text: String,
}

struct SwapSite {
    kernel: String,
    line: usize,
    param: String,
    elem_ty: String,
    ty_range: Range<usize>,
    /// `param.get(ARG)` / `param.get_mut(ARG)` argument ranges, rewritten
    /// to `(ARG).get()` so the proof-carrying index becomes a plain usize.
    arg_ranges: Vec<Range<usize>>,
}

#[derive(Default)]
struct Sites {
    barriers: Vec<BarrierSite>,
    collectives: Vec<CollectiveSite>,
    masks: Vec<MaskSite>,
    swaps: Vec<SwapSite>,
    skips: SkipCounts,
}

struct StmtCtx {
    range: Range<usize>,
    has_semi: bool,
    local_init: Option<Range<usize>>,
    /// Innermost-statement marker: set by the first classified collective
    /// call visited while this statement is on top of the stack.
    collective: Option<(String, usize)>,
}

struct Collector<'src> {
    source: &'src str,
    offsets: LineOffsets,
    kernel: Option<String>,
    swap: Option<SwapSite>,
    stmts: Vec<StmtCtx>,
    sites: Sites,
}

fn call_name(expr: &syn::Expr) -> Option<String> {
    if let syn::Expr::Call(call) = expr
        && let syn::Expr::Path(path) = call.func.as_ref()
    {
        return path.path.segments.last().map(|s| s.ident.to_string());
    }
    None
}

impl<'ast, 'src> Visit<'ast> for Collector<'src> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if !has_attr(&node.attrs, "kernel") || self.kernel.is_some() {
            syn::visit::visit_item_fn(self, node);
            return;
        }
        self.kernel = Some(node.sig.ident.to_string());

        // Swap candidate: the first `DisjointSlice<T>` parameter with a
        // single type argument (the plain 1D form); the rest are counted.
        for input in &node.sig.inputs {
            let syn::FnArg::Typed(pat_type) = input else {
                continue;
            };
            let syn::Type::Path(ty_path) = pat_type.ty.as_ref() else {
                continue;
            };
            let Some(seg) = ty_path.path.segments.last() else {
                continue;
            };
            if seg.ident != "DisjointSlice" {
                continue;
            }
            let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
                continue;
            };
            let type_args: Vec<&syn::Type> = args
                .args
                .iter()
                .filter_map(|a| match a {
                    syn::GenericArgument::Type(t) => Some(t),
                    _ => None,
                })
                .collect();
            if type_args.len() != 1 || args.args.len() != type_args.len() {
                continue; // 2D/strided forms have no mechanical slice analogue
            }
            let syn::Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
                continue;
            };
            if self.swap.is_some() {
                self.sites.skips.extra_disjoint_params += 1;
                continue;
            }
            let ty_span = pat_type.ty.span();
            self.swap = Some(SwapSite {
                kernel: node.sig.ident.to_string(),
                line: ty_span.start().line,
                param: pat_ident.ident.to_string(),
                elem_ty: self.offsets.slice(
                    self.source,
                    type_args[0].span().start(),
                    type_args[0].span().end(),
                ),
                ty_range: self.offsets.byte_of(self.source, ty_span.start())
                    ..self.offsets.byte_of(self.source, ty_span.end()),
                arg_ranges: Vec::new(),
            });
        }

        syn::visit::visit_item_fn(self, node);

        if let Some(swap) = self.swap.take() {
            self.sites.swaps.push(swap);
        }
        self.kernel = None;
    }

    fn visit_stmt(&mut self, node: &'ast syn::Stmt) {
        let span = node.span();
        let range = self.offsets.byte_of(self.source, span.start())
            ..self.offsets.byte_of(self.source, span.end());

        // A statement that IS the barrier call.
        if let syn::Stmt::Expr(expr, Some(_)) = node
            && call_name(expr).as_deref() == Some("sync_threads")
        {
            match &self.kernel {
                Some(kernel) => self.sites.barriers.push(BarrierSite {
                    kernel: kernel.clone(),
                    line: span.start().line,
                    stmt: range,
                }),
                None => self.sites.skips.sites_outside_kernels += 1,
            }
            return; // nothing mutable inside a bare call
        }

        let (has_semi, local_init) = match node {
            syn::Stmt::Local(local) => (
                true,
                local.init.as_ref().map(|init| {
                    let s = init.expr.span();
                    self.offsets.byte_of(self.source, s.start())
                        ..self.offsets.byte_of(self.source, s.end())
                }),
            ),
            syn::Stmt::Expr(_, semi) => (semi.is_some(), None),
            _ => (true, None),
        };
        self.stmts.push(StmtCtx {
            range,
            has_semi,
            local_init,
            collective: None,
        });
        syn::visit::visit_stmt(self, node);
        let ctx = self.stmts.pop().expect("stmt stack underflow");
        if let Some((callee, line)) = ctx.collective {
            let shape = match (&ctx.local_init, ctx.has_semi) {
                (Some(init), _) => Some(WrapShape::InitExpr(init.clone())),
                (None, true) => Some(WrapShape::WholeStmt(ctx.range.clone())),
                (None, false) => {
                    // Tail expression: wrapping would change the block's
                    // value. Skipped and counted.
                    self.sites.skips.tail_expression_collectives += 1;
                    None
                }
            };
            if let Some(shape) = shape
                && let Some(kernel) = &self.kernel
            {
                self.sites.collectives.push(CollectiveSite {
                    kernel: kernel.clone(),
                    line,
                    callee,
                    shape,
                });
            }
        }
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref()
            && let Some(seg) = path.path.segments.last()
        {
            let name = seg.ident.to_string();
            if is_classified_collective(&name) {
                if self.kernel.is_none() {
                    self.sites.skips.sites_outside_kernels += 1;
                } else {
                    let line = node.span().start().line;
                    if let Some(top) = self.stmts.last_mut()
                        && top.collective.is_none()
                    {
                        top.collective = Some((name.clone(), line));
                    }
                    if let Some(mask) = node.args.first()
                        && let Some(mask_text) = full_mask_text(mask)
                    {
                        let s = mask.span();
                        self.sites.masks.push(MaskSite {
                            kernel: self.kernel.clone().unwrap(),
                            line,
                            callee: name,
                            arg: self.offsets.byte_of(self.source, s.start())
                                ..self.offsets.byte_of(self.source, s.end()),
                            mask_text,
                        });
                    }
                }
            } else if is_unclassified_collective(&name) && self.kernel.is_some() {
                self.sites.skips.unclassified_collectives += 1;
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if let Some(swap) = self.swap.as_mut()
            && let syn::Expr::Path(recv) = node.receiver.as_ref()
            && recv.path.is_ident(&swap.param)
            && (node.method == "get" || node.method == "get_mut")
            && node.args.len() == 1
        {
            let s = node.args[0].span();
            let range = self.offsets.byte_of(self.source, s.start())
                ..self.offsets.byte_of(self.source, s.end());
            swap.arg_ranges.push(range);
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// A literal or named full mask as the first collective argument.
fn full_mask_text(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(p) => {
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if segs.last().map(String::as_str) == Some("FULL_MASK") {
                return Some("FULL_MASK".to_string());
            }
            if segs.ends_with(&["u32".to_string(), "MAX".to_string()]) {
                return Some("u32::MAX".to_string());
            }
            None
        }
        syn::Expr::Lit(lit) => {
            if let syn::Lit::Int(int) = &lit.lit
                && int.base10_parse::<u64>().ok() == Some(0xffff_ffff)
            {
                return Some(int.token().to_string());
            }
            None
        }
        _ => None,
    }
}

/// Apply byte-range edits (non-overlapping) to `source`.
fn apply_edits(source: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(source.len() + 128);
    let mut cursor = 0usize;
    for (range, replacement) in edits {
        out.push_str(&source[cursor..range.start]);
        out.push_str(&replacement);
        cursor = range.end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// Generate every mutant of one source file.
pub fn mutate_source(source: &str) -> Result<FileMutants, String> {
    let file = syn::parse_file(source).map_err(|e| format!("parse error: {e}"))?;
    let mut collector = Collector {
        source,
        offsets: LineOffsets::new(source),
        kernel: None,
        swap: None,
        stmts: Vec::new(),
        sites: Sites::default(),
    };
    collector.visit_file(&file);
    let sites = collector.sites;
    let mut mutants = Vec::new();

    for site in &sites.barriers {
        let stmt = &source[site.stmt.clone()];
        mutants.push(Mutant {
            class: Class::WrapBarrier,
            kernel: site.kernel.clone(),
            line: site.line,
            detail: "barrier wrapped in an index-derived if".to_string(),
            source: apply_edits(
                source,
                vec![(site.stmt.clone(), format!("if {GUARD} {{ {stmt} }}"))],
            ),
        });
        mutants.push(Mutant {
            class: Class::DeleteBarrier,
            kernel: site.kernel.clone(),
            line: site.line,
            detail: "barrier deleted (data race; no static diagnostic by design)".to_string(),
            source: apply_edits(source, vec![(site.stmt.clone(), String::new())]),
        });
    }

    for site in &sites.collectives {
        let (range, replacement) = match &site.shape {
            WrapShape::InitExpr(init) => {
                let expr = &source[init.clone()];
                (
                    init.clone(),
                    format!("if {GUARD} {{ {expr} }} else {{ Default::default() }}"),
                )
            }
            WrapShape::WholeStmt(stmt) => {
                let text = &source[stmt.clone()];
                (stmt.clone(), format!("if {GUARD} {{ {text} }}"))
            }
        };
        mutants.push(Mutant {
            class: Class::WrapCollective,
            kernel: site.kernel.clone(),
            line: site.line,
            detail: format!("{} wrapped in an index-derived if", site.callee),
            source: apply_edits(source, vec![(range, replacement)]),
        });
    }

    for site in &sites.masks {
        mutants.push(Mutant {
            class: Class::ShrinkMask,
            kernel: site.kernel.clone(),
            line: site.line,
            detail: format!("{}: {} -> 0x0000_ffff", site.callee, site.mask_text),
            source: apply_edits(source, vec![(site.arg.clone(), "0x0000_ffff".to_string())]),
        });
    }

    for site in &sites.swaps {
        let mut edits = vec![(site.ty_range.clone(), format!("&mut [{}]", site.elem_ty))];
        for arg in &site.arg_ranges {
            let text = &source[arg.clone()];
            edits.push((arg.clone(), format!("({text}).get()")));
        }
        mutants.push(Mutant {
            class: Class::SwapMutSlice,
            kernel: site.kernel.clone(),
            line: site.line,
            detail: format!(
                "param `{}`: DisjointSlice<{}> -> &mut [{}]",
                site.param, site.elem_ty, site.elem_ty
            ),
            source: apply_edits(source, edits),
        });
    }

    Ok(FileMutants {
        mutants,
        skips: sites.skips,
    })
}

/// Corpus mode: read the (already pruned) conformance workspace, emit one
/// mutants workspace plus `labels.tsv` and `mutation-report.tsv`.
pub fn run_corpus(corpus: &Path, out_dir: &Path) -> Result<usize, String> {
    let manifest = fs::read_to_string(corpus.join("Cargo.toml"))
        .map_err(|e| format!("cannot read corpus manifest: {e}"))?;
    let members: Vec<String> = manifest
        .lines()
        .filter_map(|l| l.trim().strip_prefix("\"crates/"))
        .filter_map(|l| l.strip_suffix("\","))
        .map(str::to_string)
        .collect();
    if members.is_empty() {
        return Err("corpus manifest lists no members".to_string());
    }

    let crates_dir = out_dir.join("crates");
    let _ = fs::remove_dir_all(out_dir);
    fs::create_dir_all(&crates_dir).map_err(|e| e.to_string())?;

    let mut labels =
        String::from("# mutant\tclass\texpected\tsource_crate\tkernel\tline\tdetail\n");
    let mut workspace_members = Vec::new();
    let mut per_class: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut skips = SkipCounts::default();
    for member in &members {
        let member_dir = corpus.join("crates").join(member);
        let source = fs::read_to_string(member_dir.join("lib.rs"))
            .map_err(|e| format!("cannot read {member}: {e}"))?;
        let member_manifest = fs::read_to_string(member_dir.join("Cargo.toml"))
            .map_err(|e| format!("cannot read {member} manifest: {e}"))?;
        let generated = mutate_source(&source).map_err(|e| format!("{member}: {e}"))?;
        skips.sites_outside_kernels += generated.skips.sites_outside_kernels;
        skips.unclassified_collectives += generated.skips.unclassified_collectives;
        skips.tail_expression_collectives += generated.skips.tail_expression_collectives;
        skips.extra_disjoint_params += generated.skips.extra_disjoint_params;

        let mut counters: BTreeMap<&'static str, usize> = BTreeMap::new();
        for mutant in &generated.mutants {
            let slug = mutant.class.slug();
            let n = counters.entry(slug).or_default();
            let name = format!("m_{slug}_{member}_{n:02}");
            *n += 1;
            *per_class.entry(slug).or_default() += 1;

            let mutant_dir = crates_dir.join(&name);
            fs::create_dir_all(&mutant_dir).map_err(|e| e.to_string())?;
            fs::write(mutant_dir.join("lib.rs"), &mutant.source).map_err(|e| e.to_string())?;
            fs::write(
                mutant_dir.join("Cargo.toml"),
                member_manifest.replace(
                    &format!("name = \"conformance_{member}\""),
                    &format!("name = \"{name}\""),
                ),
            )
            .map_err(|e| e.to_string())?;
            workspace_members.push(name.clone());
            let _ = writeln!(
                labels,
                "{name}\t{slug}\t{expected}\tconformance_{member}\t{kernel}\t{line}\t{detail}",
                expected = mutant.class.expected_code(),
                kernel = mutant.kernel,
                line = mutant.line,
                detail = mutant.detail,
            );
        }
    }

    let mut workspace = String::from("[workspace]\nresolver = \"3\"\nmembers = [\n");
    for member in &workspace_members {
        let _ = writeln!(workspace, "    \"crates/{member}\",");
    }
    workspace.push_str("]\n");
    fs::write(out_dir.join("Cargo.toml"), workspace).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("labels.tsv"), labels).map_err(|e| e.to_string())?;

    let mut report = String::from("# what\tcount\n");
    for class in ALL_CLASSES {
        let _ = writeln!(
            report,
            "emitted_{}\t{}",
            class.slug(),
            per_class.get(class.slug()).copied().unwrap_or(0)
        );
    }
    let _ = writeln!(
        report,
        "skipped_sites_outside_kernels\t{}",
        skips.sites_outside_kernels
    );
    let _ = writeln!(
        report,
        "skipped_unclassified_collectives\t{}",
        skips.unclassified_collectives
    );
    let _ = writeln!(
        report,
        "skipped_tail_expression_collectives\t{}",
        skips.tail_expression_collectives
    );
    let _ = writeln!(
        report,
        "skipped_extra_disjoint_params\t{}",
        skips.extra_disjoint_params
    );
    fs::write(out_dir.join("mutation-report.tsv"), report).map_err(|e| e.to_string())?;

    Ok(workspace_members.len())
}

/// Single-file mode (hardware session #2): write `<class>_<nn>.rs` variants
/// of one source file plus `labels.tsv`, for splicing over the full
/// upstream examples on a GPU host.
pub fn run_file(input: &Path, out_dir: &Path) -> Result<usize, String> {
    let source = fs::read_to_string(input).map_err(|e| format!("cannot read input: {e}"))?;
    let generated = mutate_source(&source)?;
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let mut labels = String::from("# file\tclass\texpected\tkernel\tline\tdetail\n");
    let mut counters: BTreeMap<&'static str, usize> = BTreeMap::new();
    for mutant in &generated.mutants {
        let slug = mutant.class.slug();
        let n = counters.entry(slug).or_default();
        let file_name = format!("{slug}_{n:02}.rs");
        *n += 1;
        fs::write(out_dir.join(&file_name), &mutant.source).map_err(|e| e.to_string())?;
        let _ = writeln!(
            labels,
            "{file_name}\t{slug}\t{expected}\t{kernel}\t{line}\t{detail}",
            expected = mutant.class.expected_code(),
            kernel = mutant.kernel,
            line = mutant.line,
            detail = mutant.detail,
        );
    }
    fs::write(out_dir.join("labels.tsv"), labels).map_err(|e| e.to_string())?;
    Ok(generated.mutants.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
use cuda_device::{DisjointSlice, kernel, thread, warp};
use cuda_device::warp::FULL_MASK;

mod kernels {
    use super::*;

    #[kernel]
    pub fn reduce(mut out: DisjointSlice<u32>, n: u32) {
        let i = thread::index_1d();
        thread::sync_threads();
        let vote = warp::ballot_sync(FULL_MASK, n > 1);
        let echo = warp::shuffle_sync(0xffff_ffff, vote, 0);
        if let Some(e) = out.get_mut(i) {
            *e = vote + echo;
        }
    }

    pub fn helper() {
        thread::sync_threads();
    }
}
"#;

    fn mutants_of(class: Class) -> Vec<Mutant> {
        mutate_source(SAMPLE)
            .unwrap()
            .mutants
            .into_iter()
            .filter(|m| m.class == class)
            .collect()
    }

    #[test]
    fn wraps_and_deletes_the_kernel_barrier_only() {
        let wraps = mutants_of(Class::WrapBarrier);
        assert_eq!(wraps.len(), 1, "helper() barriers are outside kernels");
        assert!(wraps[0].source.contains(
            "if cuda_device::thread::threadIdx_x() % 2 == 0 { thread::sync_threads(); }"
        ));
        assert_eq!(wraps[0].kernel, "reduce");
        syn::parse_file(&wraps[0].source).expect("wrap mutant parses");

        let deletes = mutants_of(Class::DeleteBarrier);
        assert_eq!(deletes.len(), 1);
        // Only the kernel's barrier is gone; the helper's stays.
        assert_eq!(
            deletes[0].source.matches("thread::sync_threads();").count(),
            1
        );
        syn::parse_file(&deletes[0].source).expect("delete mutant parses");
    }

    #[test]
    fn wraps_collective_initializers_preserving_the_binding() {
        let wraps = mutants_of(Class::WrapCollective);
        assert_eq!(wraps.len(), 2, "ballot and shuffle sites");
        for m in &wraps {
            assert!(m.source.contains("} else { Default::default() }"));
            syn::parse_file(&m.source).expect("collective mutant parses");
        }
    }

    #[test]
    fn shrinks_named_and_literal_full_masks() {
        let masks = mutants_of(Class::ShrinkMask);
        assert_eq!(masks.len(), 2);
        for m in &masks {
            assert!(m.source.contains("0x0000_ffff"));
            syn::parse_file(&m.source).expect("mask mutant parses");
        }
        assert!(masks.iter().any(|m| m.detail.contains("FULL_MASK ->")));
        assert!(masks.iter().any(|m| m.detail.contains("0xffff_ffff ->")));
    }

    #[test]
    fn swaps_disjoint_slice_params_and_rewrites_index_args() {
        let swaps = mutants_of(Class::SwapMutSlice);
        assert_eq!(swaps.len(), 1);
        let m = &swaps[0];
        assert!(m.source.contains("mut out: &mut [u32]"));
        assert!(m.source.contains("out.get_mut((i).get())"));
        syn::parse_file(&m.source).expect("swap mutant parses");
    }

    #[test]
    fn counts_skipped_sites_instead_of_dropping_them_silently() {
        let generated = mutate_source(SAMPLE).unwrap();
        assert_eq!(generated.skips.sites_outside_kernels, 1); // helper()'s barrier
    }
}
