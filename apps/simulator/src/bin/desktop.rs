use deskkin_simulator::{RecordingMode, run_desktop};

fn main() {
    let recording = if std::env::args().any(|argument| argument == "--recording-off") {
        RecordingMode::Off
    } else {
        RecordingMode::On
    };
    if let Err(error) = run_desktop(recording) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
