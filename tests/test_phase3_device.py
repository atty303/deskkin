import importlib.util
import json
import os
import stat
import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("phase3_device", ROOT / "scripts/phase3_device.py")
device = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(device)


class Phase3DeviceTests(unittest.TestCase):
    def profile(self):
        return {"schema_version": 1, "ssid": "fixture", "password": "fake-password", "host_ipv4": "192.168.10.2"}

    def test_profile_schema_is_exact_and_rfc1918(self):
        self.assertEqual(device.validate_profile(self.profile()), self.profile())
        for change in ({"extra": 1}, {"host_ipv4": "8.8.8.8"}, {"password": "short"}, {"schema_version": 2}):
            value = self.profile() | change
            with self.assertRaises(device.DeviceError):
                device.validate_profile(value)

    def test_control_frame_is_bounded_and_canonical(self):
        payload = device.wifi_payload(self.profile())
        frame = device.control_frame("wifi-provision", 7, payload)
        self.assertEqual(int.from_bytes(frame[:2], "big"), len(frame) - 2)
        self.assertEqual(frame[2:4], bytes([1, 4]))
        self.assertEqual(int.from_bytes(frame[20:28], "big"), 7)
        self.assertEqual(int.from_bytes(frame[28:30], "big"), len(payload))
        device.zeroize(payload)
        device.zeroize(frame)
        self.assertEqual(set(payload), {0})
        self.assertEqual(set(frame), {0})

    def test_control_io_handles_partial_reads_and_writes(self):
        writes = []

        def partial_write(_descriptor, value):
            length = min(2, len(value))
            writes.append(bytes(value[:length]))
            return length

        chunks = [b"a", b"bc", b"d"]
        with mock.patch.object(device.select, "select", side_effect=lambda reads, writes, errors, timeout: (reads, writes, errors)), mock.patch.object(device.os, "write", side_effect=partial_write), mock.patch.object(device.os, "read", side_effect=lambda _descriptor, _length: chunks.pop(0)):
            device.write_all(7, b"abcdef", 1)
            self.assertEqual(b"".join(writes), b"abcdef")
            self.assertEqual(device.read_exact(7, 4, 1), b"abcd")

    def test_run_success_requires_valid_result_and_rendered_correlations(self):
        record = {
            "shell_state": 4,
            "valid_availability_result": True,
            "session_context_id": "11" * 16,
            "operation_context_id": "22" * 16,
            "rgb565_digest": "12345678",
            "run_attempt": 7,
            "result_attempt": 7,
            "frame_attempt": 7,
        }
        self.assertTrue(device.run_succeeded([record], 7))
        for key, invalid in (
            ("valid_availability_result", False),
            ("session_context_id", "00" * 16),
            ("operation_context_id", "00" * 16),
            ("rgb565_digest", "00000000"),
        ):
            self.assertFalse(device.run_succeeded([record | {key: invalid}], 7))
        for key in ("run_attempt", "result_attempt", "frame_attempt"):
            self.assertFalse(device.run_succeeded([record | {key: 6}], 7))

    def test_hosted_profile_parser_is_exact_and_bounded(self):
        plaintext = bytearray(json.dumps(self.profile(), separators=(",", ":")).encode())
        try:
            self.assertEqual(device.profile_payload_from_json(ROOT, plaintext), device.wifi_payload(self.profile()))
            plaintext[:] = json.dumps(self.profile() | {"extra": 1}, separators=(",", ":")).encode()
            with self.assertRaises(device.DeviceError):
                device.profile_payload_from_json(ROOT, plaintext)
        finally:
            device.zeroize(plaintext)

    @unittest.skipUnless(os.environ.get("DESKKIN_TEST_AGE") == "1", "age round trip is run by the locked task")
    def test_age_round_trip_has_no_plaintext_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            os.chmod(root, 0o700)
            identity = root / "identity.txt"
            subprocess.run(
                ["age-keygen", "-o", str(identity)],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            profile = root / "wifi.age"
            device.create_profile(profile, identity, self.profile())
            self.assertEqual(stat.S_IMODE(profile.stat().st_mode), 0o600)
            plaintext = device.decrypt_profile(profile, identity)
            self.assertEqual(json.loads(plaintext), self.profile())
            device.zeroize(plaintext)
            self.assertEqual(sorted(path.name for path in root.iterdir()), ["identity.txt", "wifi.age"])

    def test_atomic_result_is_private_and_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = device.publish_result(root, "run", "success", "first")
            second = device.publish_result(root, "run", "error", "second")
            self.assertEqual(first, second)
            self.assertEqual(stat.S_IMODE(first.stat().st_mode), 0o600)
            self.assertEqual(json.loads(first.read_text())["run_id"], "second")

    def test_device_diagnostics_are_private_bounded_and_allowlisted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            records = [
                {
                    "operation": "device_ui",
                    "operation_id": 1,
                    "parent_operation_id": None,
                    "status": "success",
                    "error_type": None,
                    "effect_id": None,
                    "virtual_time_ms": 0,
                    "end_virtual_time_ms": 0,
                    "duration_ms": 0,
                    "render_width": 320,
                    "render_height": 240,
                    "value": "unknown",
                    "session_context_id": "00" * 16,
                    "operation_context_id": "11" * 16,
                    "rgb565_digest": "12345678",
                    "shell_state": 1,
                }
            ]
            for index in range(12):
                device.publish_diagnostic(root, f"run-{index:02}", "success", records)
            diagnostics = root / ".deskkin/phase3/device/diagnostics"
            self.assertEqual(len(list(diagnostics.glob("*.json"))), 10)
            self.assertTrue(all(stat.S_IMODE(path.stat().st_mode) == 0o600 for path in diagnostics.glob("*.json")))
            persisted = b"".join(path.read_bytes() for path in diagnostics.glob("*.json"))
            for forbidden in (b"password", b"ssid", b"socket_address", b"authentication_string", b"private_key"):
                self.assertNotIn(forbidden, persisted)

    def test_device_diagnostics_reject_symlink_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            target.mkdir()
            (root / ".deskkin").symlink_to(target, target_is_directory=True)
            with self.assertRaises(OSError):
                device.publish_diagnostic(root, "run", "success", [])


if __name__ == "__main__":
    unittest.main()
