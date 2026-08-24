mod diagnostics;
mod presenter;
mod runtime;
mod scenario_driver;

slint::include_modules!();

pub use deskkin_protocol_client::{ConnectionState, ProtocolAdapter, ProtocolAdapterError};
pub use diagnostics::{
    DiagnosticsCommand, RecordingMode, run_diagnostics_command, run_diagnostics_command_at,
};
pub use runtime::{run_desktop, run_protocol_desktop, run_protocol_desktop_with_recording};
pub use scenario_driver::{ScenarioName, run_scenario_command};
