use crate::server::command::{CommandSplitter, parse_str_to_u64};

#[test]
fn test_command_splitter() {
    let command: Vec<String> = CommandSplitter::new("this is my command").map(|f| f.unwrap()).collect();
    assert_eq!(["this", "is", "my", "command"], command.as_slice());

    let command: Vec<String> = CommandSplitter::new("this is \"my command\"").map(|f| f.unwrap()).collect();
    assert_eq!(["this", "is", "my command"], command.as_slice());

    let command: Vec<String> = CommandSplitter::new("this is my\\ command").map(|f| f.unwrap()).collect();
    assert_eq!(["this", "is", "my command"], command.as_slice());

    let command: Vec<String> = CommandSplitter::new("this is \"\\\"my\\\"\" command").map(|f| f.unwrap()).collect();
    assert_eq!(["this", "is", "\"my\"", "command"], command.as_slice());
}

#[test]
fn test_parse_hexadecimal() {
    assert_eq!(Some(0xDEADBEEF), parse_str_to_u64("0xDEADBEEF"));
    assert_eq!(Some(0xDEADBEEF), parse_str_to_u64("0xdeadbeef"));
    assert_eq!(Some(0xDEADBEEF), parse_str_to_u64("3735928559"));
}
