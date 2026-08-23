#!/usr/bin/env python3
"""Dispatch recovery to the test firmware identified by its expected digest."""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

import gate1c_runner
import gate1d_runner
import gate1e_runner


def runner_for_digest(root: Path, expected_firmware: str) -> Path | None:
    choices = {
        gate1c_runner.firmware_digest(root): root / "scripts/gate1c_runner.py",
        gate1d_runner.firmware_digest(root): root / "scripts/gate1d_runner.py",
        gate1e_runner.firmware_digest(root): root / "scripts/gate1e_runner.py",
    }
    return choices.get(expected_firmware)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected-firmware", required=True)
    parser.add_argument("--device")
    parser.add_argument("--recording", choices=("on", "off"), default="on")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    runner = runner_for_digest(root, args.expected_firmware)
    if runner is None:
        print("Device recovery could not start: expected_firmware_not_recognized", file=sys.stderr)
        return 2
    command = [
        str(root / ".deskkin/venv/bin/python"),
        str(runner),
        "recover",
        "--expected-firmware",
        args.expected_firmware,
        "--recording",
        args.recording,
    ]
    if args.device is not None:
        command.extend(("--device", args.device))
    os.execv(command[0], command)


if __name__ == "__main__":
    raise SystemExit(main())
