//! Extract the kernel side of upstream cuda-oxide examples into kernel-only
//! conformance crates.
//!
//! Why extraction: upstream's examples are host+device programs whose host
//! half (cuda-bindings) hard-requires the CUDA SDK headers at build time —
//! `bindgen` runs against `cuda.h` in every build, with no SDK-free
//! fallback. reconverge's hard rules forbid requiring the SDK anywhere, so
//! conformance compiles just the device side: each example's
//! `#[cuda_module] mod … { … }` spliced verbatim (by source span) into a
//! crate that depends only on `cuda-device`.
//!
//! What is carried over, besides the module(s): top-level `use` items
//! rooted at `core`/`std`/`cuda_device`; top-level type-and-value items
//! (`const`, `static`, `type`, `struct`, `enum`, `union`, `trait`, `impl`,
//! `macro_rules!`, `extern` blocks); and `#[device]`-attributed functions —
//! the things kernel modules reach through their `use super::*`. Plain
//! (host) functions are deliberately not carried. Examples whose kernels
//! reach a host item fail to compile and are pruned (and counted) by the
//! conformance runner; the extraction floor in `run-conformance.sh` keeps
//! that honest.
//!
//! Nothing extracted is committed to the reconverge repository: the corpus
//! is regenerated from the pinned checkout on every run. Upstream sources
//! are Apache-2.0; each generated crate reproduces its source header.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;

use crate::util::{LineOffsets, has_attr};

pub fn run(upstream: &Path, out_dir: &Path) -> Result<(usize, usize), String> {
    let examples_dir = upstream.join("crates/rustc-codegen-cuda/examples");
    let cuda_device = upstream.join("crates/cuda-device");
    if !cuda_device.is_dir() {
        return Err(format!(
            "{} is not an upstream checkout",
            upstream.display()
        ));
    }

    let mut examples: Vec<PathBuf> = fs::read_dir(&examples_dir)
        .map_err(|e| format!("cannot read {}: {e}", examples_dir.display()))?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.join("src/main.rs").is_file().then_some(path)
        })
        .collect();
    examples.sort();

    let crates_dir = out_dir.join("crates");
    let _ = fs::remove_dir_all(out_dir);
    fs::create_dir_all(&crates_dir).map_err(|e| e.to_string())?;

    let mut members = Vec::new();
    let mut report = String::new();
    let mut skipped = 0usize;
    for example in &examples {
        // Normalized (underscored) names keep package, crate, cargo target,
        // and directory identical, so the runner's prune step can map
        // failing targets straight back to member directories.
        let name = example
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace('-', "_");
        let source = fs::read_to_string(example.join("src/main.rs")).map_err(|e| e.to_string())?;
        match extract_lib_rs(&source) {
            Ok(lib_rs) => {
                let crate_dir = crates_dir.join(&name);
                fs::create_dir_all(&crate_dir).map_err(|e| e.to_string())?;
                fs::write(crate_dir.join("lib.rs"), lib_rs).map_err(|e| e.to_string())?;
                fs::write(
                    crate_dir.join("Cargo.toml"),
                    member_manifest(&name, &cuda_device),
                )
                .map_err(|e| e.to_string())?;
                members.push(name.clone());
                let _ = writeln!(report, "{name}\textracted\t-");
            }
            Err(reason) => {
                skipped += 1;
                let _ = writeln!(report, "{name}\tskipped\t{reason}");
            }
        }
    }

    fs::write(out_dir.join("Cargo.toml"), workspace_manifest(&members))
        .map_err(|e| e.to_string())?;
    fs::write(out_dir.join("extraction-report.tsv"), report).map_err(|e| e.to_string())?;
    Ok((members.len(), skipped))
}

fn member_manifest(example: &str, cuda_device: &Path) -> String {
    format!(
        "[package]\n\
         name = \"conformance_{example}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\n\
         [lib]\n\
         path = \"lib.rs\"\n\n\
         [dependencies]\n\
         cuda-device = {{ path = \"{}\" }}\n",
        cuda_device.display()
    )
}

fn workspace_manifest(members: &[String]) -> String {
    let mut manifest = String::from("[workspace]\nresolver = \"3\"\nmembers = [\n");
    for member in members {
        let _ = writeln!(manifest, "    \"crates/{member}\",");
    }
    manifest.push_str("]\n");
    manifest
}

/// Build the conformance crate's `lib.rs` from an example's `main.rs`.
fn extract_lib_rs(source: &str) -> Result<String, String> {
    let file = syn::parse_file(source).map_err(|e| format!("parse error: {e}"))?;
    let offsets = LineOffsets::new(source);

    let mut header = source
        .lines()
        .take_while(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with("*/")
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !header.is_empty() {
        header.push('\n');
    }

    // Inner file attributes (`#![feature(…)]` and friends) gate language
    // features the kernels may rely on; carry them verbatim.
    let mut inner_attrs = Vec::new();
    for attr in &file.attrs {
        inner_attrs.push(offsets.slice(source, attr.span().start(), attr.span().end()));
    }

    let mut carried = Vec::new();
    let mut modules = Vec::new();
    for item in &file.items {
        match item {
            syn::Item::Use(item_use) if allowed_use_root(&item_use.tree) => {
                carried.push(offsets.slice(source, item.span().start(), item.span().end()));
            }
            syn::Item::Const(_)
            | syn::Item::Type(_)
            | syn::Item::Static(_)
            | syn::Item::Struct(_)
            | syn::Item::Enum(_)
            | syn::Item::Union(_)
            | syn::Item::Trait(_)
            | syn::Item::Impl(_)
            | syn::Item::Macro(_)
            | syn::Item::ForeignMod(_) => {
                carried.push(offsets.slice(source, item.span().start(), item.span().end()));
            }
            syn::Item::Fn(item_fn) if has_attr(&item_fn.attrs, "device") => {
                carried.push(offsets.slice(source, item.span().start(), item.span().end()));
            }
            syn::Item::Mod(item_mod)
                if has_attr(&item_mod.attrs, "cuda_module") && item_mod.content.is_some() =>
            {
                // Start at the `mod` keyword: this drops the outer
                // attributes (including #[cuda_module], whose expansion
                // needs cuda-host) and the visibility qualifier, neither of
                // which matters at the root of the generated library.
                modules.push(offsets.slice(
                    source,
                    item_mod.mod_token.span.start(),
                    item_mod.span().end(),
                ));
            }
            _ => {}
        }
    }
    if modules.is_empty() {
        return Err("no #[cuda_module] mod with inline content".to_string());
    }

    let mut lib_rs = header;
    lib_rs.push_str("// Extracted by reconverge's conformance-extractor; device side only.\n");
    for attr in inner_attrs {
        lib_rs.push_str(&attr);
        lib_rs.push('\n');
    }
    lib_rs.push_str("#![allow(dead_code, unused_imports)]\n\n");
    for item in carried {
        lib_rs.push_str(&item);
        lib_rs.push('\n');
    }
    lib_rs.push('\n');
    for module in modules {
        lib_rs.push_str(&module);
        lib_rs.push('\n');
    }
    Ok(lib_rs)
}

fn allowed_use_root(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Path(path) => {
            matches!(
                path.ident.to_string().as_str(),
                "core" | "std" | "cuda_device"
            )
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::extract_lib_rs;

    const SAMPLE: &str = r#"/*
 * SPDX-FileCopyrightText: Copyright (c) 2026 EXAMPLE HOLDER
 * SPDX-License-Identifier: Apache-2.0
 */
use cuda_core::{CudaContext, LaunchConfig};
use cuda_device::{DisjointSlice, kernel, thread};
use cuda_host::cuda_module;

const THREADS: usize = 128;

#[cuda_module]
mod kernels {
    use super::*;

    #[kernel]
    pub fn scale(mut out: DisjointSlice<f32>) {
        let i = thread::index_1d();
        if let Some(e) = out.get_mut(i) {
            *e *= 2.0;
        }
    }
}

fn main() {
    println!("host side");
}
"#;

    #[test]
    fn extracts_device_side_only() {
        let lib = extract_lib_rs(SAMPLE).unwrap();
        assert!(lib.contains("SPDX-License-Identifier: Apache-2.0"));
        assert!(lib.contains("use cuda_device::{DisjointSlice, kernel, thread};"));
        assert!(lib.contains("const THREADS: usize = 128;"));
        assert!(lib.contains("mod kernels {"));
        assert!(lib.contains("pub fn scale"));
        assert!(!lib.contains("cuda_core"));
        assert!(!lib.contains("cuda_host"));
        assert!(!lib.contains("#[cuda_module]"));
        assert!(!lib.contains("fn main"));
    }

    #[test]
    fn refuses_examples_without_kernel_modules() {
        let err = extract_lib_rs("fn main() {}").unwrap_err();
        assert!(err.contains("no #[cuda_module] mod"));
    }
}
