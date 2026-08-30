# Foundation E proposal: CoreS3 SDK download cache

Status: Approved and implemented locally; remote qualification pending

Date: 2026-08-30

## Goal

Reduce repeated CoreS3 CI download time with the standard GitHub Actions cache
while leaving bootstrap and conformance unchanged.

Foundation E is successful when cache-hit and cache-miss CoreS3 jobs both run
the existing bootstrap and pass the existing `mise run test:core-s3` boundary.
The cache is only an optimization; it does not determine firmware success.

This is durable development infrastructure. It changes no product behavior,
protocol, UI, device state, local developer state, provider authority, or
product dependency.

## Current evidence

Foundation D's first qualified remote run completed the host lane in 4 minutes
8 seconds and the CoreS3 lane in 7 minutes 31 seconds. The two public SDK
archives under `.deskkin/downloads` occupy approximately 599 MiB on one
workstation.

The existing `scripts/bootstrap_core_s3.sh` already owns archive integrity. It
checks the pinned SHA-256 of an existing archive before using it. If the file is
absent or invalid, it downloads a temporary replacement, verifies that file,
and atomically replaces the destination. Foundation E does not add another
digest verifier or cache-specific validation path.

## Proposed workflow

Add one pinned `actions/cache` step to the CoreS3 job before bootstrap:

```yaml
- name: Cache CoreS3 SDK downloads
  uses: actions/cache@<reviewed-commit>
  with:
    path: .deskkin/downloads
    key: deskkin-core-s3-downloads-v1-${{ runner.os }}-${{ runner.arch }}-${{ hashFiles('scripts/bootstrap_core_s3.sh') }}
```

The implementation pins `actions/cache` v6.1.0 at commit
`55cc8345863c7cc4c66a329aec7e433d2d1c52a9`. The key has no branch, run, or
commit identifier and uses no `restore-keys` prefix. A bootstrap change
produces a new exact key.

After restore, CI runs the same commands as today:

```text
mise run phase3:device:bootstrap
mise run test:core-s3
```

The standard cache post-action saves `.deskkin/downloads` only after a
successful job. GitHub owns cache scope, retention, eviction, transfer, and
runner cleanup. Deskkin adds no cache helper, manifest, lifecycle manager,
classification system, quota manager, or remote deletion automation.

## Scope

Foundation E includes only:

- the two SDK archives already owned by `.deskkin/downloads`;
- one exact OS-, architecture-, and bootstrap-input-based cache key;
- one pinned standard cache action in the CoreS3 job;
- the unchanged bootstrap archive digest checks;
- the unchanged clean CoreS3 conformance boundary; and
- remote timing comparison after separately authorized push and workflow runs.

Foundation E does not include:

- installed SDK, west, Rust, Python, Cargo, or other executable tool trees;
- build output, firmware artifacts, results, diagnostics, profiles, identities,
  credentials, Wi-Fi material, NVS data, or the surrounding `.deskkin` tree;
- a custom validator, staging helper, cache classification, size policy,
  generation approval, cleanup implementation, or cache deletion command;
- changes to the existing mise cache, host job, bootstrap behavior, test
  coverage, lane ownership, or `mise run test` contract;
- a local cache manager, prebuilt image, release artifact, or new service; or
- push, workflow execution, cache creation, release, physical-device operation,
  or provider access.

## Failure behavior

A cache miss follows the ordinary bootstrap download path. An invalid restored
archive follows the same path because the existing bootstrap rejects and
replaces it. Cache transfer and service behavior use the standard action's
non-authoritative defaults; Foundation E does not turn cache status into a
Deskkin conformance result.

The job fails only when its normal prerequisites or authoritative work cannot
complete, such as runner exhaustion, ordinary download failure, bootstrap
failure, or CoreS3 conformance failure. No cache-specific failure policy is
added.

## Acceptance

Implementation must prove:

1. the cache path is exactly `.deskkin/downloads` and the key has no restore
   prefix;
2. the host job and existing mise action configuration are unchanged;
3. an empty cache runs the existing download, bootstrap, and clean CoreS3
   conformance path successfully;
4. an exact cache hit runs the same bootstrap and conformance path
   successfully;
5. the unchanged bootstrap still checks and replaces an invalid restored
   archive;
6. `mise run test` passes; and
7. fresh review finds no unintended cache path, authority, or product change.

After explicit authorization for the push, its automatically triggered workflow,
and remote cache creation, record one miss. A separately authorized later
workflow records a hit on the same accepted inputs. Compare CoreS3 job duration
and confirm both runs passed the same conformance boundary. Remove the cache if
the hit is not materially faster or if repository cache pressure makes the
optimization counterproductive.

## Ordered checkpoints

1. Review and approve the proposal. Completed on 2026-08-30.
2. Add the pinned cache step and the smallest workflow assertion needed to keep
   its path and key in scope. Implemented locally on 2026-08-30.
3. Run `mise run test` locally without creating a remote cache. Completed on
   2026-08-30.
4. After one explicit authorization covering the push, its automatically
   triggered workflow, and remote cache creation, observe the remote miss run.
5. After separate workflow authorization, observe a comparable hit run and
   retain or remove the cache from the measured result.

The approval authorized checkpoints 2 and 3 only. It does not authorize a push
or its automatic workflow and cache creation, a later workflow run, remote cache
deletion, release, device operation, provider access, or other external-state
change.

## Approval decision

Approve or revise Foundation E as the standard `.deskkin/downloads` cache above.
The recommended decision is one pinned cache action, one exact key, and no
cache-specific validation or lifecycle code.
