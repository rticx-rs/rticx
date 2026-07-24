fn main() {
    // Make `memory.x` (next to this package) discoverable by the linker.
    println!("cargo:rustc-link-search=.");
}