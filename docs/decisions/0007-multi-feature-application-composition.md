# ADR-0007: Compose portable features through one application owner

- Status: Accepted
- Date: 2026-08-30

## Context

The first portable slice placed the availability state machine directly in
`application-core`. Adding another feature there would mix feature policy with
application-level routing, effect identity, display precedence, and presenter
ownership. Letting platform adapters perform that composition would also allow
the simulator, desktop, and CoreS3 to select different visible surfaces.

The approved scope and acceptance contract are recorded in
[`foundation-b-multi-feature-application-composition-proposal.md`](../foundation-b-multi-feature-application-composition-proposal.md).

## Decision

Split the portable application into three allocation-free `no_std` crates.
`application-core` owns only feature-neutral lifecycle, local effect identity,
and surface-class vocabulary. `application-features` depends only on that core
and contains the current availability and bounded synthetic-notice modules.
`deskkin-application` depends on both and is the sole concrete composition
owner for the compile-time registry, input and effect routing, namespaced
effect identities, surface arbitration, and presenter model.

Features return feature-local semantic surfaces and never depend on another
feature or the composition crate. The application wraps those surfaces with a
closed feature identity. `Information` surfaces take precedence over
`Ambient`; equal classes use fixed registry order. Platform presenters consume
only the selected application view and never inspect feature state or repeat
arbitration.

Broadcast lifecycle events in registry order. Namespace every effect identity
as `(FeatureId, LocalEffectId)`, route completions by the feature component
first, and validate the local identity inside that feature. Apply transitions
transactionally so rejected inputs and capacity or identity exhaustion publish
no state, effect, or surface change.

Keep the protocol client independent of all three portable application crates.
It emits closed semantic availability-completion and session-invalidation
events; each platform adapter maps those events into application input.

Keep application transition observation out of the portable libraries. The
simulator application owns the existing bounded local scenario recorder and
records only closed routing, lifecycle, effect, surface, outcome, completeness,
and recording-health values. Recording failure or opt-out cannot affect
application results, view selection, virtual timestamps, or rendered frames.

## Consequences

- Availability and synthetic notice share one feature implementation crate;
  small future features remain modules there by default.
- A materially independent future feature may become a crate, but within the
  portable application graph it depends only on `application-core`.
- Desktop, simulator, and CoreS3 compile the same registry and arbitration
  policy while retaining platform-specific effect execution and shell policy.
- Protocol major 1, pairing identity, reconnect policy, device NVS, and
  provider boundaries remain unchanged.
- Dynamic feature loading, a plugin ABI, provider connectors, and protocol
  feature routing remain future decisions.

## Alternatives

### Keep every feature in `application-core`

This avoids crates initially but makes the feature-neutral contracts and
concrete product policy share one change and dependency boundary.

### Create one crate per current feature

Availability and the synthetic conformance feature do not yet have independent
dependency, reuse, or evolution boundaries, so separate crates would add
manifest and release maintenance without a corresponding isolation benefit.

### Compose features in each platform adapter

That would duplicate routing and display policy and make deterministic parity
between desktop, simulator, and CoreS3 a convention instead of a type-checked
application contract.
