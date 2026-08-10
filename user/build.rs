use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let ld = manifest.join("linker.ld");
    let ld = ld.to_str().unwrap().replace('\\', "/");
    println!("cargo:rustc-link-arg=-T{ld}");
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=-nostdlib");
    println!("cargo:rerun-if-changed=linker.ld");
}
