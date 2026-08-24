use std::path::PathBuf;

use deskkin_simulator::{DiagnosticsCommand, run_diagnostics_command, run_diagnostics_command_at};

fn main() {
    let mut arguments: Vec<_> = std::env::args().skip(1).collect();
    let root = if arguments
        .first()
        .is_some_and(|argument| argument == "--root")
    {
        if arguments.len() < 3 {
            std::process::exit(2);
        }
        arguments.remove(0);
        Some(PathBuf::from(arguments.remove(0)))
    } else {
        None
    };
    let command = match arguments.as_slice() {
        [operation] if operation == "list" => DiagnosticsCommand::List,
        [operation, id] if operation == "retain" => DiagnosticsCommand::Retain(id.clone()),
        [operation, id] if operation == "unretain" => DiagnosticsCommand::Unretain(id.clone()),
        [operation, id] if operation == "delete" => DiagnosticsCommand::Delete(id.clone()),
        _ => std::process::exit(2),
    };
    let result = match root {
        Some(root) => run_diagnostics_command_at(root, command),
        None => run_diagnostics_command(command),
    };
    match result {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
