fn main() {
    // The frontend is embedded at compile time, so a change to it is a reason to rebuild
    // this crate. Without these lines a rebuilt binary keeps serving the previous page,
    // which looks exactly like "my fix did nothing".
    println!("cargo:rerun-if-changed=dist");
    println!("cargo:rerun-if-changed=dist/index.html");
    println!("cargo:rerun-if-changed=dist/app.js");
    println!("cargo:rerun-if-changed=dist/style.css");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
