# Phase 3 dependency review

Review date: 2026-08-24

This checkpoint resolves the six approved direct dependency versions in the
root `Cargo.lock`. Registry package metadata records the following licenses:

| Package | Resolved version | License |
| --- | --- | --- |
| `serde` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 |
| `postcard` | 1.1.3 | MIT OR Apache-2.0 |
| `tokio` | 1.53.1 | MIT |
| `snow` | 0.10.0 | Apache-2.0 OR MIT |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT |

The RustSec package index was reviewed on the review date. The applicable
published Snow advisory, RUSTSEC-2024-0011, affects versions before 0.9.5;
the locked 0.10.0 is in the patched range. The current Tokio information
advisory RUSTSEC-2025-0023 is patched in 1.44.2 and later; the locked 1.53.1 is
in that patched range. No published RustSec advisory found for the other
approved direct package/version pairs made this resolution ineligible.

This is a dependency/advisory review, not an independent audit of Noise,
cryptography, Tokio, Slint, or their transitive dependency implementations.
Exact version or feature changes require a new review. Deskkin source remains
MIT; the existing GPLv3 boundary for a combined simulator binary containing
Slint is unchanged, and no binary is distributed by this checkpoint.
