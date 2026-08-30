use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use deskkin_desktop_host::IdentityStore;

fn host(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deskkin-desktop-host"))
        .current_dir(root)
        .args(arguments)
        .output()
        .unwrap()
}

fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "profile host did not exit after signal"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn saturated_owner_control_signal_exits_bounded_and_cleans_ownership() {
    let root = std::env::temp_dir().join(format!(
        "deskkin-profile-signal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = probe.local_addr().unwrap().to_string();
    drop(probe);
    assert!(
        host(
            &root,
            &[
                "profile",
                "set",
                "physical",
                "--role-root",
                "roles/host",
                "--bind-mode",
                "loopback",
                "--address",
                &address,
                "--availability",
                "available",
                "--recording",
                "off",
            ],
        )
        .status
        .success()
    );
    std::fs::create_dir_all(root.join(".deskkin/roles/host")).unwrap();
    std::fs::set_permissions(
        root.join(".deskkin/roles"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(
        root.join(".deskkin/roles/host"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    IdentityStore::new(root.join(".deskkin/roles/host/identity"))
        .init()
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_deskkin-desktop-host"))
        .current_dir(&root)
        .args(["profile-host", "--profile", "physical"])
        .spawn()
        .unwrap();
    let control = root.join(".deskkin/roles/host/control/owner.sock");
    for _ in 0..300 {
        if control.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(control.exists());
    let partial = (0..4)
        .map(|_| UnixStream::connect(&control).unwrap())
        .collect::<Vec<_>>();
    thread::sleep(Duration::from_millis(100));
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = wait_for_exit(&mut child);
    assert!(!status.success());
    drop(partial);
    assert!(!control.exists());
    let status = host(&root, &["profile-status", "--profile", "physical"]);
    assert!(status.status.success());
    assert_eq!(String::from_utf8(status.stdout).unwrap().trim(), "stopped");
    std::fs::remove_dir_all(root).unwrap();
}
