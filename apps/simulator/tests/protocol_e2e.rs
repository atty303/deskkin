use std::fs;
use std::thread;

use application_core::{Command, Core, Input, RefreshDue, StatusView, TimerArmCompleted};
use deskkin_desktop_host::{
    IdentityStore, bind_loopback, pair_initiator, pair_responder, read_once, serve_one,
};
use deskkin_protocol::AvailabilityResult;
use deskkin_simulator::ProtocolAdapter;
use local_run_recorder::ResourceRole;
use serde_json::Value;

#[test]
fn paired_loopback_result_reaches_core_and_disconnect_invalidates_view() {
    let base = std::env::temp_dir().join(format!("deskkin-protocol-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let host = IdentityStore::new(base.join("host/identity"));
    let device = IdentityStore::new_for_role(
        base.join("device-simulator/identity"),
        ResourceRole::DeviceSimulator,
    );
    host.init().unwrap();
    device.init().unwrap();
    let pairing = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = pairing.local_addr().unwrap();
    let host_pair = host.clone();
    let join = thread::spawn(move || pair_responder(&pairing, &host_pair, |_, _| true));
    let device_sas = pair_initiator(address, &device, [1; 16], |_, _| true).unwrap();
    assert_eq!(device_sas, join.join().unwrap().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let (host_diagnostics, device_diagnostics) = loop {
        let host = diagnostic_values(&base.join("host/diagnostics"));
        let device = diagnostic_values(&base.join("device-simulator/diagnostics"));
        if host.iter().any(|run| run["transaction_id"].is_string())
            && device.iter().any(|run| run["transaction_id"].is_string())
        {
            break (host, device);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pairing diagnostics timed out"
        );
        thread::sleep(std::time::Duration::from_millis(10));
    };
    let host_transaction = host_diagnostics
        .iter()
        .find_map(|run| run["transaction_id"].as_str())
        .unwrap();
    assert!(device_diagnostics.iter().any(|run| {
        run["transaction_id"].as_str() == Some(host_transaction)
            && run["resource"]["role"] == Value::String("device_simulator".into())
    }));
    let diagnostic_text = format!("{host_diagnostics:?}{device_diagnostics:?}");
    assert!(!diagnostic_text.contains(&host.public_key().unwrap()));
    assert!(!diagnostic_text.contains(&device.public_key().unwrap()));
    assert!(!diagnostic_text.contains(&String::from_utf8_lossy(&device_sas).to_string()));
    let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = listener.local_addr().unwrap();
    let first_host = host.clone();
    let join =
        thread::spawn(move || serve_one(&listener, &first_host, AvailabilityResult::Available));
    let wire = read_once(address, &device, [2; 16], [3; 16]).unwrap();
    join.join().unwrap().unwrap();
    let mut core = Core::new();
    core.transition(Input::Command(Command::Start)).unwrap();
    let mut adapter = ProtocolAdapter::new();
    adapter.authenticated([2; 16]);
    let request = adapter.begin_read(&core, [3; 16]).unwrap();
    let timer = adapter
        .result(&mut core, [2; 16], request, [3; 16], wire)
        .unwrap()
        .unwrap();
    assert_eq!(core.view(), StatusView::Available);
    core.transition(Input::TimerArmCompleted(TimerArmCompleted {
        effect_id: timer.id,
        result: Ok(()),
    }))
    .unwrap();
    let waiting = core.state();
    adapter.disconnected(&mut core).unwrap();
    assert_eq!(core.view(), StatusView::Unknown);
    assert_eq!(core.state(), waiting);

    core.transition(Input::RefreshDue(RefreshDue {
        effect_id: timer.id,
    }))
    .unwrap()
    .effect
    .unwrap();
    let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = listener.local_addr().unwrap();
    let join = thread::spawn(move || serve_one(&listener, &host, AvailabilityResult::Available));
    let wire = read_once(address, &device, [4; 16], [5; 16]).unwrap();
    join.join().unwrap().unwrap();
    adapter.connecting();
    adapter.authenticated([4; 16]);
    let request = adapter.begin_read(&core, [5; 16]).unwrap();
    adapter
        .result(&mut core, [4; 16], request, [5; 16], wire)
        .unwrap();
    assert_eq!(core.view(), StatusView::Available);
}

fn diagnostic_values(root: &std::path::Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "json")
        })
        .filter_map(|entry| serde_json::from_slice(&fs::read(entry.path()).unwrap()).ok())
        .collect()
}
