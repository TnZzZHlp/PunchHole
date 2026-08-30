use PunchHole::script_arguments;

#[test]
fn builds_three_separate_script_arguments() {
    assert_eq!(
        script_arguments("203.0.113.7:42424".parse().unwrap(), 10001),
        vec!["203.0.113.7", "42424", "10001"]
    );
}
