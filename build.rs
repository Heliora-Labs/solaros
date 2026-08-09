use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_SOLARCORE_solarcore")
            .expect("CARGO_BIN_FILE_SOLARCORE_solarcore ayarlanmadı"),
    );

    let bios_path = out_dir.join("solaros-bios.img");
    bootloader::BiosBoot::new(&kernel)
        .create_disk_image(&bios_path)
        .expect("BIOS disk imajı oluşturulamadı");
    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());

    let uefi_path = out_dir.join("solaros-uefi.img");
    match bootloader::UefiBoot::new(&kernel).create_disk_image(&uefi_path) {
        Ok(()) => {
            println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
        }
        Err(e) => {
            println!("cargo:warning=UEFI imajı oluşturulamadı (atlandı): {e:?}");
        }
    }
}
