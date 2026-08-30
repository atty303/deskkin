# Foundation B proposal: multi-feature application composition

Status: Proposed

Date: 2026-08-30

This proposal is the approval boundary for Foundation B. It defines the next
portable application checkpoint after the completed repeatable physical
profile. Reviewing this document does not authorize an ADR, implementation,
dependency change, protocol change, private-LAN launch, device access, provider
access, push, release, or publication.

## Goal and observable result

Replace the availability-specific top-level application with one deterministic,
compile-time composition owner that can run more than one typed feature without
giving features direct access to each other, Slint, a runtime, a transport, or a
provider.

The checkpoint is observable when the existing availability feature and one
bounded synthetic notice feature run through the same portable application,
effect router, surface arbiter, and presenter shell on deterministic simulator
scenarios. A notice must temporarily replace the availability surface and, when
cleared or invalidated, reveal the latest availability surface without
restarting either feature. Existing availability behavior and protocol bytes
must remain unchanged.

Foundation B is `durable`. The synthetic notice is a bounded conformance feature
used to prove composition, not a provider connector or a complete notification
product.

## Observed problem

`application-core` currently owns one availability state machine, one effect-ID
counter, one effect enum, and one status view. That is correct for the first
vertical slice, but adding a second feature directly would make top-level input,
effect, and view enums grow without an owner for routing, lifecycle, session
invalidation, or display precedence.

The shared Slint surface also has no application-level rule for deciding which
feature is visible. Letting each feature switch Slint state directly would move
application policy into platform presenters and produce different behavior on
the simulator and CoreS3.

## Scope

Foundation B includes:

- a compile-time registry containing availability and synthetic notice;
- explicit feature lifecycle and typed input routing;
- feature-namespaced application effect identities;
- one shared session-invalidation event for session-derived feature state;
- deterministic surface requests and arbitration;
- one typed presenter model and shared Slint presenter shell;
- one feature-neutral `application-core`, one current-feature implementation
  crate, and one concrete application composition crate;
- deterministic multi-feature scenarios and existing-platform builds; and
- observation of composition decisions without recording notice content.

It does not include:

- a provider connector, host capability registry, credential, HTTP client, or
  external payload;
- new Deskkin protocol messages, feature negotiation, permissions, or wire
  bytes;
- a production notification, conversation, character, action, or Unraid
  feature;
- dynamic plugins, runtime feature loading, scripting, or an extension ABI;
- daemonization, autostart, discovery, multi-device management, or OTA;
- pairing, provisioning, flashing, NVS mutation, power cycling, or a live host
  launch; or
- a new external package, dependency version, tool, or licensing boundary.

Host capability and connector composition remains Foundation C. Protocol
feature routing is deferred until a real second wire feature establishes its
message and permission requirements.

## Portable application composition

Keep every portable application crate pure `no_std`, allocation-free,
runtime-neutral, and free of unsafe code. Retain closed Rust enums and concrete
feature modules; do not introduce a dynamic plugin ABI or a trait hierarchy for
hypothetical features.

The portable application is split into exactly three workspace crates for this
checkpoint:

```text
application-core
    shared feature-neutral contracts
        ↑
application-features
    availability + synthetic notice
        ↑
deskkin-application
    concrete registry + routing + arbitration + presenter model
```

`application-core` owns only feature-neutral primitives used across crate
boundaries: lifecycle vocabulary, local effect identity, bounded transition
contracts, and semantic surface classes. It owns no concrete feature state,
feature name, application registry, protocol adapter, presenter model, or
Slint type.

`application-features` depends only on `application-core` and contains the two
current concrete feature modules, `availability` and `synthetic_notice`. Each
module owns its typed state, input, effect request, completion, and semantic
surface. The two small features intentionally share this crate; Foundation B
does not create one crate per feature.

`deskkin-application` depends on `application-core` and
`application-features`. It is the concrete composition root and owns the closed
registry, application-level input and view, namespaced effect wrapper, routing,
surface arbitration, and presenter model. Neither feature module depends on
`deskkin-application` or on the other feature.

Future features remain in `application-features` by default. Split a feature
into its own crate only after it has a materially independent domain contract,
effect family, dependency boundary, reuse boundary, or evolution lifecycle.
Within the portable application graph, such a crate depends only on
`application-core`; it does not depend on `application-features`, another
feature implementation crate, or `deskkin-application`.
`deskkin-application` may then register it alongside `application-features`.

The top-level portable owner is `deskkin_application::Application`. Its
compile-time registry is closed for this checkpoint:

```text
Application
├── AvailabilityFeature
├── SyntheticNoticeFeature
├── EffectRouter
└── SurfaceArbiter
```

`FeatureId` is a closed enum owned by `deskkin-application`, with
`Availability` and `SyntheticNotice`. It is not part of the feature-neutral
crate contract. Each feature owns only its typed state, inputs, effects, and
feature-local semantic surface. The application routes an input to exactly one
feature or broadcasts an explicitly defined lifecycle event. It wraps each
active feature-local surface in a composition-owned request and then recomputes
the presented surface from all such requests.

Feature transitions remain transactional: validate the complete input and
effect identity against a copied candidate state, then publish state, effects,
and surface together. A rejected input changes none of them. There is no shared
mutable global state and no feature can call another feature directly.

The existing availability state machine keeps these semantics:

- start requests an immediate availability read;
- successful reads map to `Available` or `Unavailable`;
- read failure maps to `Unknown` and retains the five-second refresh policy;
- timer-arm failure stops the feature and displays `Unknown`;
- source/session invalidation immediately displays `Unknown`; and
- stale or mismatched completions are rejected without mutation.

Moving that state machine into
`application_features::availability::AvailabilityFeature` is an internal
ownership change, not a behavior-compatibility layer. Remove the old
single-feature top-level shape once all callers use
`deskkin_application::Application`; do not retain aliases or a parallel legacy
core.

## Feature lifecycle and session invalidation

The top-level lifecycle is closed:

- `Start` starts every registered feature in registry order;
- `Stop` stops every feature, withdraws every surface, and rejects later
  completions from the stopped lifecycle;
- `SessionInvalidated` invalidates every feature state marked session-derived
  before another surface is presented.

For this checkpoint, availability and synthetic notice are both
session-derived. Invalidation atomically maps availability to `Unknown`, clears
the synthetic notice, invalidates the notice expiry identity, and presents the
resulting availability surface in the same application transition. It does not
cancel availability's local refresh lifecycle: an active read becomes the
existing unavailable-read transition and arms the next refresh, while
`ArmingRefresh` and `Waiting` retain their current timer identity and schedule.
A stale notice expiry is rejected and cannot restore or clear another surface.

The existing protocol client remains the authority for detecting session loss,
but it no longer mutates an application feature directly. It returns its own
closed semantic availability and invalidation events; each platform adapter
maps those events to `deskkin_application::ApplicationInput`. The protocol
client depends on neither `application-features` nor `deskkin-application`.
Foundation B does not add a session generation to the protocol or change
reconnect behavior.

## Namespaced effects and deterministic routing

An application effect identity is the pair `(FeatureId, LocalEffectId)`.
`LocalEffectId` remains a monotonically increasing, nonzero `u64` owned by one
feature. Completion routing first selects the exact feature and then validates
the local identity. Consequently equal local counters in different features
cannot collide, and a completion cannot be delivered to a different feature.

Effect requests remain closed typed enums. Availability keeps
`ReadAvailability` and `ArmRefreshTimer`. Synthetic notice uses only a
deterministic arm/expiry effect driven by the existing virtual-time runtime; it
performs no I/O and carries no arbitrary text or provider data.

One application transition returns a fixed-capacity, allocation-free effect
batch bounded by the number of registered features. Registry start and
invalidation use registration order; independent effect completion order does
not change surface arbitration. Capacity exhaustion and local identity
exhaustion are closed transition errors and never silently drop an effect.

Desktop, simulator, and CoreS3 adapters dispatch effects by `FeatureId`. Each
adapter must use bounded queues and return the existing typed completion or a
typed unavailable result. Foundation B does not generalize the host runtime or
create a connector abstraction.

## Synthetic notice conformance feature

The synthetic notice proves a second independent feature without introducing a
real integration. It accepts only bounded typed commands:

- show one fixed notice kind;
- arm its deterministic lifetime; and
- clear it explicitly or when its lifetime expires.

The notice kind is a closed enum with repository-owned display text. No caller
supplies arbitrary text, markup, image, URL, identifier, or payload. At most one
notice is active. Showing a new notice replaces the previous notice state in one
transition; a stale expiry cannot clear a newer notice.

The feature is compiled from `application-features` into every
`deskkin-application` target so portable composition is not simulator-only.
Product runtimes do not expose a user or network command that activates it. The
deterministic simulator driver is its only activation source in Foundation B.

## Surface request and arbitration

Features return feature-local semantic surfaces and their `SurfaceClass`, not
Slint properties or composition-owned types. A feature may expose at most one
active local surface. `deskkin-application` wraps it with the registered
`FeatureId` and the application-level `FeatureSurface` variant to form:

```text
SurfaceRequest {
    owner: FeatureId,
    class: SurfaceClass,
    surface: FeatureSurface,
}
```

The closed classes established and exercised here are:

- `Ambient`, used by availability; and
- `Information`, used by synthetic notice.

`Information` always wins over `Ambient`. Equal-class requests are resolved by
the fixed compile-time registry order, not arrival timing. The arbiter is a pure
function of current requests and therefore has no clock, queue, mutable
priority, or hidden previous-screen state. Withdrawal immediately reveals the
highest remaining request. No request produces a blank surface; if no feature
requests a surface, the shell presents a closed `Empty` model.

Future attention, conversation, confirmation, and critical classes are not
defined until a real feature needs their precedence, lifetime, and dismissal
contracts.

## Presenter shell and shared UI

`deskkin_application::ApplicationView` is the only portable presenter input. It
is a closed enum for `Empty`, availability status, and the fixed synthetic
notice. Platform code maps it to one shared Slint shell; it cannot inspect
feature state or rerun arbitration.

The shell remains the single UI owner. It contains the existing shared status
surface and one notice surface, displays exactly the selected model, and emits
only typed callbacks. CoreS3 retains its board shell, pairing controls, display
adapter, dirty-range union, and one Slint event/render owner. Foundation B does
not move pairing UI into application composition or change the 320 x 240
physical layout contract beyond the selected application surface.

## Program observation contract

Application composition is deterministic in-process state transition and does
not require a Diagnostic Run for each transition. The simulator's existing
scenario run remains the out-of-band observation surface for the multi-stage
scenario. Add closed semantic records for:

- routed feature identity;
- lifecycle input kind;
- effect request and completion kind;
- surface owner and class before and after arbitration;
- transition outcome; and
- completeness and recording health.

Forbid notice display text, arbitrary payloads, protocol bytes, addresses,
paths, identity material, authentication strings, credentials, usernames,
hostnames, process identities, and arbitrary error text. Recording disabled,
degraded, partial, or dropped remains separate from application results and
cannot change transition, effect, or surface selection.

Production adapters do not add a new public log or diagnostic transport for
this checkpoint. Existing protocol, host, and device diagnostic contracts
retain their fields and meanings.

## Reproducible acceptance

Foundation B implementation passes only when all of the following hold:

1. Pure `deskkin-application` tests prove exact feature routing, transactional
   rejection, namespaced effect completion, bounded batch behavior, identity
   exhaustion, registry-order startup, and stopped-lifecycle rejection.
2. `application-core` contract tests and `deskkin-application` surface-arbiter
   tests prove `Information` over `Ambient`, deterministic equal-class
   resolution, immediate underlying-surface restoration, `Empty`, and order
   independence for equivalent current requests.
3. `application-features` synthetic-notice tests prove bounded activation,
   replacement, explicit clear, expiry, stale-expiry rejection, and session
   invalidation.
4. `application-features` availability tests preserve every current state,
   view, effect, refresh, failure, invalidation, and stale-completion result
   after its ownership moves below `deskkin-application`.
5. A deterministic scenario proves availability visible, notice preemption,
   availability changing while covered, notice withdrawal revealing the latest
   value, session invalidation clearing both features, and reconnect restoring
   availability without restoring the invalidated notice.
6. Recording on/off produces a byte-identical complete replay for the same
   scenario, including semantic records, view sequence, virtual timestamps, and
   every RGB565 frame; replay remains deterministic.
7. Native desktop simulator and headless scenario use the shared presenter
   shell and visibly/semantically follow the same selected surfaces.
8. CoreS3 and both portable target checks compile the same application
   composition and shared UI without live hardware access or device mutation.
9. Protocol major 1 bytes, availability feature/permission negotiation,
   identity stores, pairing, reconnect, host profile lifecycle, and retained
   Phase 3P device tooling remain unchanged in behavior.
10. Static dependency inspection proves that `application-core` is
    feature-neutral, `application-features` depends only on it, its two feature
    modules do not call each other, `deskkin-application` is the sole composition
    root, and the protocol client depends on neither feature nor composition
    crate.
11. `mise run fix`, `mise run test`, and fresh durable review succeed.

The reproducible checkpoint uses deterministic fakes, virtual time, loopback,
and temporary state only. It does not read the retained physical profile or
identity, bind the private LAN, contact the CoreS3, or access a provider.

## Dependencies, licensing, and distribution

Foundation B adds the internal workspace crates `application-features` and
`deskkin-application`, plus their local path dependency edges. It adds no
external package, dependency version, or tool. Regenerate and commit the root
and device lockfiles when Cargo changes their workspace-package records; do not
hand-edit them.

All three portable crates remain MIT, `no_std`, allocation-free, and free of
Slint, runtime, transport, and provider dependencies. Foundation B reuses the
current Rust workspace, Slint toolchain, deterministic simulator, local run
recorder, and existing application/runtime adapters. Existing GPL-bearing
simulator/device binary boundaries remain unchanged. No binary, package,
release, or published artifact is produced.

## Ordered checkpoints

1. Review and approve or revise this proposal. Approval authorizes only the
   architecture, implementation, isolated tests, shared UI changes,
   reproducible builds, and documentation defined above.
2. After approval, record the long-lived application composition and surface
   ownership decision in a new ADR without rewriting ADR-0003 or ADR-0004.
3. Refactor `application-core`, add `application-features` and
   `deskkin-application`, then implement the synthetic conformance feature,
   presenter shell, adapters, observation records, and deterministic scenarios
   as one reviewed local checkpoint.
4. Run `mise run fix`, `mise run test`, portable dependency inspection, and a
   fresh durable review before committing the implementation.

Physical hardware, private-LAN launch, device mutation, provider access,
Foundation C, push, release, and publication remain outside these checkpoints
unless separately requested.

## Approval choices proposed

- Composition is a closed compile-time registry, not a dynamic plugin ABI.
- `application-core` owns feature-neutral contracts,
  `application-features` contains both current concrete feature modules, and
  `deskkin-application` is the sole concrete composition root.
- Small features remain modules in `application-features`; only a materially
  independent future feature is promoted to its own crate.
- Concrete closed feature modules and enums are used instead of a speculative
  feature trait hierarchy or one crate per feature.
- Feature-local effect identities are paired with `FeatureId`; protocol request
  identities and wire bytes do not change.
- Session invalidation is one application lifecycle input and atomically clears
  all session-derived feature output.
- Surface arbitration is pure, uses only `Ambient` and `Information`, and has a
  fixed registry-order tie break.
- Synthetic notice is fixed-content conformance behavior activated only by the
  simulator driver; it is not a production notification ingress.
- The presenter receives one selected semantic surface and remains the sole
  Slint owner.
- Foundation C owns host capability and connector registration after this
  checkpoint.
