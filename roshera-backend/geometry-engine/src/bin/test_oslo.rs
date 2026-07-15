// Reason: benchmark/diagnostic harness binary -- fixtures are compile-time-
// constant; abort-on-failure is the harness's failure mode. The workspace
// production deny stands for library code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Binary to test Oslo knot insertion algorithm
use geometry_engine::math::test_oslo::test_oslo_simple;

fn main() {
    test_oslo_simple();
}
