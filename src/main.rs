fn main() {
    let bios = std::env::var("BIOS_PATH").unwrap_or_default();
    let uefi = std::env::var("UEFI_PATH").unwrap_or_default();
    println!("NovaOS boot imajları:");
    println!("  BIOS: {bios}");
    println!("  UEFI: {uefi}");
    println!();
    println!("QEMU'da çalıştırmak için: .\\run-qemu.ps1");
}
