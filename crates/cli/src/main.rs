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
        Some("build") => {
            if let Some(path) = args.get(2) {
                let output = args.get(3).map(|s| s.as_str());
                build_binary(path, output);
            } else {
                eprintln!("Usage: lisprint build <file.lisp> [output]");
                std::process::exit(1);
            }
        }
        Some(cmd) => {
            eprintln!("Unknown command: {}", cmd);
            eprintln!("Usage: lisprint [repl|run <file>|build <file> [output]|test <files...>]");
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

fn build_binary(path: &str, output: Option<&str>) {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let exprs = match parser::parse(&source) {
        Ok(exprs) => exprs,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    };

    let compiler = match lisprint_compiler::Compiler::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Compiler init error: {}", e);
            std::process::exit(1);
        }
    };

    let obj_bytes = match compiler.compile_exprs(&exprs) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Compile error: {}", e);
            std::process::exit(1);
        }
    };

    // Determine output name
    let output_name = output.unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("a.out")
    });

    // Write object file
    let obj_path = format!("{}.o", output_name);
    if let Err(e) = std::fs::write(&obj_path, &obj_bytes) {
        eprintln!("Error writing object file: {}", e);
        std::process::exit(1);
    }

    // Write runtime entry point (C wrapper that calls _lsp_main)
    let entry_c_path = format!("{}_entry.c", output_name);
    let entry_c = r#"
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <math.h>

// Forward declarations for runtime functions
void lsp_println(int64_t tag, int64_t payload);
void lsp_print(int64_t tag, int64_t payload);

// Tags
#define TAG_NIL   0
#define TAG_BOOL  1
#define TAG_INT   2
#define TAG_FLOAT 3
#define TAG_STR   4

void lsp_println(int64_t tag, int64_t payload) {
    lsp_print(tag, payload);
    printf("\n");
}

void lsp_print(int64_t tag, int64_t payload) {
    switch (tag) {
        case TAG_NIL:   printf("nil"); break;
        case TAG_BOOL:  printf("%s", payload ? "true" : "false"); break;
        case TAG_INT:   printf("%lld", (long long)payload); break;
        case TAG_FLOAT: {
            double f;
            memcpy(&f, &payload, sizeof(double));
            printf("%g", f);
            break;
        }
        case TAG_STR:   printf("%s", (const char*)payload); break;
        default:        printf("<unknown:%lld>", (long long)tag); break;
    }
}

typedef struct { int64_t tag; int64_t payload; } TaggedValue;
TaggedValue lsp_str_concat(int64_t t1, int64_t p1, int64_t t2, int64_t p2) {
    const char* s1 = (const char*)p1;
    const char* s2 = (const char*)p2;
    size_t len = strlen(s1) + strlen(s2) + 1;
    char* result = malloc(len);
    strcpy(result, s1);
    strcat(result, s2);
    return (TaggedValue){TAG_STR, (int64_t)result};
}

TaggedValue lsp_to_string(int64_t tag, int64_t payload) {
    char buf[64];
    char* result;
    switch (tag) {
        case TAG_NIL:   result = strdup("nil"); break;
        case TAG_BOOL:  result = strdup(payload ? "true" : "false"); break;
        case TAG_INT:   snprintf(buf, sizeof(buf), "%lld", (long long)payload); result = strdup(buf); break;
        case TAG_FLOAT: {
            double f;
            memcpy(&f, &payload, sizeof(double));
            snprintf(buf, sizeof(buf), "%g", f);
            result = strdup(buf);
            break;
        }
        case TAG_STR:   return (TaggedValue){TAG_STR, payload};
        default:        result = strdup("<unknown>"); break;
    }
    return (TaggedValue){TAG_STR, (int64_t)result};
}

// Compiled Lisp entry point
extern void _lsp_main(int64_t* ret_tag, int64_t* ret_payload);

int main() {
    int64_t tag, payload;
    // _lsp_main returns two i64 values
    // On most ABIs this is via registers, we call it directly
    typedef struct { int64_t tag; int64_t payload; } LspResult;
    LspResult (*lsp_main_fn)(void) = (LspResult(*)(void))_lsp_main;
    LspResult result = lsp_main_fn();
    return 0;
}
"#;

    if let Err(e) = std::fs::write(&entry_c_path, entry_c) {
        eprintln!("Error writing entry file: {}", e);
        std::process::exit(1);
    }

    // Link with cc
    let status = std::process::Command::new("cc")
        .args([&entry_c_path, &obj_path, "-o", output_name, "-lm"])
        .status();

    // Cleanup temp files
    let _ = std::fs::remove_file(&obj_path);
    let _ = std::fs::remove_file(&entry_c_path);

    match status {
        Ok(s) if s.success() => {
            println!("Built: {}", output_name);
        }
        Ok(s) => {
            eprintln!("Linker failed with exit code: {}", s);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to run linker: {}", e);
            std::process::exit(1);
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
