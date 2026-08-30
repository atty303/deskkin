# Repository Guidelines

## Product boundary

Deskkin is a platform for embodied desktop companions, not a CoreS3-specific
firmware and not an Unraid client. The first device is StackChan on M5Stack
CoreS3. No provider connector is the current checkpoint; Unraid is one future
candidate. Preserve boundaries that allow additional devices and integrations
without changing the portable application core.

Keep external service credentials, authorization, persistence, and connector
protocols in the desktop host. Device code receives semantic capabilities,
events, and action requests through the Deskkin protocol. It must not contain
Unraid, ChatGPT, desktop-notification, or other provider APIs.

## Architecture invariants

- Keep `application-core` as pure `no_std` Rust, independent of Zephyr,
  Embassy, Slint, desktop APIs, transports, and boards.
- Treat Embassy as an application runtime adapter. Do not put executor,
  platform driver, or global clock calls in the portable core.
- Use Slint for the shared declarative UI. Keep one UI owner and communicate
  with application code through typed commands, events, and view models.
- Use Zephyr for device discovery, drivers, hardware topology, and system
  services. Keep board quirks in devicetree, board support, or drivers rather
  than application conditionals.
- Put unsafe Rust, C FFI, and Zephyr raw bindings behind narrow platform
  adapters. Do not expose Zephyr types through portable interfaces.
- Model external effects as typed requests and completion events so desktop
  tests can use virtual time and deterministic fakes.
- Exchange semantic protocol messages, not Slint properties, Zephyr events,
  provider payloads, or hardware operations.
- Keep device features compile-time registered until a demonstrated need for a
  dynamic plugin ABI exists. Desktop connector loading is a separate decision.
- Keep credentials and broad external authority off companion devices. Model
  read capabilities, mutation capabilities, and confirmation policy
  separately.

## Current checkpoint

The repository contains the portable application and protocol crates, Linux
host and simulator, CoreS3 firmware, and their reproducible tooling. Completed
feasibility-gate harnesses have been removed; their accepted contracts and
evidence remain under `docs/`.

Treat `docs/implementation-plan.md` as the source of truth for current status
and next work. Do not begin a new product slice or add dependencies before its
approval checkpoint. Record long-lived architectural changes as a new ADR; do
not rewrite accepted ADRs to hide superseded decisions.

## Development workflow

- Use `mise run check` for explicitly requested non-mutating repository checks.
- Use `mise run fix` for safe formatter and linter fixes without staging
  changes.
- Use `mise run test:host` for host and portable feedback without CoreS3 state.
- Use `mise run test:core-s3` for clean CoreS3 conformance after the explicit
  toolchain bootstrap.
- Use `mise run test` for all reproducible tests before completing a change.
- Pass paths after `--` to limit check or fix to selected files.
- Keep tool versions in `mise.toml` and commit the generated `mise.lock`.
- Treat `hk.pkl` as the source of truth for fast checks and Git hooks.

## Agent feedback

Use fix-first feedback during development. With hk MCP, call `inspect_project`
and `plan`, then `start_safe_fix` without a preliminary check or diff. Without
MCP, call `mise run fix` directly. If autofix succeeds with no remaining
diagnostics, continue without reading the diff. Read run output and the diff
only when autofix fails, leaves diagnostics, reports a parser warning, or may
have applied a partial change; repair the issue and run fix again. Use
`start_safe_check` or `mise run check` only when non-mutating verification is
explicitly requested or fix cannot be used diagnostically. Never bypass a safe
refusal with unrestricted MCP execution or a direct `hk` command. Child test
tasks provide feedback only; run `mise run test` once as the final verification.

## Agent bootstrap

Run `mise trust --yes` and `mise install --yes` before development. The
project-scoped Codex configuration starts hk's MCP server through mise; restart
Codex after the first install if the server was unavailable during startup.

## Change policy

Keep changes focused, update documentation with public behavior, and use short
Conventional Commit subjects. Do not push, publish, release, or change external
state unless the user explicitly requests it.
