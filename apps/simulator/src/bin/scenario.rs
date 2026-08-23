use deskkin_simulator::{RecordingMode, ScenarioName, run_scenario_command};

fn main() {
    let mut scenario = None;
    let mut recording = RecordingMode::On;
    for argument in std::env::args().skip(1) {
        if argument == "--recording-off" {
            recording = RecordingMode::Off;
        } else if scenario.is_none() {
            scenario = Some(argument);
        } else {
            std::process::exit(2);
        }
    }
    let Some(name) = scenario else {
        std::process::exit(2)
    };
    let Ok(name) = ScenarioName::parse(&name) else {
        std::process::exit(2)
    };
    match run_scenario_command(name, recording) {
        Ok(summary) => println!("{summary}"),
        Err(_) => std::process::exit(1),
    }
}
