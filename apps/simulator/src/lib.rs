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

#[cfg(test)]
pub(crate) struct TempCleanup(std::path::PathBuf);

#[cfg(test)]
impl TempCleanup {
    pub(crate) fn new(path: &std::path::Path) -> Self {
        remove_test_path(path).unwrap();
        Self(path.to_path_buf())
    }
}

#[cfg(test)]
impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Err(error) = remove_test_path(&self.0)
            && !std::thread::panicking()
        {
            panic!("failed to clean temporary test path: {error}");
        }
    }
}

#[cfg(test)]
fn remove_test_path(path: &std::path::Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}
