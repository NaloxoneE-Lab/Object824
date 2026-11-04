fn main() {
    // `option_env!` is read at compile time, and cargo does not know that a change to this
    // variable means the crate must be rebuilt. Without this, building a release and then
    // rebuilding with the key set quietly keeps the keyless binary — which is exactly the
    // kind of silent wrong answer a trusted key must never produce.
    println!("cargo:rerun-if-env-changed=O824_RELEASE_KEY");
}
