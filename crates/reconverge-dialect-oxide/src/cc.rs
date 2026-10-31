//! Compute-capability limits table (RC004).
//!
//! Sources are public NVIDIA documentation only: the CUDA C++ Programming
//! Guide's "Features and Technical Specifications" table and the per-arch
//! tuning guides. 10.3, 11.0 and 12.1 come from the CUDA 13 guide's "Memory
//! Information per Compute Capability" table, where 10.3 and 11.0 have
//! columns of their own and 12.1 is the second member of the `12.x` column
//! (its "Family-Specific Compatibility" table names `compute_120f` as
//! covering 12.0 and 12.1). 10.7 is in that table too and is left out on
//! purpose: its 327 KB per block needs an oversized-shared-memory opt-in
//! beyond the ordinary dynamic one, which this table does not model. Two
//! facts matter to reconverge:
//!
//! - **Static shared memory is capped at 48 KiB per kernel on every listed
//!   architecture.** Larger blocks of shared memory exist, but only as
//!   *dynamic* shared memory with an explicit opt-in at launch time; a
//!   statically sized allocation above 48 KiB can never load.
//! - The **per-block capacity** (the opt-in maximum) differs per
//!   architecture; it bounds static + dynamic together and gives the
//!   diagnostic its "what this target could ever offer" context.

/// Hard per-kernel cap for statically sized shared memory, in bytes,
/// uniform across all supported architectures.
pub const STATIC_SHARED_LIMIT_BYTES: u64 = 48 * 1024;

/// A compute capability, e.g. `(8, 6)` for `--cc 8.6`.
pub type ComputeCapability = (u8, u8);

/// Per-architecture shared-memory capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedMemoryLimits {
    /// The compute capability these limits describe.
    pub cc: ComputeCapability,
    /// Maximum shared memory per thread block (bytes), including the
    /// dynamic opt-in region.
    pub max_per_block: u64,
}

const KIB: u64 = 1024;

/// Shared-memory capacity table, ascending by compute capability.
pub const SHARED_MEMORY_LIMITS: &[SharedMemoryLimits] = &[
    SharedMemoryLimits {
        cc: (7, 0),
        max_per_block: 96 * KIB,
    },
    SharedMemoryLimits {
        cc: (7, 2),
        max_per_block: 96 * KIB,
    },
    SharedMemoryLimits {
        cc: (7, 5),
        max_per_block: 64 * KIB,
    },
    SharedMemoryLimits {
        cc: (8, 0),
        max_per_block: 163 * KIB,
    },
    SharedMemoryLimits {
        cc: (8, 6),
        max_per_block: 99 * KIB,
    },
    SharedMemoryLimits {
        cc: (8, 7),
        max_per_block: 163 * KIB,
    },
    SharedMemoryLimits {
        cc: (8, 9),
        max_per_block: 99 * KIB,
    },
    SharedMemoryLimits {
        cc: (9, 0),
        max_per_block: 227 * KIB,
    },
    SharedMemoryLimits {
        cc: (10, 0),
        max_per_block: 227 * KIB,
    },
    SharedMemoryLimits {
        cc: (10, 3),
        max_per_block: 227 * KIB,
    },
    SharedMemoryLimits {
        cc: (11, 0),
        max_per_block: 227 * KIB,
    },
    SharedMemoryLimits {
        cc: (12, 0),
        max_per_block: 99 * KIB,
    },
    SharedMemoryLimits {
        cc: (12, 1),
        max_per_block: 99 * KIB,
    },
];

/// Look up the shared-memory capacity for a compute capability.
#[must_use]
pub fn shared_memory_limits(cc: ComputeCapability) -> Option<SharedMemoryLimits> {
    SHARED_MEMORY_LIMITS.iter().copied().find(|l| l.cc == cc)
}

/// The compute capabilities the table covers, for error messages.
#[must_use]
pub fn known_compute_capabilities() -> Vec<String> {
    SHARED_MEMORY_LIMITS
        .iter()
        .map(|l| format!("{}.{}", l.cc.0, l.cc.1))
        .collect()
}

/// Parse a `--cc` value like `"8.6"`.
///
/// The CUDA spellings are accepted too. `sm_86` is what `nvcc -arch` takes,
/// what `ptxas` prints and what a PTX `.target` directive says, `compute_86`
/// is the virtual-architecture spelling of the same target, and `86` is the
/// number with the prefix dropped; all three normalize to `8.6`. A
/// capability is one major digit or two, followed by exactly one minor
/// digit, so a run of digits is unambiguous: the last one is the minor.
/// `launchbound` — which passes this flag straight through to this
/// parser — has accepted these spellings since 2.0, and a user moving
/// between the pair should not have to retype the number.
///
/// An `a` (architecture-specific) or `f` (family) suffix — `sm_90a`,
/// `sm_100f`, `9.0a` — is accepted and dropped. cuda-oxide's `--arch` is
/// spelled that way throughout, and the suffix selects an instruction set on
/// the *same* device: the memory table has one column per compute
/// capability, not per suffix, so the limits are the base capability's. The
/// suffix is still checked rather than ignored, because it only exists from
/// a known generation on (`a` from `sm_90a`, `f` from `sm_100f`), and
/// `sm_86a` is a typo for something, not a request for 8.6.
///
/// Failures are told apart, because the reader needs different things from
/// each. A part that is not a number at all (`8.x`, `8.`) is reported as
/// non-numeric. A part that *is* a number but cannot be a compute
/// capability — out of `u8` range (`256`, `999`) or negative (`-1`) — is
/// reported against the table instead, so the reader still sees the
/// capabilities that would have worked rather than being told their digits
/// are not digits.
pub fn parse_compute_capability(s: &str) -> Result<ComputeCapability, String> {
    let (normalized, suffix) = normalize_cuda_spelling(s);
    let (major, minor) = normalized.split_once('.').ok_or_else(|| {
        format!(
            "`{s}` is not a compute capability; expected e.g. `8.6` \
             (`sm_86`, `compute_86`, `86` and the `sm_90a`/`sm_100f` \
             suffixes are accepted too)"
        )
    })?;
    let parse = |part: &str, what: &str| {
        part.parse::<u8>().map_err(|_| {
            if is_integer_literal(part) {
                format!(
                    "`{s}` is not in the compute-capability table; known: {}",
                    known_compute_capabilities().join(", ")
                )
            } else {
                format!("`{s}` has a non-numeric {what} part; expected e.g. `8.6`")
            }
        })
    };
    let cc = (parse(major, "major")?, parse(minor, "minor")?);
    if let Some(suffix) = suffix {
        let (first_major, first) = match suffix {
            Suffix::ArchSpecific => (9, "sm_90a"),
            Suffix::Family => (10, "sm_100f"),
        };
        if cc.0 < first_major {
            return Err(format!(
                "`{s}` names no architecture: the `{}` suffix starts at `{first}`; \
                 drop it to mean {}.{}",
                suffix.letter(),
                cc.0,
                cc.1
            ));
        }
    }
    Ok(cc)
}

/// The variant suffix of an architecture name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Suffix {
    /// `a`: features of exactly this capability (`sm_90a`).
    ArchSpecific,
    /// `f`: features shared by the capability's family (`sm_100f`).
    Family,
}

impl Suffix {
    fn letter(self) -> char {
        match self {
            Suffix::ArchSpecific => 'a',
            Suffix::Family => 'f',
        }
    }
}

/// Normalize `sm_86`, `compute_86` and `86` to `8.6`, and split off an
/// `a`/`f` suffix, leaving anything else untouched for the parser to reject
/// with the message it would have used anyway.
///
/// Only a bare run of two or three ASCII digits is rewritten. One digit
/// (`8`) is a major with no minor and stays an error; four (`1234`) is not a
/// capability in any spelling. The prefixes and the suffix are matched
/// case-insensitively because `SM_86` is what a datasheet prints. A suffix
/// is only split off a digit, so `sm_a` stays one malformed word.
fn normalize_cuda_spelling(s: &str) -> (String, Option<Suffix>) {
    let mut body = s;
    for prefix in ["sm_", "compute_"] {
        // `get`, not indexing: `--cc ¼86` must be refused, not panic on a
        // byte offset inside a character.
        if body.len() > prefix.len()
            && body
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            body = &body[prefix.len()..];
            break;
        }
    }
    let bytes = body.as_bytes();
    let suffix = match bytes {
        [.., digit, last] if digit.is_ascii_digit() => match last.to_ascii_lowercase() {
            b'a' => Some(Suffix::ArchSpecific),
            b'f' => Some(Suffix::Family),
            _ => None,
        },
        _ => None,
    };
    if suffix.is_some() {
        body = &body[..body.len() - 1];
    }
    let normalized = if matches!(body.len(), 2 | 3) && body.bytes().all(|b| b.is_ascii_digit()) {
        let (major, minor) = body.split_at(body.len() - 1);
        format!("{major}.{minor}")
    } else {
        body.to_string()
    };
    (normalized, suffix)
}

/// Whether `part` is an integer literal — an optional sign followed by ASCII
/// digits — and so a real number even when it overflows `u8` or is negative,
/// as opposed to input that is not numeric at all.
fn is_integer_literal(part: &str) -> bool {
    let digits = part.strip_prefix(['+', '-']).unwrap_or(part);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cuda_spellings_normalize() {
        for spelling in ["8.6", "86", "sm_86", "SM_86", "Sm_86"] {
            assert_eq!(
                parse_compute_capability(spelling),
                Ok((8, 6)),
                "`{spelling}` should mean 8.6"
            );
        }
        // Two majors and one minor, so the last digit is always the minor.
        assert_eq!(parse_compute_capability("100"), Ok((10, 0)));
        assert_eq!(parse_compute_capability("sm_120"), Ok((12, 0)));
        assert_eq!(parse_compute_capability("75"), Ok((7, 5)));
    }

    #[test]
    fn the_spellings_cuda_oxide_uses_are_accepted() {
        // cargo-oxide's `--arch` values, the virtual-architecture prefix,
        // and a suffix on the dotted form: each is the base capability.
        for (spelling, cc) in [
            ("sm_90a", (9, 0)),
            ("SM_90A", (9, 0)),
            ("sm_100a", (10, 0)),
            ("sm_100f", (10, 0)),
            ("sm_103f", (10, 3)),
            ("sm_120a", (12, 0)),
            ("sm_121f", (12, 1)),
            ("compute_86", (8, 6)),
            ("COMPUTE_90a", (9, 0)),
            ("compute_120f", (12, 0)),
            ("90a", (9, 0)),
            ("9.0a", (9, 0)),
            ("10.0f", (10, 0)),
        ] {
            assert_eq!(
                parse_compute_capability(spelling),
                Ok(cc),
                "`{spelling}` should mean {}.{}",
                cc.0,
                cc.1
            );
        }
    }

    #[test]
    fn a_suffix_before_its_generation_is_refused() {
        // `a` begins at sm_90a and `f` at sm_100f; earlier ones name no
        // device, so they are not quietly read as the base capability.
        for (spelling, first) in [
            ("sm_86a", "sm_90a"),
            ("8.0a", "sm_90a"),
            ("sm_90f", "sm_100f"),
            ("compute_89f", "sm_100f"),
        ] {
            let err = parse_compute_capability(spelling).unwrap_err();
            assert!(
                err.contains("names no architecture") && err.contains(first),
                "`{spelling}`: {err}"
            );
        }
        // And a suffix is only split off a number.
        for spelling in [
            "sm_a", "a", "compute_", "sm_9.a", "sm_90b", "s¼86", "co¼¼86",
        ] {
            assert!(
                parse_compute_capability(spelling).is_err(),
                "`{spelling}` must not parse"
            );
        }
    }

    #[test]
    fn every_documented_capability_has_limits() {
        // 10.3, 11.0 and 12.1 are in the CUDA 13 guide's memory table (12.1
        // as part of `12.x`); a suffixed spelling of each resolves to them.
        for spelling in ["10.3", "sm_103a", "11.0", "sm_110f", "12.1", "sm_121a"] {
            let cc = parse_compute_capability(spelling).unwrap();
            assert!(
                shared_memory_limits(cc).is_some(),
                "`{spelling}` parsed to {cc:?}, which the table must cover"
            );
        }
        assert_eq!(
            shared_memory_limits((10, 3)).unwrap().max_per_block,
            227 * KIB
        );
        assert_eq!(
            shared_memory_limits((11, 0)).unwrap().max_per_block,
            227 * KIB
        );
        assert_eq!(
            shared_memory_limits((12, 1)).unwrap().max_per_block,
            99 * KIB
        );
    }

    #[test]
    fn a_digit_run_that_cannot_be_a_capability_is_still_refused() {
        // One digit is a major with no minor; four is not a capability in
        // any spelling. Neither is rewritten, so both reach the parser and
        // fail with the message they always did -- now naming the spellings.
        for spelling in ["8", "1234", "sm_8", "sm_1234", "8x", "sm_", "", "x86"] {
            let err = parse_compute_capability(spelling).unwrap_err();
            assert!(
                err.contains("is not a compute capability"),
                "`{spelling}` should be refused as malformed, got: {err}"
            );
            assert!(
                err.contains("sm_86"),
                "the refusal should name the accepted spellings, got: {err}"
            );
        }
    }

    #[test]
    fn normalizing_is_a_spelling_change_and_nothing_more() {
        // It rewrites the text and hands the result to the same parser, so
        // every judgement stays where it was. `99.9` is well formed and
        // absent from the table, and this function has never been the place
        // that consults the table -- the caller is -- so it still says Ok.
        assert_eq!(parse_compute_capability("99.9"), Ok((99, 9)));
        // And a digit run wide enough to overflow `u8` still reaches the
        // table message rather than being called non-numeric.
        let err = parse_compute_capability("999.9").unwrap_err();
        assert!(err.contains("not in the compute-capability table"), "{err}");
    }

    #[test]
    fn table_is_ascending_and_complete_enough() {
        let ccs: Vec<_> = SHARED_MEMORY_LIMITS.iter().map(|l| l.cc).collect();
        let mut sorted = ccs.clone();
        sorted.sort_unstable();
        assert_eq!(ccs, sorted, "table must stay ascending");
        // Every capacity can hold at least the universal static cap.
        for l in SHARED_MEMORY_LIMITS {
            assert!(l.max_per_block >= STATIC_SHARED_LIMIT_BYTES);
        }
    }

    #[test]
    fn lookup_and_parse() {
        assert_eq!(
            shared_memory_limits((8, 6)).unwrap().max_per_block,
            99 * 1024
        );
        assert_eq!(shared_memory_limits((3, 5)), None);
        assert_eq!(parse_compute_capability("8.6"), Ok((8, 6)));
        assert_eq!(parse_compute_capability("12.0"), Ok((12, 0)));
        // A leading `+` is numeric and in range: `u8::from_str` accepts it.
        assert_eq!(parse_compute_capability("+8.6"), Ok((8, 6)));
        // `86` was an error until the CUDA spellings were accepted; it is
        // now 8.6, and `the_cuda_spellings_normalize` below is its contract.
        assert_eq!(parse_compute_capability("86"), Ok((8, 6)));
        assert!(parse_compute_capability("8.x").is_err());
    }

    #[test]
    fn out_of_range_is_reported_against_the_table_not_as_non_numeric() {
        // 999 and 256 overflow u8; -1 is negative. All three are numbers,
        // just impossible capabilities, so the reader should see the table
        // that would have helped rather than "non-numeric".
        for s in ["999.999", "256.0", "-1.0"] {
            let msg = parse_compute_capability(s).unwrap_err();
            assert!(
                msg.contains("not in the compute-capability table"),
                "`{s}` should fall through to the table message, got: {msg}"
            );
            assert!(
                !msg.contains("non-numeric"),
                "`{s}` should not be called non-numeric, got: {msg}"
            );
        }
        // Genuinely non-numeric input is still named as such.
        for s in ["8.x", "8.", "x.6"] {
            let msg = parse_compute_capability(s).unwrap_err();
            assert!(msg.contains("non-numeric"), "`{s}` got: {msg}");
        }
    }
}
