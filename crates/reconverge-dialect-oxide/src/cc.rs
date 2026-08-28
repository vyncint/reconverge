//! Compute-capability limits table (RC004).
//!
//! Sources are public NVIDIA documentation only: the CUDA C++ Programming
//! Guide's "Features and Technical Specifications" table and the per-arch
//! tuning guides. Two facts matter to reconverge:
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
        cc: (12, 0),
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
/// Failures are told apart, because the reader needs different things from
/// each. A part that is not a number at all (`8.x`, `8.`) is reported as
/// non-numeric. A part that *is* a number but cannot be a compute
/// capability — out of `u8` range (`256`, `999`) or negative (`-1`) — is
/// reported against the table instead, so the reader still sees the
/// capabilities that would have worked rather than being told their digits
/// are not digits.
pub fn parse_compute_capability(s: &str) -> Result<ComputeCapability, String> {
    let (major, minor) = s
        .split_once('.')
        .ok_or_else(|| format!("`{s}` is not a compute capability; expected e.g. `8.6`"))?;
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
    Ok((parse(major, "major")?, parse(minor, "minor")?))
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
        assert!(parse_compute_capability("86").is_err());
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
