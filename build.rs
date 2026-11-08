// Build the committed training data (data/training.bin) into the program, so a fresh
// install has working models before it ever reaches Wikipedia. If the file isn't there,
// the program builds without it and pulls its training data on first run.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(bundled_training)");
    println!("cargo:rerun-if-changed=data/training.bin");
    if std::path::Path::new("data/training.bin").exists() {
        println!("cargo:rustc-cfg=bundled_training");
    }
}
