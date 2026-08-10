fn main() {
    let user = std::env::vars()
        .find(|(k, _)| k.starts_with("CARGO_BIN_FILE_USER"))
        .map(|(_, v)| v)
        .expect("CARGO_BIN_FILE_USER* not set - user ELF artifact missing");
    println!("cargo:rustc-env=USER_ELF={}", user);
}
