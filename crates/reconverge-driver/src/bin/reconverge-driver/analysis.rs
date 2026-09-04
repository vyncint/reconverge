//! Kernel detection and the M1 lint passes (RC003, RC004, RC005).
//!
//! Everything here runs inside a `rustc_public` session. Dialect knowledge
//! (paths, naming contract, CC table) comes from `reconverge_dialect_oxide`;
//! this module only walks MIR.

use std::collections::BTreeMap;

use reconverge_artifacts::findings::{Confidence, Finding};
use reconverge_dialect_oxide::cc::{
    self, ComputeCapability, STATIC_SHARED_LIMIT_BYTES, SharedMemoryLimits,
};
use reconverge_dialect_oxide::kernel_base_name;
use reconverge_dialect_oxide::paths::{self, IndexFn, LaunchDomain};
use rustc_public::mir::alloc::GlobalAlloc;
use rustc_public::mir::mono::{Instance, StaticDef};
use rustc_public::mir::visit::MirVisitor;
use rustc_public::mir::{ConstOperand, Mutability, Terminator, TerminatorKind};
use rustc_public::ty::{ConstantKind, GenericArgKind, RigidTy, Ty, TyConst, TyConstKind, TyKind};
use rustc_public::{CrateDef, CrateItem, ItemKind};

use crate::emit;

/// A detected cuda-oxide kernel.
pub struct Kernel {
    /// User-facing kernel name (`scale`), per the naming contract.
    pub name: String,
    /// Fully qualified item path of the generated function.
    pub path: String,
    pub item: CrateItem,
}

/// Detect every cuda-oxide kernel among the local items, sorted by name.
pub fn detect_kernels() -> Vec<Kernel> {
    let mut kernels: Vec<Kernel> = rustc_public::all_local_items()
        .into_iter()
        .filter(|item| item.kind() == ItemKind::Fn)
        .filter_map(|item| {
            let path = item.name();
            let name = kernel_base_name(&path)?.to_string();
            Some(Kernel { name, path, item })
        })
        .collect();
    kernels.sort_by(|a, b| a.name.cmp(&b.name));
    kernels
}

/// Run the syntactic lint passes (RC003/RC004/RC005) over every kernel.
/// RC001 comes from the uniformity engine (`crate::uniformity`); callers
/// re-sort after appending it.
pub fn run_lints(kernels: &[Kernel], target_cc: Option<ComputeCapability>) -> Vec<Finding> {
    let mut findings = Vec::new();
    for kernel in kernels {
        rc003_mut_slice_params(kernel, &mut findings);
        rc004_shared_memory_budget(kernel, target_cc, &mut findings);
        rc005_launch_contract(kernel, &mut findings);
    }
    sort_findings(&mut findings);
    findings
}

/// Deterministic finding order: file, line, column, code.
pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        (
            &a.span.file,
            a.span.line_start,
            a.span.column_start,
            &a.code,
        )
            .cmp(&(
                &b.span.file,
                b.span.line_start,
                b.span.column_start,
                &b.code,
            ))
    });
}

/// RC003 (deny): `&mut [T]` as a kernel parameter.
///
/// Every thread of the launch receives the same slice, so writes through it
/// alias across threads; upstream's safety model says to treat a kernel
/// taking one as if every line of it were unsafe.
fn rc003_mut_slice_params(kernel: &Kernel, findings: &mut Vec<Finding>) {
    let Some(body) = kernel.item.body() else {
        return;
    };
    for local in body.arg_locals() {
        let TyKind::RigidTy(RigidTy::Ref(_, pointee, Mutability::Mut)) = local.ty.kind() else {
            continue;
        };
        let TyKind::RigidTy(RigidTy::Slice(element)) = pointee.kind() else {
            continue;
        };
        findings.push(Finding {
            code: "RC003".to_string(),
            confidence: Confidence::Deny,
            message: format!(
                "kernel `{}` takes `&mut [{element}]` as a parameter",
                kernel.name
            ),
            kernel: Some(kernel.name.clone()),
            span: emit::source_span(local.span),
            notes: vec![
                "every thread of the launch receives the same slice, so writes through it \
                 alias the same memory across threads"
                    .to_string(),
                "upstream's safety model treats a kernel taking `&mut [T]` as if every line \
                 of it were unsafe"
                    .to_string(),
            ],
            help: Some(format!(
                "pass `DisjointSlice<{element}>` and index it with a thread-index witness"
            )),
            explain: "RC003".to_string(),
            provenance: Vec::new(),
        });
    }
}

/// RC004 (deny): static shared memory over the architectural limit.
///
/// Sums the sizes of every `SharedArray`/`Barrier` static the kernel body
/// references. Statically sized shared memory is capped at 48 KiB per kernel
/// on every supported architecture; `--cc` adds the target's total capacity
/// (static + dynamic opt-in) to the message.
fn rc004_shared_memory_budget(
    kernel: &Kernel,
    target_cc: Option<ComputeCapability>,
    findings: &mut Vec<Finding>,
) {
    let Some(body) = kernel.item.body() else {
        return;
    };
    let mut collector = SharedStaticCollector::default();
    collector.visit_body(&body);

    let mut total: u64 = 0;
    let mut notes = Vec::new();
    let mut unreadable = Vec::new();
    for (def, size, kind) in collector.statics.values() {
        match size {
            Some(size) => {
                total = total.saturating_add(*size);
                notes.push(format!("`{}`: {size} bytes ({kind})", def.trimmed_name()));
            }
            None => unreadable.push(format!("`{}` ({kind})", def.trimmed_name())),
        }
    }

    // A size the analysis could not read is reported before the budget is
    // compared, and separately from it: the comparison that follows is over a
    // total this kernel is known to exceed by an unknown amount, so a clean
    // verdict from it would be meaningless rather than merely incomplete.
    if !unreadable.is_empty() {
        unreadable.sort();
        let mut notes = unreadable.clone();
        notes.push(format!(
            "the sizes it could read total {total} bytes, against the \
             {STATIC_SHARED_LIMIT_BYTES}-byte static limit"
        ));
        notes.push(
            "a length given as a literal, or as a `const` with no generic parameters of its \
             own, is readable here; one that depends on a generic parameter is not known \
             until the kernel is instantiated"
                .to_string(),
        );
        findings.push(Finding {
            code: "RC004".to_string(),
            confidence: Confidence::Deny,
            message: format!(
                "kernel `{}` declares shared memory whose size could not be read, so its \
                 shared-memory budget is unchecked",
                kernel.name
            ),
            kernel: Some(kernel.name.clone()),
            span: emit::source_span(kernel.item.span()),
            notes,
            help: Some(
                "give the length as a literal or a non-generic `const`, or report this \
                 kernel — an unreadable size is a gap in the check, not a property of the \
                 kernel"
                    .to_string(),
            ),
            explain: "RC004".to_string(),
            provenance: Vec::new(),
        });
    }

    if total <= STATIC_SHARED_LIMIT_BYTES {
        return;
    }

    notes.sort();
    notes.push(
        "statically sized shared memory above 48 KiB cannot load on any supported \
         architecture; larger regions must be dynamic shared memory with a launch-time opt-in"
            .to_string(),
    );
    if let Some(SharedMemoryLimits { cc, max_per_block }) =
        target_cc.and_then(cc::shared_memory_limits)
    {
        notes.push(format!(
            "target compute capability {}.{} offers at most {max_per_block} bytes per block \
             even with the dynamic opt-in",
            cc.0, cc.1
        ));
    }

    findings.push(Finding {
        code: "RC004".to_string(),
        confidence: Confidence::Deny,
        message: format!(
            "kernel `{}` declares {total} bytes of static shared memory, over the \
             {STATIC_SHARED_LIMIT_BYTES}-byte static limit",
            kernel.name
        ),
        kernel: Some(kernel.name.clone()),
        span: emit::source_span(kernel.item.span()),
        notes,
        help: Some(
            "shrink the shared arrays, or move the excess to dynamic shared memory sized at \
             launch"
                .to_string(),
        ),
        explain: "RC004".to_string(),
        provenance: Vec::new(),
    });
}

/// Collects shared-memory statics (`SharedArray`, `Barrier`) referenced by a
/// body, deduplicated and in stable name order, with their device sizes.
///
/// A size of `None` is one the analysis could not read. It is kept rather
/// than dropped, because dropping it is indistinguishable from the static not
/// being there — which is how a kernel over the cap came back clean.
#[derive(Default)]
struct SharedStaticCollector {
    statics: BTreeMap<String, (StaticDef, Option<u64>, &'static str)>,
}

impl MirVisitor for SharedStaticCollector {
    fn visit_const_operand(
        &mut self,
        constant: &ConstOperand,
        _location: rustc_public::mir::visit::Location,
    ) {
        if let ConstantKind::Allocated(allocation) = constant.const_.kind() {
            for (_, prov) in &allocation.provenance.ptrs {
                let GlobalAlloc::Static(def) = GlobalAlloc::from(prov.0) else {
                    continue;
                };
                match shared_resident_size(def.ty()) {
                    SharedSize::Known(size, kind) => {
                        self.statics.insert(def.name(), (def, Some(size), kind));
                    }
                    SharedSize::Unresolved(kind) => {
                        self.statics.insert(def.name(), (def, None, kind));
                    }
                    SharedSize::NotShared => {}
                }
            }
        }
        self.super_const_operand(constant, _location);
    }
}

/// What a static's type contributes to the shared-memory budget.
enum SharedSize {
    /// Not a shared-resident type: contributes nothing, and that is a fact
    /// rather than a gap.
    NotShared,
    /// A shared-resident type of a size the analysis could read.
    Known(u64, &'static str),
    /// A shared-resident type whose size the analysis could **not** read.
    ///
    /// Kept rather than dropped. RC004 is a capacity gate against a hard
    /// architectural limit, so a size it cannot read is a kernel it has not
    /// checked — and reporting that as clean is the one outcome a gate must
    /// never produce (docs/SAFETY.md).
    Unresolved(&'static str),
}

/// The value of a const generic argument, following an unevaluated const to
/// its own body.
///
/// `SharedArray<f32, 16384>` arrives as `TyConstKind::Value` and
/// `eval_target_usize` reads it directly. `SharedArray<f32, TILE>` arrives as
/// `Unevaluated(ConstDef, _)` — an anonymous const body wrapping the path —
/// and `eval_target_usize` refuses it outright:
///
/// ```text
/// Const `UnevaluatedConst { def: …::STAGE::{constant#0}, args: [] }`
/// cannot be encoded as u64
/// ```
///
/// That body is an ordinary item with no generic arguments of its own, so it
/// evaluates through the instance machinery. Without this, every named size
/// was silently invisible to RC004 — and a tuner that rewrites named consts
/// per candidate, as launchbound does, takes exactly that path for every
/// configuration it tries.
fn const_as_usize(c: &TyConst) -> Option<u64> {
    if let Ok(len) = c.eval_target_usize() {
        return Some(len);
    }
    let TyConstKind::Unevaluated(def, args) = c.kind() else {
        return None;
    };
    // Arguments of its own mean it still depends on something not yet known;
    // there is nothing to evaluate against.
    if !args.0.is_empty() {
        return None;
    }
    let instance = Instance::try_from(CrateItem(def.0)).ok()?;
    let value = instance
        .try_const_eval(Ty::usize_ty())
        .ok()?
        .read_uint()
        .ok()?;
    u64::try_from(value).ok()
}

/// Device-side shared-memory footprint of a static's type, if it is a
/// shared-resident cuda-oxide type.
///
/// `SharedArray<T, N, ALIGN>` is a host-side ZST — a `PhantomData` marker
/// whose storage the device backend materializes — so its footprint is
/// computed as `N * size_of::<T>()` from the generic arguments (a lower
/// bound: inter-array alignment padding can only add to it, which keeps the
/// over-limit verdict sound). `Barrier` is a real 8-byte object.
fn shared_resident_size(ty: Ty) -> SharedSize {
    let TyKind::RigidTy(RigidTy::Adt(adt, generic_args)) = ty.kind() else {
        return SharedSize::NotShared;
    };
    let type_path = adt.name();
    if paths::is_shared_array(&type_path) {
        let mut element_size = None;
        let mut len = None;
        for arg in &generic_args.0 {
            match arg {
                GenericArgKind::Type(t) if element_size.is_none() => {
                    element_size = t.layout().ok().map(|l| l.shape().size.bytes() as u64);
                }
                // First const parameter is N; the second is ALIGN.
                GenericArgKind::Const(c) if len.is_none() => {
                    len = const_as_usize(c);
                }
                _ => {}
            }
        }
        match (element_size, len) {
            (Some(element_size), Some(len)) => {
                SharedSize::Known(element_size.saturating_mul(len), "SharedArray")
            }
            _ => SharedSize::Unresolved("SharedArray"),
        }
    } else if paths::is_barrier(&type_path) {
        let size = ty
            .layout()
            .ok()
            .map_or(8, |l| l.shape().size.bytes() as u64);
        SharedSize::Known(size, "Barrier")
    } else {
        SharedSize::NotShared
    }
}

/// RC005 (warning): launch-contract inconsistency.
///
/// Two shapes, both derived from the generic arguments the `#[kernel]` macro
/// baked into the index-witness calls:
/// - **mismatch** — a declared `domain = N` wider than what the index
///   formula can prove thread-unique;
/// - **missing contract** — a shape-dependent index formula under
///   `UnknownDomain`, where uniqueness rests on the runtime fallback that
///   silently invalidates the witness on a mismatched launch.
fn rc005_launch_contract(kernel: &Kernel, findings: &mut Vec<Finding>) {
    let Some(body) = kernel.item.body() else {
        return;
    };
    let mut collector = IndexCallCollector {
        locals: body.locals().to_vec(),
        calls: Vec::new(),
    };
    collector.visit_body(&body);

    // One finding per (formula, domain) pair: the macro rewrites every call
    // site, so spans degrade to the kernel item and repeats add nothing.
    let mut seen = std::collections::BTreeSet::new();
    for (index_fn, domain) in collector.calls {
        if !seen.insert((index_fn, domain)) {
            continue;
        }
        let Some(proven_max) = index_fn.max_proven_dimensions() else {
            continue; // shape-independent formula
        };
        let user = index_fn.user_name();
        let span = kernel.item.span();
        match domain.dimensions() {
            Some(declared) if declared > proven_max => findings.push(Finding {
                code: "RC005".to_string(),
                confidence: Confidence::Warning,
                message: format!(
                    "kernel `{}` declares `{}` but calls `{user}()`, which is \
                     thread-unique only for launches of at most {}",
                    kernel.name,
                    domain.contract_syntax(),
                    axes(proven_max),
                ),
                kernel: Some(kernel.name.clone()),
                span: emit::source_span(span),
                notes: vec![format!(
                    "on a launch that uses all {declared} contracted axes the witness fails \
                     its runtime shape check and is silently invalid — guarded writes are \
                     skipped with no error"
                )],
                help: Some(if declared > 2 {
                    format!(
                        "narrow the contract; no recognized index formula covers {}",
                        axes(declared)
                    )
                } else {
                    format!(
                        "use an index formula that covers {}, or narrow the contract",
                        axes(declared)
                    )
                }),
                explain: "RC005".to_string(),
                provenance: Vec::new(),
            }),
            Some(_) => {}
            None => findings.push(Finding {
                code: "RC005".to_string(),
                confidence: Confidence::Warning,
                message: format!(
                    "kernel `{}` calls `{user}()` without a launch contract",
                    kernel.name
                ),
                kernel: Some(kernel.name.clone()),
                span: emit::source_span(span),
                notes: vec![
                    "without `#[launch_contract]` the launch shape is unknown, so the witness \
                     is only validated at runtime and is silently invalid on a mismatched \
                     launch — guarded writes are skipped with no error"
                        .to_string(),
                ],
                help: Some(format!(
                    "declare `#[launch_contract(domain = {proven_max}, coordinates = u32, \
                     block = (…))]` on the kernel"
                )),
                explain: "RC005".to_string(),
                provenance: Vec::new(),
            }),
        }
    }
}

fn axes(n: u8) -> &'static str {
    match n {
        1 => "one axis",
        2 => "two axes",
        _ => "three axes",
    }
}

/// Collects calls to launch-shape-dependent index-witness functions,
/// together with the launch domain baked into their generic arguments.
struct IndexCallCollector {
    locals: Vec<rustc_public::mir::LocalDecl>,
    calls: Vec<(IndexFn, LaunchDomain)>,
}

impl MirVisitor for IndexCallCollector {
    fn visit_terminator(
        &mut self,
        terminator: &Terminator,
        location: rustc_public::mir::visit::Location,
    ) {
        if let TerminatorKind::Call { func, .. } = &terminator.kind
            && let Ok(fn_ty) = func.ty(&self.locals)
            && let TyKind::RigidTy(RigidTy::FnDef(def, generic_args)) = fn_ty.kind()
            && let Some(index_fn) = paths::index_fn(&def.name())
        {
            let domain = generic_args
                .0
                .iter()
                .find_map(|arg| match arg {
                    GenericArgKind::Type(ty) => domain_of(*ty),
                    _ => None,
                })
                .unwrap_or(LaunchDomain::Unknown);
            self.calls.push((index_fn, domain));
        }
        self.super_terminator(terminator, location);
    }
}

fn domain_of(ty: Ty) -> Option<LaunchDomain> {
    match ty.kind() {
        TyKind::RigidTy(RigidTy::Adt(adt, _)) => paths::launch_domain(&adt.name()),
        _ => None,
    }
}
