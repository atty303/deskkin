# Foundation E proposal: validated CoreS3 SDK download cache

Status: Proposed; not the current implementation authority

Date: 2026-08-30

## Goal and observable result

Reduce repeated CoreS3 CI download time without weakening toolchain provenance,
clean firmware conformance, or Foundation D's independent-lane contract. Cache
only the two public SDK archives whose SHA-256 digests are already pinned in
the maintained bootstrap. Validate restored bytes with a non-cached interpreter
before copying them into bootstrap input, and use the ordinary network download
path whenever a completed restore is absent, recoverably unavailable, partial,
malformed, logically oversize, or digest-invalid.

The checkpoint is observable when a cache-hit and a cache-miss run from the
same accepted inputs both:

- construct every installed SDK, west, Rust, and Python tool tree through the
  unchanged ordinary bootstrap;
- pass the existing installed-toolchain verification;
- perform clean product and inert firmware builds;
- produce successful CoreS3 conformance results with the same firmware inputs;
- leave the host lane and complete `mise run test` contract unchanged; and
- expose cache restore, archive validation, bootstrap, conformance, and cache
  save as distinct GitHub Actions steps.

This is durable development infrastructure. It changes no product behavior,
protocol byte, UI, product dependency graph, device state, local developer
cache, release policy, or provider authority.

## Current evidence and rejected broader cache

Foundation D's first qualified remote run completed the host lane in 4 minutes
8 seconds and the CoreS3 lane in 7 minutes 31 seconds. The CoreS3 lane owns disk
reclamation, mise setup, the pinned bootstrap, and clean product and inert
builds. The timing is an observation, not an acceptance threshold.

The current bootstrap keeps two digest-pinned SDK archives below
`.deskkin/downloads`; they occupy approximately 599 MiB on one workstation.
They are public inputs and the bootstrap verifies their complete SHA-256 before
extraction. This measurement motivates a 1 GiB staging limit but is not a
portable size guarantee.

Caching installed `.deskkin/sdk`, `.deskkin/west`, `.deskkin/rustup`, or
`.deskkin/venv` was considered and rejected. Those trees total about 8.5 GiB
locally, contain executable build inputs, and are not completely verified as
bytes before execution. In particular, `--verify-only` currently runs the
repository-local venv Python, so a cache containing that venv could control its
own verifier. A cache key or successful clean build would not repair that
provenance defect. Foundation E therefore caches no installed or executable
toolchain tree.

The broader `.deskkin` tree also contains builds, results, diagnostics,
profiles, identities, retained physical-device state, and historical local
state. It must never be selected or enumerated as a cache source.

## Scope

Foundation E includes:

- one CoreS3-job-only cache entry with an exact input key and no prefix restore;
- `cache: false` and `cache_save: false` on the CoreS3 job's existing
  `jdx/mise-action` step so the digest verifier is freshly installed rather
  than restored from a tool cache;
- one staging directory containing exactly the two pinned public SDK archives;
- validation with the mise-provided system Python before bootstrap can observe
  restored bytes;
- ordinary download fallback for every non-valid classification that reaches
  the trusted helper;
- a 1 GiB main-save and post-restore logical staging limit plus exact file-count,
  type, name, size, and digest checks;
- cache-hit, cache-miss, unavailable, partial, malformed, symlink, oversize,
  and digest-mismatch conformance checks;
- save restricted to successful `main` push jobs after CoreS3 conformance;
- pinned GitHub Action revisions and dependency-free workflow drift tests; and
- remote qualification of one miss followed by one hit after separately
  authorized push and rerun operations.

Foundation E does not include:

- installed SDK, west, Rust, Python, Cargo, Clang, or other executable trees;
- `.deskkin/downloads` as the cache restore target;
- `.deskkin` as a whole, build directories, Cargo target directories, results,
  diagnostics, profiles, identities, credentials, Wi-Fi material, NVS data, or
  firmware artifacts in a cache;
- cache restore keys, cross-OS reuse, cross-architecture reuse, mutable branch
  labels, or commit-SHA keys that prevent reuse;
- skipping ordinary bootstrap installation or installed-toolchain verification;
- caching clean product or inert build output;
- pull-request cache save, in-place repair of restored staging, or retaining an
  invalid staging entry;
- changes to `test:host`, `test:core-s3`, `test`, lane ownership, coverage, or
  path-independent CI execution;
- changes to the host job's existing mise setup or cache behavior;
- a local cache manager, automatic deletion of developer state, a prebuilt
  container image, GHCR publication, release artifact, or new remote service;
  or
- product dependencies, physical-device operations, provider access, push,
  release, or publication.

## Cache owner, staging, and archive allowlist

Only the `core-s3` CI job owns restore, validation, bootstrap input copying, and
save. The host job neither reads nor writes this cache.

The cache action restores only this CI staging directory:

```text
.deskkin/ci-cache/core-s3-sdk-downloads
```

Bootstrap never reads that directory. A small repository helper, executed by
the mise-provided system Python rather than any restored interpreter, recognizes
only these files:

```text
zephyr-sdk-1.0.1_linux-x86_64_minimal.tar.xz
toolchain_gnu_linux-x86_64_xtensa-espressif_esp32s3_zephyr-elf.tar.xz
```

Implementation moves their URLs, names, and expected SHA-256 values from shell
constants into one committed `requirements/core-s3-downloads.json` manifest.
Both the bootstrap and helper must read that manifest; neither retains a second
manually synchronized digest list.

The helper rejects a missing or extra entry, non-regular file, symlink at any
staging ancestor or entry, name mismatch, individual or aggregate oversize,
read failure, and digest mismatch. It does not execute, extract, import, or
inspect archive contents. Only after both complete file digests match does it
create a private sibling candidate directory below `.deskkin`, arm cleanup for
that candidate, copy and rehash both regular files there, and atomically rename
the complete candidate directory to `.deskkin/downloads`. It never publishes
one archive at a time. An existing `.deskkin/downloads` at this pre-bootstrap
boundary is not overwritten or repaired in place.

The helper arms cleanup for staging, the private candidate, and any unpublished
bootstrap input before touching each path. A copy, rehash, permission, or rename
failure removes both the candidate and staging and leaves
`.deskkin/downloads` absent so ordinary download remains the only fallback.
In restore mode, every classification removes the exact staging directory on
the ephemeral runner after the validated bytes have been published or rejected.
It resolves targets beneath the checkout's exact `.deskkin` roots without
following symlinks and never deletes an unknown sibling. Cleanup failure is a
closed cache-stage failure; it cannot be reported as an accepted cache or
silently continue into bootstrap.

## Exact cache key and action

The proposed implementation uses the split restore and save interfaces from
`actions/cache` v6.1.0, pinned to commit
`55cc8345863c7cc4c66a329aec7e433d2d1c52a9`. Approval must re-read its source,
license, runtime, and current GitHub compatibility before adding the action.
No Rust, Python, product, or device dependency is added.

The existing pinned `jdx/mise-action` remains the CoreS3 tool installer, but its
CoreS3 step must set both `cache: false` and `cache_save: false`. The workflow
drift test fixes those values and also proves the host job's existing mise step
is unchanged. The resulting mise Python is installed from the locked repository
tool declaration during that job and is never restored from either Foundation
E or mise-action cache state.

The single key has this semantic shape:

```text
deskkin-core-s3-sdk-downloads-v1-${runner.os}-${runner.arch}-${input_digest}
```

`input_digest` is computed by GitHub's exact file hash over the committed cache
and archive contracts:

```text
scripts/bootstrap_core_s3.sh
requirements/core-s3-downloads.json
```

The key does not include a branch name, run ID, or commit SHA. There are no
`restore-keys`; a non-exact entry is a miss. `v1` is a manually advanced cache
schema and staging-contract version.

Cache entries are immutable generations rather than replacements. The workflow
therefore also contains one reviewed `approved_save_input_digest` equal to the
exact `input_digest` approved after the quota readback. Restore always uses the
computed exact key, but save is enabled only when the computed digest equals
that reviewed value. A bootstrap or archive-manifest change consequently gets
an ordinary cache miss and cannot create another generation automatically.
Changing the approved value is a new checkpoint: read back quota and entries,
decide whether the previous Foundation E generation remains or is explicitly
removed, obtain approval for that external-state plan, and update the value in
a separate reviewed change. This proposal grants no automatic cache deletion.

## Restore, validation, fallback, and save

The CoreS3 job executes these stages in order:

1. restore the exact cache entry into the isolated staging directory with the
   cache action's failure kept non-authoritative;
2. run the trusted helper for every action outcome, including reported miss and
   restore error, so partially extracted or unexpected staging state cannot be
   mistaken for absence;
3. assign exactly one primary classification using the precedence table below;
4. only for `valid`, copy both verified archives atomically into a fresh private
   `.deskkin/downloads`; for every other non-cleanup classification, leave
   bootstrap input empty and continue through its ordinary download path;
5. require staging cleanup before continuing;
6. run `mise run phase3:device:bootstrap`, which independently rehashes any
   copied archive before extraction and constructs all installed tool trees;
7. run `scripts/bootstrap_core_s3.sh --verify-only` as the authoritative
   installed-toolchain check;
8. run the unchanged `mise run test:core-s3` clean conformance lane; and
9. only on a successful `push` to `refs/heads/main` whose computed input digest
   equals the reviewed save-generation digest, enter the separate save lifecycle
   defined below.

Restore hit or miss affects only whether bootstrap performs the two public
archive downloads. Recoverable cache restore failure, invalid bytes, partial
extraction, or cache save failure must not change ordinary bootstrap and
conformance. A cleanup failure, ordinary download failure, bootstrap failure,
or final installation verification failure remains a CoreS3 job failure.

Save is never enabled for `pull_request`. Pull requests may restore the
default-line Foundation E SDK entry but cannot create a merge-ref entry for this
cache. `workflow_dispatch` may restore an existing Foundation E entry for
controlled hit qualification but does not save or replace it.

### Save lifecycle

Save mode does not apply restore mode's immediate valid-staging cleanup. After
successful CoreS3 conformance, the trusted helper:

1. creates a fresh private save-candidate directory and arms its cleanup;
2. copies both bootstrap-verified `.deskkin/downloads` archives into it;
3. verifies shape, 1 GiB aggregate bound, and both complete digests again;
4. atomically renames the complete candidate to the cache staging path; and
5. on success only, transfers staging cleanup ownership to the workflow.

Any prepare failure cleans the candidate and staging immediately and skips save
without changing conformance. The cache save action then reads the published
staging path. A following cleanup step runs unconditionally after the save
attempt and removes the exact staging path whether save succeeds, fails, times
out, or is cancelled while GitHub still schedules post steps. Save failure is
non-authoritative; cleanup failure is a visible job failure because temporary
cache state was not proven removed. Runner teardown is the final cleanup owner
if GitHub terminates the job before any post step can run.

### Classification precedence

The helper receives the restore action outcome as `hit`, `miss`, or `error` and
assigns one primary classification in this exact order:

| Priority | Condition | Primary classification |
| ---: | --- | --- |
| 1 | restore action returned error or timed out | `unavailable` |
| 2 | action reported miss and staging has no entries | `absent` |
| 3 | action reported miss but staging contains any entry | `malformed` |
| 4 | any staging ancestor or entry is a symlink | `symlink` |
| 5 | an extra name, unsupported file type, or invalid directory shape exists | `malformed` |
| 6 | action reported hit but either expected file is absent | `partial` |
| 7 | an individual file or aggregate staging size exceeds the bound | `oversize` |
| 8 | a complete regular file cannot be read | `read_error` |
| 9 | either complete-file SHA-256 differs | `digest_mismatch` |
| 10 | both files pass every check | `valid` |

The helper still inspects metadata needed for symlink-safe cleanup after an
action error, but `unavailable` remains primary unless cleanup itself fails.
`cleanup_failed` is the sole final override of every primary classification.
After `valid`, any candidate copy, rehash, permission, or atomic publication
failure becomes `copy_failed`; it falls back only after candidate, staging, and
bootstrap input are confirmed absent. If that cleanup fails, the final result
is `cleanup_failed` and bootstrap does not start. Fixtures combining action
error, symlink, malformed shape, missing file, and oversize metadata must prove
that this precedence is stable.

## Provenance, security, and privacy

The cache is untrusted optimization input even though GitHub stores it for the
repository. The cache action and service control staging bytes, but no restored
code or executable runs. The non-cached mise Python validates complete archive
digests before those bytes reach bootstrap input, and the bootstrap hashes them
again before extraction. The committed digest contract, not cache identity,
remains provenance.

The cache contains no credential, user data, executable installed tree, or
build output. CI must not enumerate or upload the surrounding `.deskkin` tree.
Cache logs may contain the fixed cache key, closed classification, aggregate
bytes, and stable stage failure. They must not print environment dumps, archive
contents, filesystem inventories, profile or identity paths, or signed cache
URLs.

Repository cache capacity is shared external state. Restricting Foundation E
save to successful `main` pushes prevents pull-request-controlled churn from
this SDK entry. The unchanged host job's existing mise-action cache remains
outside Foundation E and may consume the same repository quota. The
implementation checkpoint must read back the repository's current cache quota,
usage, and existing entry owners before enabling save. If one 1 GiB-bounded SDK
generation cannot coexist with the retained host mise and other entries,
implementation stops for a revised scope rather than relying on eviction.
Because immutable generations accumulate rather than replace one another, that
readback and approval recur before changing the reviewed save-generation
digest. Until the retention or explicitly authorized deletion of the previous
generation is decided, a new key remains restore-miss and save-disabled.

`actions/cache` does not expose a maximum download or pre-extraction byte input.
The 1 GiB contract therefore bounds only entries prepared by Deskkin on `main`
and logical staging after a restore has completed; it is not a hard bound on the
cache action's temporary download. If the external cache service returns an
archive large enough to exhaust the runner before the helper can execute, the
job remains a closed CI setup failure. Foundation E does not claim fallback or
CoreS3 conformance for that run and must never convert it to success. The exact
key is written only by a size-validated successful `main` job, which makes this
an external corruption or service-failure boundary rather than an accepted
entry path. Any observed restore-time resource exhaustion triggers cache
removal before another qualification attempt.

## Observation contract

GitHub Actions, compiler, test runner, and cache workflow are development ACI,
not product runtime diagnostic runs. Foundation E adds no Deskkin recorder,
artifact, telemetry SDK, or remote diagnostic destination.

The result surface is the overall workflow and two job conclusions. The
operation surface remains the workflow trigger, cancellation, and job exit
status. The existing out-of-band observation surface is sufficient when these
CoreS3 steps stay distinct:

```text
Restore CoreS3 SDK download cache
Validate CoreS3 SDK download staging
Bootstrap CoreS3 toolchain
Verify CoreS3 toolchain
Test CoreS3 conformance
Prepare CoreS3 SDK download cache
Save CoreS3 SDK download cache
```

Stable cache classifications are `valid`, `absent`, `partial`, `malformed`,
`symlink`, `oversize`, `read_error`, `digest_mismatch`, `unavailable`,
`copy_failed`, and `cleanup_failed`. GitHub step status and logs identify the
owner and failure stage; the existing atomic CoreS3 result JSON remains
authoritative for firmware conformance. Cache classification is never written
into product diagnostics or used to change firmware success.

## Reproducible acceptance

Implementation must prove:

1. the workflow statically contains only the staging path, exact-key inputs,
   pinned action commit, no restore prefix, a 1 GiB logical/save limit, and a
   `main`-push-only save condition; the CoreS3 mise step has cache and cache-save
   disabled while the host mise step is unchanged;
2. an absent staging directory and a reported miss leave bootstrap input empty,
   exercise ordinary download, and pass clean product and inert builds;
3. an exact valid cache is fully hashed by non-cached Python, copied atomically,
   rehashed by bootstrap, and passes the same clean builds;
4. recoverable cache restore error after partial extraction, missing or extra
   file, directory entry, symlink at every ancestor or file, post-restore
   logical oversize, read failure, and either digest mismatch are rejected,
   cleaned, and followed by ordinary download;
5. every first-file and second-file copy, rehash, permission, candidate rename,
   and destination-publication failure removes candidate, staging, and any
   unpublished bootstrap input before ordinary download; no single cached
   archive remains reusable after a failed publication;
6. simultaneous action error, symlink, malformed shape, missing file, oversize,
   read failure, and digest mismatch fixtures always select the documented
   primary classification, with `cleanup_failed` as the only final override;
7. restore and save lifecycles target only their exact staging and candidate
   directories, reject path escape, preserve unknown siblings, arm cleanup
   before each mutation, transfer ownership only after complete save staging
   publication, and make cleanup failure visible;
8. no restored interpreter, executable, SDK tree, west checkout, Rust toolchain,
   build file, or archive content is executed before complete digest validation;
9. recoverable cache-service restore or save failure does not suppress ordinary
   bootstrap, installed verification, or conformance, while restore-time runner
   exhaustion remains a visible closed setup failure rather than false success;
10. pull-request and `workflow_dispatch` runs cannot save the Foundation E SDK
    entry, while a successful `main` push may save it only after CoreS3
    conformance and an exact match with the reviewed save-generation digest;
    drift tests prove an input change is save-disabled until quota, retained
    generations, and deletion authority are re-reviewed, and the quota test
    accounts separately for existing host mise cache entries;
11. no build, result, diagnostic, profile, identity, credential, device state,
   or unknown `.deskkin` entry is selected;
12. host-lane ownership and behavior are byte-for-byte unchanged at the
    workflow task-definition boundary;
13. `mise run test` still passes from the repository's documented local state;
    and
14. fresh durable review finds no remaining provenance, cache scope, privacy,
    authority, or cleanup defect.

After an explicitly requested push, remote qualification requires one observed
miss on `main` that saves the entry and one separately authorized
`workflow_dispatch` hit that validates it without saving. Record both job
conclusions, cache classification, CoreS3 result, elapsed times, and whether the
hit materially shortened the intended lane. A hit that is not materially faster
is evidence to remove the cache rather than retain its complexity.

## Removal criteria

Remove the cache and its helper logic when any of these conditions holds:

- restore plus complete digest validation is not materially faster than ordinary
  download in two comparable remote runs;
- the entry or repository cache pressure causes repeated eviction;
- GitHub cache semantics no longer support restore-only pull requests and
  `main`-push-only save;
- cache bytes cannot remain non-executable until complete digest validation;
- cache download or extraction exhausts runner resources before trusted
  validation and cleanup can run;
- fallback or staging cleanup becomes unreliable or materially harder to
  diagnose; or
- the upstream archives or bootstrap stop having one complete committed digest
  per cached file.

Removal restores Foundation D's uncached CoreS3 job. No product or local-state
migration is required.

## Ordered checkpoints

1. Review and approve or revise this proposal, including the two-file allowlist,
   digest SSOT, staging path, action revision, 1 GiB limit, main-only save,
   CoreS3 mise-cache disablement, unchanged host mise cache, validation,
   fallback, restore-time exhaustion boundary, cleanup, conformance matrix, and
   removal criteria.
2. Before implementation, read back the repository cache quota and current
   usage. Stop for revised approval if one bounded generation cannot coexist.
3. In one separate implementation commit, add the cache workflow, the smallest
   trusted staging helper, dependency-free drift and fault tests, and synchronized
   documentation. Record the current reviewed save-generation digest only after
   checkpoint 2 passes. Run the complete `mise run test` without creating a
   remote cache.
4. After a separate push authorization, qualify the miss path on `main`.
5. After the miss has saved an entry, obtain separate `workflow_dispatch`
   authority to qualify the exact hit path and record the comparison.

Any later change to the bootstrap or archive manifest returns to checkpoint 2
before its new digest can be approved for save. The default is to leave the new
generation disabled; deleting or retaining an older remote entry is a separate
external-state decision.

Approval of this proposal authorizes only the repository-local implementation
and local reproducible tests in checkpoint 3 after the quota readback passes.
It does not authorize a push, remote cache creation, workflow rerun, release,
artifact publication, physical-device operation, provider access, or product
dependency.

## Approval decision

Approve or revise Foundation E as the bounded two-archive cache above. The
recommended decision is one all-or-nothing entry, 1 GiB maximum, restore-only
pull requests and workflow dispatches, save only after successful CoreS3
conformance on `main` with an explicitly approved input generation, no
mise-action cache in the CoreS3 job, unchanged host mise caching and retained
Foundation E generations accounted in recurring quota preflight, explicit
acceptance that the cache action cannot pre-bound its temporary download, and
removal if the qualified hit has no material benefit or causes restore-time
resource exhaustion.
