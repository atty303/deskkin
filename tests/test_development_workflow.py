import json
import os
import re
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

HOST_COMMANDS = (
    "mise run check",
    "cargo clippy --locked --workspace --all-targets -- -D warnings",
    "cargo test --locked --workspace",
    "cargo run --locked --quiet --bin deskkin-scenario -- periodic-success --recording-off",
    "cargo run --locked --quiet --bin deskkin-scenario -- periodic-read-failure --recording-off",
    "cargo run --locked --quiet --bin deskkin-scenario -- protocol-disconnect-recovery --recording-off",
    "cargo run --locked --quiet --bin deskkin-scenario -- multi-feature-composition --recording-off",
    "cargo check --locked -p application-core --target thumbv7m-none-eabi",
    "cargo check --locked -p application-features --target thumbv7m-none-eabi",
    "cargo check --locked -p deskkin-application --target thumbv7m-none-eabi",
    "cargo check --locked -p deskkin-protocol --target thumbv7m-none-eabi",
    "cargo check --locked -p deskkin-protocol-client --target thumbv7m-none-eabi",
    "cargo check --locked -p deskkin-core-s3 --target thumbv7m-none-eabi",
    "DESKKIN_TEST_AGE=1 python -m unittest discover -s tests",
    "cargo tree --locked -p application-core --edges normal",
    "cargo tree --locked -p application-features --edges normal",
    "cargo tree --locked -p deskkin-application --edges normal",
    "cargo tree --locked -p deskkin-host-capabilities --edges normal",
    "cargo tree --locked -p deskkin-protocol --edges normal",
)
CORE_S3_COMMANDS = ("python scripts/phase3_device.py build",)
AGGREGATE_COMMANDS = ("mise run test:host", "mise run test:core-s3")


def resolved_tasks() -> dict[str, dict]:
    result = subprocess.run(
        ["mise", "tasks", "--json"],
        check=True,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
    )
    return {task["name"]: task for task in json.loads(result.stdout)}


def task_commands(task: dict) -> tuple[str, ...]:
    lines = task["run"][0].splitlines()
    return tuple(
        line
        for line in lines
        if line and not line.startswith("#!") and line != "set -euo pipefail"
    )


class DevelopmentWorkflowTests(unittest.TestCase):
    def test_resolved_tasks_have_exactly_one_lane_owner(self):
        tasks = resolved_tasks()
        self.assertEqual(task_commands(tasks["test:host"]), HOST_COMMANDS)
        self.assertEqual(task_commands(tasks["test:core-s3"]), CORE_S3_COMMANDS)
        self.assertEqual(task_commands(tasks["test"]), AGGREGATE_COMMANDS)
        self.assertEqual(tasks["test"]["depends"], [])

        owned = task_commands(tasks["test:host"]) + task_commands(tasks["test:core-s3"])
        for command in HOST_COMMANDS + CORE_S3_COMMANDS:
            self.assertEqual(owned.count(command), 1, command)

    def test_aggregate_stops_before_core_s3_after_host_failure(self):
        aggregate = resolved_tasks()["test"]["run"][0]
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            log = directory / "calls"
            fake_mise = directory / "mise"
            fake_mise.write_text(
                "#!/usr/bin/env python3\n"
                "import os\n"
                "import sys\n"
                "from pathlib import Path\n"
                "with Path(os.environ['DESKKIN_FAKE_MISE_LOG']).open('a') as output:\n"
                "    output.write(' '.join(sys.argv[1:]) + '\\n')\n"
                "raise SystemExit(17 if sys.argv[1:] == ['run', 'test:host'] else 0)\n",
                encoding="utf-8",
            )
            fake_mise.chmod(fake_mise.stat().st_mode | stat.S_IXUSR)
            environment = os.environ.copy()
            environment["DESKKIN_FAKE_MISE_LOG"] = str(log)
            environment["PATH"] = os.pathsep.join((str(directory), environment["PATH"]))
            result = subprocess.run(
                ["bash", "-c", aggregate],
                cwd=ROOT,
                env=environment,
                check=False,
            )
            calls = log.read_text(encoding="utf-8").splitlines()

        self.assertEqual(result.returncode, 17)
        self.assertEqual(calls, ["run test:host"])

    def test_host_lane_does_not_reference_core_s3_state(self):
        host = "\n".join(task_commands(resolved_tasks()["test:host"]))
        for forbidden in (
            "phase3_device.py",
            "bootstrap_core_s3",
            ".deskkin/sdk",
            ".deskkin/west",
            ".deskkin/rustup",
            ".deskkin/venv",
        ):
            self.assertNotIn(forbidden, host)

    def test_core_s3_lane_does_not_own_host_checks(self):
        core_s3 = "\n".join(task_commands(resolved_tasks()["test:core-s3"]))
        for forbidden in (
            "cargo test",
            "cargo clippy",
            "deskkin-scenario",
            "unittest",
            "cargo tree",
        ):
            self.assertNotIn(forbidden, core_s3)

    def test_ci_runs_both_lanes_independently(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        trigger = workflow.split("on:\n", 1)[1].split("\npermissions:\n", 1)[0]
        self.assertEqual(
            trigger,
            "  pull_request:\n"
            "    branches:\n"
            "      - main\n"
            "  push:\n"
            "    branches:\n"
            "      - main\n"
            "  workflow_dispatch:\n",
        )
        permission = workflow.split("permissions:\n", 1)[1].split(
            "\n\nconcurrency:\n", 1
        )[0]
        self.assertEqual(permission, "  contents: read")
        concurrency = workflow.split("concurrency:\n", 1)[1].split(
            "\n\njobs:\n", 1
        )[0]
        self.assertEqual(
            concurrency,
            "  group: ci-${{ github.workflow }}-${{ github.ref }}\n"
            "  cancel-in-progress: true",
        )

        jobs = workflow.split("jobs:\n", 1)[1]
        names = re.findall(r"^  ([a-z0-9-]+):$", jobs, flags=re.MULTILINE)
        self.assertEqual(names, ["host", "core-s3"])
        checkout = "actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803"
        setup_mise = "jdx/mise-action@3c2e0cf82a5b2e5249f0d3635a4d83d0ae861518"
        host, core_s3 = jobs.split("  core-s3:\n", 1)
        host_uses = re.findall(
            r"^\s+uses: (\S+)(?:\s+#.*)?$", host, flags=re.MULTILINE
        )
        core_s3_uses = re.findall(
            r"^\s+uses: (\S+)(?:\s+#.*)?$", core_s3, flags=re.MULTILINE
        )
        self.assertEqual(host_uses, [checkout, setup_mise])
        self.assertEqual(core_s3_uses, [checkout, setup_mise])

        self.assertIn("run: mise run test:host", host)
        self.assertNotIn("Reclaim runner disk", host)
        self.assertNotIn("phase3:device:bootstrap", host)
        self.assertNotRegex(
            host,
            r"(?m)^    (?:if|needs):",
            msg="host lane must start independently",
        )

        self.assertIn("mise run phase3:device:bootstrap", core_s3)
        self.assertIn("run: mise run test:core-s3", core_s3)
        self.assertNotRegex(
            core_s3,
            r"(?m)^    (?:if|needs):",
            msg="CoreS3 lane must start independently",
        )


if __name__ == "__main__":
    unittest.main()
