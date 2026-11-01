//! Materialises one fixture tree on disk, outside a test.
//!
//! ```text
//! cargo run -p o824-fixtures --example materialize -- <kind> <directory>
//! ```
//!
//! This exists so a tree can be handed to something that is not a Rust test —
//! the reference implementation above all, which is how the fixture builder
//! itself is checked against the oracle.

use std::path::PathBuf;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(kind) = arguments.next() else {
        eprintln!("usage: materialize <kind> <directory>");
        std::process::exit(2);
    };
    let Some(directory) = arguments.next() else {
        eprintln!("usage: materialize <kind> <directory>");
        std::process::exit(2);
    };
    let directory = PathBuf::from(directory);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!("cannot create {}: {error}", directory.display());
        std::process::exit(2);
    }
    match o824_fixtures::build_at(&kind, &directory) {
        Ok(()) => println!("{}", directory.display()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
