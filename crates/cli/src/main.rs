use std::env as std_env;

use lisprint_core::builtins;
use lisprint_core::env::Env;
use lisprint_core::eval;
use lisprint_core::parser;
use lisprint_core::prelude;

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
        Some("test") => {
            let files: Vec<&str> = args[2..].iter().map(|s| s.as_str()).collect();
            run_tests(&files);
        }
        Some(cmd) => {
            eprintln!("Unknown command: {}", cmd);
            eprintln!("Usage: lisprint [repl|run <file>|test <files...>]");
            std::process::exit(1);
        }
    }
}

fn run_repl() {
    println!("lisprint v0.1.0");
    println!("Type (quit) to exit.\n");

    let mut env = Env::new();
    builtins::register(&mut env);
    prelude::load(&mut env).expect("failed to load prelude");

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

fn run_tests(files: &[&str]) {
    let test_files: Vec<String> = if files.is_empty() {
        // デフォルト: カレントディレクトリ以下の *_test.lisp, *-test.lisp を探す
        find_test_files(".")
    } else {
        files.iter().map(|s| s.to_string()).collect()
    };

    if test_files.is_empty() {
        println!("No test files found.");
        return;
    }

    let mut total_passed = 0;
    let mut total_failed = 0;

    for path in &test_files {
        println!("\n{}", path);

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  Error reading {}: {}", path, e);
                total_failed += 1;
                continue;
            }
        };

        let mut env = Env::new();
        builtins::register(&mut env);
        prelude::load(&mut env).expect("failed to load prelude");

        // ファイルを評価してdeftestを登録
        match parser::parse(&source) {
            Ok(exprs) => {
                for expr in &exprs {
                    if let Err(e) = eval::eval(expr, &mut env) {
                        eprintln!("  Error: {}", e);
                        total_failed += 1;
                        continue;
                    }
                }
            }
            Err(e) => {
                eprintln!("  Parse error: {}", e);
                total_failed += 1;
                continue;
            }
        }

        // テスト実行
        match eval::run_tests(&mut env) {
            Ok((passed, failed)) => {
                total_passed += passed;
                total_failed += failed;
            }
            Err(e) => {
                eprintln!("  Test runner error: {}", e);
                total_failed += 1;
            }
        }
    }

    println!("\n{} passed, {} failed", total_passed, total_failed);
    if total_failed > 0 {
        std::process::exit(1);
    }
}

fn find_test_files(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_test_files(path.to_str().unwrap_or("")));
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with("_test.lisp") || name.ends_with("-test.lisp") || name.ends_with(".test.lisp") {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
    files
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
    prelude::load(&mut env).expect("failed to load prelude");

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
