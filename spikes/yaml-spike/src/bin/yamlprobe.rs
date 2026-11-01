fn main() {
    let cases: [(&str, &str); 5] = [
        ("broken-cordis", "- id: dsh-base\n  enabled: [true, false\n"),
        ("broken-eof", "- id: dsh-base\n  enabled: [true, false"),
        ("bad-indent", "- id: a\n   x: 1\n  y: 2\n"),
        ("unclosed-brace", "a: {b: 1\n"),
        ("tab", "\t- a\n"),
    ];
    for (name, text) in cases {
        match yaml_rust2::YamlLoader::load_from_str(text) {
            Ok(docs) => println!("{name:16} OK docs={}", docs.len()),
            Err(error) => {
                let marker = error.marker();
                println!(
                    "{name:16} ERR line0={} col0={} index={} info={:?}",
                    marker.line(),
                    marker.col(),
                    marker.index(),
                    error.info()
                );
            }
        }
    }
}
