//! Materialise a fixture tree: `cargo run -p o824-fixtures --example mk -- <kind> <dir>`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(kind), Some(root)) = (args.first(), args.get(1)) else {
        eprintln!("usage: mk <kind> <dir>");
        std::process::exit(2);
    };
    o824_fixtures::build_at(kind, std::path::Path::new(root)).expect("build fixture");
    println!("{root}");
}
