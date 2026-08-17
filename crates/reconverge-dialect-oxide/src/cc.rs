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
pub fn parse_compute_capability(s: &str) -> Result<ComputeCapability, String> {
    let (major, minor) = s
        .split_once('.')
        .ok_or_else(|| format!("`{s}` is not a compute capability; expected e.g. `8.6`"))?;
    let parse = |part: &str, what: &str| {
        part.parse::<u8>()
            .map_err(|_| format!("`{s}` has a non-numeric {what} part; expected e.g. `8.6`"))
    };
    Ok((parse(major, "major")?, parse(minor, "minor")?))
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
        assert!(parse_compute_capability("86").is_err());
        assert!(parse_compute_capability("8.x").is_err());
    }
}
