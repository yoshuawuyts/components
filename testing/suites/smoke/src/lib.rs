//! Milestone 1 smoke suite: a trivial passing test and a trivial failing test,
//! with no component under test. Exercises the full `wasi:test` async path
//! (streaming logs, `suite!` macro, compose with the runner) in isolation.

use wasi_test::TestContext;

fn smoke_pass(ctx: &TestContext) {
    ctx.log("this passing test's logs are not printed by default");
}

fn smoke_fail(ctx: &TestContext) -> Result<(), &'static str> {
    ctx.log("this test is expected to fail");
    Err("an expected failure is still a failure")
}

wasi_test::suite!(smoke_pass, smoke_fail);
