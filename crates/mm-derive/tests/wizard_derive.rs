#[test]
fn wizard_derive_pass() {
    let t = trybuild::TestCases::new();
    t.pass("tests/pass/*.rs");
}

#[test]
fn wizard_derive_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/fail/*.rs");
}
