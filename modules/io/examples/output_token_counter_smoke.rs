use ctx_traits_io::harness::OutputTokenCounter;

fn main() {
    let mut counter = OutputTokenCounter::default();

    assert_eq!(
        counter.push(b"{\"usage\":{\"output_tokens\":10}}\n"),
        Some(10)
    );
    assert_eq!(
        counter.push(b"{\"usage\":{\"output_tokens\":12}}\n"),
        Some(2)
    );

    let response = "x".repeat(64 * 1024);
    let mut event =
        format!(r#"{{"type":"result","result":"{response}","usage":{{"output_tokens":25}}}}"#);
    event.push('\n');
    assert_eq!(counter.push(event.as_bytes()), Some(13));
}
