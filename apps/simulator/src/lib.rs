mod diagnostics;
mod presenter;
mod runtime;
mod scenario_driver;

slint::include_modules!();

pub use diagnostics::{DiagnosticsCommand, RecordingMode, run_diagnostics_command};
pub use runtime::run_desktop;
pub use scenario_driver::{ScenarioName, run_scenario_command};
