#[test]
fn cli() {
    trycmd::TestCases::new().case("tests/cmd/*.toml");
}
