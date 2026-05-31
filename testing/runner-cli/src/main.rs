//! Generic `wasi:test` runner. Ported from Lann Martin's `wasi:test` prototype
//! (<https://github.com/lann/wasi-test>, `runner-cli`).
//!
//! Imports `wasi:test/tests` and exports `wasi:cli/run` (via `fn main`). It
//! runs every test returned by `all()`, prints `PASS`/`FAIL` per test, streams
//! and prints logs for failures, and exits non-zero if any test failed. This
//! single binary is reused (via composition) for every component under test.

use crate::wasi::test::tests::{TestContext, TestOptions, TestResult};

wit_bindgen::generate!({
    world: "runner",
    path: "../wit",
    async: ["wasi:test/tests#[method]test-case.run"],
});

fn main() -> Result<(), String> {
    let failed = wit_bindgen::block_on(run_tests());
    if failed > 0 {
        Err(format!("{failed} test(s) failed"))
    } else {
        Ok(())
    }
}

async fn run_tests() -> usize {
    let mut failed = 0;
    for test in wasi::test::tests::all() {
        eprint!("Test {}...", test.name());

        let opts = TestOptions::new();
        let logs_stream = opts.enable_logging();

        let (result, logs) = futures_lite::future::zip(
            async {
                let ctx = TestContext::new(opts);
                let result = test.run(&ctx).await;
                drop(ctx);
                result
            },
            logs_stream.collect(),
        )
        .await;
        match result {
            TestResult::Pass => eprintln!("PASS"),
            TestResult::Fail(msg) => {
                failed += 1;
                eprintln!("FAIL ({msg})");
                if !logs.is_empty() {
                    eprintln!("Test logs:");
                    for msg in logs {
                        eprintln!("{msg}");
                    }
                    eprintln!();
                }
            }
        }
    }
    failed
}
