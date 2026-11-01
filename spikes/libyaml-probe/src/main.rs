//! Probe: does `unsafe-libyaml` reproduce `gopkg.in/yaml.v3`'s "line N" for a
//! broken document?

use unsafe_libyaml::{
    yaml_document_t, yaml_error_type_t, yaml_parser_delete, yaml_parser_initialize,
    yaml_parser_load, yaml_parser_set_input_string, yaml_parser_t,
};

fn main() {
    let cases: [(&str, &str); 7] = [
        ("broken-cordis", "- id: dsh-base\n  enabled: [true, false\n"),
        ("broken-eof", "- id: dsh-base\n  enabled: [true, false"),
        ("flow-line1", "a: [1,\n 2,\n 3\n"),
        ("flow-line2", "x: 1\ny: [1,\n 2\n"),
        ("flow-line3", "x: 1\ny: 2\nz: [1,\n 2\n"),
        ("mid-doc", "a: 1\n b: 2\n"),
        ("bad-tab", "a:\n\t- 1\n"),
    ];
    for (name, text) in cases {
        println!("{name:16} {}", describe(text));
    }
}

fn describe(text: &str) -> String {
    unsafe {
        let mut parser = std::mem::zeroed::<yaml_parser_t>();
        if !yaml_parser_initialize(&mut parser).ok {
            return "init failed".to_owned();
        }
        yaml_parser_set_input_string(&mut parser, text.as_ptr(), text.len() as u64);

        let mut document = std::mem::zeroed::<yaml_document_t>();
        let loaded = yaml_parser_load(&mut parser, &mut document);
        let report = if loaded.ok {
            "OK".to_owned()
        } else {
            let error = parser.error;
            let scanner = error == yaml_error_type_t::YAML_SCANNER_ERROR;
            let mut line = 0u64;
            if parser.context_mark.line != 0 {
                line = parser.context_mark.line;
                if scanner {
                    line += 1;
                }
            } else if parser.problem_mark.line != 0 {
                line = parser.problem_mark.line;
                if scanner {
                    line += 1;
                }
            }
            format!(
                "ERR kind={error:?} ctx={} prob={} => line={line} problem={:?}",
                parser.context_mark.line,
                parser.problem_mark.line,
                cstr(parser.problem)
            )
        };
        yaml_parser_delete(&mut parser);
        report
    }
}

unsafe fn cstr<'a>(pointer: *const i8) -> &'a str {
    if pointer.is_null() {
        return "";
    }
    let bytes = pointer as *const u8;
    let mut length = 0usize;
    while *bytes.add(length) != 0 {
        length += 1;
    }
    std::str::from_utf8(std::slice::from_raw_parts(bytes, length)).unwrap_or("")
}
