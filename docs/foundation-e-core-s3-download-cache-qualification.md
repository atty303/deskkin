# Foundation E CoreS3 SDK download cache qualification evidence

Date: 2026-08-30

Foundation E was qualified remotely on the same `main` commit,
`bb9a7ec629667a14c8679ae6cc368640700b8151`, after separately authorized miss
and hit executions. The push-triggered miss was GitHub Actions run
[`33309043258`](https://github.com/atty303/deskkin/actions/runs/33309043258).
The later `workflow_dispatch` hit was run
[`33309526078`](https://github.com/atty303/deskkin/actions/runs/33309526078).

Both runs passed the unchanged host and clean CoreS3 conformance jobs. Neither
run changed product behavior, protocol, UI, dependencies, credentials,
profiles, identities, device state, or firmware behavior. They created no
release or product artifact and performed no physical-device or provider
operation.

## Observed results

| Observation | Cache miss | Exact cache hit |
| --- | ---: | ---: |
| Host job | 4 min 2 sec | 4 min 50 sec |
| CoreS3 job | 8 min 35 sec | 7 min 50 sec |
| Reclaim runner disk | 1 min 16 sec | 31 sec |
| Bootstrap CoreS3 toolchain | 3 min 6 sec | 3 min 7 sec |
| Test CoreS3 conformance | 3 min 47 sec | 3 min 47 sec |

The miss saved a 121,116,049-byte entry for the exact OS, architecture, and
bootstrap-input key. The hit log reported `Cache restored from key` for that
same entry, and GitHub advanced its last-access time without changing its
creation time.

The hit CoreS3 job was 45 seconds shorter overall, but the disk-reclamation step
was also exactly 45 seconds shorter. The bootstrap step, which was the intended
optimization target, changed from 3 minutes 6 seconds to 3 minutes 7 seconds.
The cache therefore demonstrated correct non-authoritative behavior but no
material speedup attributable to the cached SDK archives.

These elapsed times are observations from two GitHub-hosted runs, not portable
performance thresholds. The step-level comparison is used only for the
proposal's retain-or-remove decision.

## Qualification conclusion

Foundation E met its behavioral acceptance criteria: miss and exact-hit paths
both ran the ordinary bootstrap and passed the same clean CoreS3 conformance
boundary, while bootstrap retained archive-integrity ownership. It did not meet
the proposal's retention criterion because the hit was not materially faster.

The `actions/cache` step and its dedicated workflow assertion were removed.
The ordinary bootstrap, CoreS3 conformance job, and complete `mise run test`
contract remain unchanged. The remote cache entry was not deleted; GitHub owns
its normal retention and eviction lifecycle.
