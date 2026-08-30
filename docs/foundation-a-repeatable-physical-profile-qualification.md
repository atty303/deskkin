# Foundation A repeatable physical profile qualification evidence

Date: 2026-08-30

Foundation A was qualified with the retained M5Stack CoreS3 and its retained
Phase 3P Linux host identity on the approved private LAN. The run used the
ignored local profile `core-s3`, the exact retained role root, the configured
private-LAN listener on fixed port 39042, fake `available`, and recording on.
The exact assigned address remains only in the ignored profile and is not
duplicated in this public evidence.

The qualification did not pair, provision, flash, unpair, erase, power-cycle,
change NVS, change an interface or firewall, access a provider, release, or
publish an artifact.

## Observed results

- `deskkin:host` resolved the named profile and reached owner readiness with the
  configured retained role, private-LAN bind mode, fake availability, and
  recording selection. The complete, healthy lifecycle run was
  `profile-e9e32626ec1178bbb01e6cde6e76f9c3`.
- `deskkin:status` reported `running` for the same profile and exact launch
  metadata. The corresponding successful, healthy profile-status diagnostic was
  `profile-d3daa6055edb5f94657b72cee6b39c00`; the closed state itself was
  observed from the CLI result rather than stored in that diagnostic.
- The already paired CoreS3 reconnected without opening a pairing window or
  changing either identity. Read-only device status reported the paired shell,
  `Available`, completed view application, and no operation error as device run
  `3090b5ad-e163-4b91-ba72-789a3fae7f51`.
- `deskkin:stop` used the observed owner generation and completed the joined,
  generation-bound shutdown. The successful, healthy profile-stop diagnostic
  was `profile-e2b3dffaf06af822015867c16143cbc9`. A later status query reported
  `stopped`; its successful, healthy profile-status diagnostic was
  `profile-f59f550daab876f609429c9f07f2e01f`. The closed states were observed
  from the CLI results; no owner socket or running listener was claimed.
- After host shutdown, read-only device status reported the expected disconnected
  state as device run `745477fa-b070-4588-bca7-4d3e3045ac95`: the `Connecting`
  shell while retaining the paired identity, unknown availability, and
  `availability_timeout`. This invalidated the remote-derived value rather than
  fabricating `Unavailable` or unpairing the device.
- The healthy availability child diagnostic
  `run-339ae2e23c136a247c6361dcbaa789bf` links to lifecycle run
  `profile-e9e32626ec1178bbb01e6cde6e76f9c3` and completed its availability and
  transport operations successfully.

## Retained state

The ignored `core-s3` profile remains available for later explicitly approved
physical work. The CoreS3 still retains the Phase 3P demo firmware, Wi-Fi
profile, local Noise identity, and paired peer identity already selected by the
Phase 3P qualification. The matching host identity remains below the retained
role root. This run neither created nor modified those values.

The Wi-Fi credential and Noise private identity remain plaintext in device NVS
under the accepted Phase 3P residual-state boundary. Cleanup remains a separate
explicit device-mutation action and was not performed.

## Qualification conclusion

The named profile reproduced the intended physical host configuration, selected
the retained identity correctly, admitted the already paired CoreS3, exposed
exact read-only lifecycle status, and stopped only the observed owner
generation. Foundation A's isolated implementation and live retained-profile
qualification checkpoints are complete.
