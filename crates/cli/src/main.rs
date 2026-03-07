use std::env as std_env;

use lisprint_core::builtins;
use lisprint_core::env::Env;
use lisprint_core::eval;
use lisprint_core::parser;

fn main() {
    let args: Vec<String> = std_env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("repl") | None => run_repl(),
        Some("run") => {
            if let Some(path) = args.get(2) {
                run_file(path);
            } else {
                eprintln!("Usage: lisprint run <file.lisp>");
                std::process::exit(1);
            }
        }
        Some(cmd) => {
            eprintln!("Unknown command: {}", cmd);
            eprintln!("Usage: lisprint [repl|run <file>]");
            std::process::exit(1);
        }
    }
}

fn run_repl() {
    println!("lisprint v0.1.0");
    println!("Type (quit) to exit.\n");

    let mut env = Env::new();
    builtins::register(&mut env);

    let mut rl = rustyline::DefaultEditor::new().expect("failed to initialize editor");

    loop {
        let readline = rl.readline("lisprint> ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);

                if line == "(quit)" || line == "(exit)" {
                    break;
                }

                match parser::parse(line) {
                    Ok(exprs) => {
                        for expr in &exprs {
                            match eval::eval(expr, &mut env) {
                                Ok(val) => {
                                    if !val.is_nil() {
                                        println!("{}", val);
                                    }
                                }
                                Err(e) => eprintln!("Error: {}", e),
                            }
                        }
                    }
                    Err(e) => eprintln!("Parse error: {}", e),
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("^C");
                break;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }
}

fn run_file(path: &str) {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let mut env = Env::new();
    builtins::register(&mut env);

    match parser::parse(&source) {
        Ok(exprs) => {
            for expr in &exprs {
                if let Err(e) = eval::eval(expr, &mut env) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}
