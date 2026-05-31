//! A generic runner for `wasi:test` suites.
//!
//! This binary is compiled to a `wasm32-wasip2` command component. It imports
//! the `wasi:test/tests` interface, which is satisfied by composing this
//! runner with a test-suite component via `wac plug`. Running the composed
//! component (e.g. with `wasmtime run`) executes every test and reports the
//! results, exiting non-zero if any test failed.

wit_bindgen::generate!({
    world: "test-runner",
    path: "wit",
    generate_all,
});

use wasi::test::tests::run_all;

fn main() {
    let outcomes = run_all();
    let mut failed = 0usize;

    for outcome in &outcomes {
        match &outcome.outcome {
            Ok(()) => println!("test {} ... ok", outcome.name),
            Err(message) => {
                failed += 1;
                println!("test {} ... FAILED: {message}", outcome.name);
            }
        }
    }

    let total = outcomes.len();
    let passed = total - failed;
    println!();
    println!(
        "test result: {}. {passed} passed; {failed} failed",
        if failed == 0 { "ok" } else { "FAILED" },
    );

    if failed > 0 {
        std::process::exit(1);
    }
}
