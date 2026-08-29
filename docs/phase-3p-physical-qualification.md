# Phase 3P physical qualification evidence

Date: 2026-08-29

Phase 3P was qualified with the dedicated M5Stack CoreS3 and the Linux desktop
host on the approved private LAN. The run used disposable host state and the
device's explicitly provisioned persistent state. It did not access a provider,
change firewall or network configuration, publish an artifact, release, or
push.

## Observed results

- The host and CoreS3 derived the same six-digit authentication string from the
  Noise XX handshake. A human compared the two displays before confirming on
  both sides. The authentication string was not copied into this evidence.
- Exact identity readback reported both sides as paired. A later exact unpair
  returned both sides to unpaired before a fresh pairing window was opened.
- Cancelling on the CoreS3 returned to `ReadyToPair` and removed both pairing
  controls without resetting the display. Confirming a later pairing also
  removed the controls without a reset and reached `Available`.
- Stopping the host invalidated the current value immediately and displayed
  `Unknown`. Restarting the same pinned host recovered to `Available` without
  another confirmation.
- Host results mapped `unavailable` to `Unavailable` and `read_failed` to
  `Unknown` on the physical display.
- With host and device recording disabled, the semantic view recovered to the
  same `Available` state. Byte-identical RGB565 recording equivalence remains
  covered by the reproducible fake-boundary tests; a second physical `run`
  command was correctly rejected while the boot-started run was already active,
  so this qualification does not claim a second physical digest comparison.
- After a human power-cycled the paired CoreS3, persistent Wi-Fi configuration
  and Noise identity restored a pinned session without confirmation. Read-only
  USB status reported the paired application shell, `Available`, completed view
  application, and no boot or operation error.

## Repairs made during qualification

The first physical attempts exposed two implementation defects. Both were
reproduced before repair and verified on the same device afterward.

1. Zephyr had installed a preferred global DHCPv4 address while its internal
   DHCP state was still transient. The device incorrectly timed out before TCP
   connection. DHCP readiness now uses the usable preferred global IPv4 address
   as its success condition while retaining DHCP-only configuration.
2. Slint emitted more than one dirty range for a scanline. Keeping only the last
   callback omitted earlier erased regions and left stale Confirm and Cancel
   controls. The display adapter now unions all dirty ranges for each scanline
   before transferring the retained framebuffer.

The full repository test entrypoint, clean CoreS3 build, targeted formatting,
and fresh independent review passed after these repairs. The review found no
blocking contract violation. It identified non-blocking opportunities for a
host-executable dirty-range union test and fake DHCP readiness sequence tests.

## Retained state and security boundary

The selected residual state is intentional: the demo firmware, one Wi-Fi
profile, the local Noise identity, and the paired peer identity remain in the
CoreS3 flash. The Wi-Fi credential and Noise private identity are plaintext in
NVS because this checkpoint does not enable flash encryption, secure boot, or
forensic erasure. The matching disposable host identity is also retained
locally. Cleanup was not performed; it requires a later explicit
`phase3:device:recover -- --erase-storage` authorization.

