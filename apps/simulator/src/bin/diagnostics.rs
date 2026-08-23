use deskkin_simulator::{DiagnosticsCommand, run_diagnostics_command};

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let command = match arguments.as_slice() {
        [operation] if operation == "list" => DiagnosticsCommand::List,
        [operation, id] if operation == "retain" => DiagnosticsCommand::Retain(id.clone()),
        [operation, id] if operation == "unretain" => DiagnosticsCommand::Unretain(id.clone()),
        [operation, id] if operation == "delete" => DiagnosticsCommand::Delete(id.clone()),
        _ => std::process::exit(2),
    };
    match run_diagnostics_command(command) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
