use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempCleanup(PathBuf);

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
            && !std::thread::panicking()
        {
            panic!("failed to clean temporary test path: {error}");
        }
    }
}

fn temp_root(label: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("deskkin-{label}-{now:x}-{count:x}"))
}

fn run(name: &str, root: &Path, recording_off: bool) -> (String, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskkin-scenario"));
    command.arg(name).env("DESKKIN_PHASE2_DIR", root);
    if recording_off {
        command.arg("--recording-off");
    }
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.starts_with("result=pass run_id=scenario-"));
    let path = stdout
        .trim_end()
        .split_once(" result_path=")
        .map(|(_, path)| path)
        .unwrap();
    let result = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    (stdout, result)
}

#[test]
fn recording_on_and_off_preserve_semantics_and_frames() {
    for name in [
        "periodic-success",
        "periodic-read-failure",
        "protocol-disconnect-recovery",
        "multi-feature-composition",
    ] {
        let on_root = temp_root("recording-on");
        let off_root = temp_root("recording-off");
        let _on_cleanup = TempCleanup(on_root.clone());
        let _off_cleanup = TempCleanup(off_root.clone());
        let (_, on) = run(name, &on_root, false);
        let (_, off) = run(name, &off_root, true);
        assert_eq!(on["replay"], off["replay"]);
        assert_eq!(on["replay_equal"], Value::Bool(true));
        assert_eq!(off["replay_equal"], Value::Bool(true));
        assert_eq!(on["protocol_major"], Value::from(1));
        assert_eq!(on["selected_features"], off["selected_features"]);
        assert_eq!(on["granted_permissions"], off["granted_permissions"]);
        let expected_refresh_runs = if name == "multi-feature-composition" {
            6
        } else {
            4
        };
        assert!(on["child_refresh_runs"].as_array().unwrap().len() == expected_refresh_runs);
        assert!(off["child_refresh_runs"].as_array().unwrap().len() == expected_refresh_runs);
        assert!(
            on["child_refresh_runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|run| run["stored"] == Value::Bool(true))
        );
        assert!(
            off["child_refresh_runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|run| run["stored"] == Value::Bool(false))
        );
        assert!(on_root.join("diagnostics").exists());
        assert!(!off_root.join("diagnostics").exists());
    }
}

#[test]
fn invalid_result_paths_cannot_break_single_line_stdout() {
    for root in [
        OsString::from("/tmp/deskkin-line\nbreak"),
        OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', b'x', 0xff]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_deskkin-scenario"))
            .args(["periodic-success", "--recording-off"])
            .env("DESKKIN_PHASE2_DIR", root)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}
