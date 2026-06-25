//! Ensure cargo rebuilds when the embedded adapter/catalog data changes.
//! `include_dir!` does not track directory contents on its own, so without this
//! a newly added or edited `adapters/*.yaml` would not be re-embedded.

fn main() {
    println!("cargo:rerun-if-changed=adapters");
    println!("cargo:rerun-if-changed=catalog");
}
