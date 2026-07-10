//! Ensure cargo rebuilds when the embedded descriptor data changes.
//! `include_dir!` does not track directory contents on its own.

fn main() {
    println!("cargo:rerun-if-changed=descriptors");
}
