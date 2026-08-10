# ADR-009: PCI enumeration — legacy PIO config space (Faz 3a)

**Tarih:** 2026-08-10 · **Durum:** Kabul edildi · **Bağlam:** TODO 3a

## Karar

- **Config space erişimi legacy PIO mekanizmasıyla** (0xCF8/0xCFC): QEMU
  `-machine pc` (PIIX3, sadece bus 0 + isteğe bağlı PCI-PCI köprüler) için
  yeterli ve en basit yol; gerçek donanımda da bus 0 (ve legacy routing ile
  köprüler) PIO ile okunur. ECAM (MCFG, ACPI) bilinçli olarak ertelendi —
  gerçek makine + çok root port'lu topolojiler (1c/3d) gündeme gelince
  eklenir; modül o zaman `config_read_*`'in arkasına takılır.
- **Enumerasyon**: bus 0'dan başla; her cihaz/function için vendor okunur
  (0xFFFF = boş slot → fonksiyon döngüsü kırılır), multifunction biti
  (header type bit 7) fonksiyon 0'da kontrol edilir. Type-1 header (bridge)
  ise 0x19 (secondary bus) okunur ve taranacak bus kuyruğuna eklenir —
  köprü arkasındaki topolojiler de bulunur. Sonuçlar `Vec<PciDevice>`'ta
  (spin::Mutex, MAX_DEVICES=64 sınırı).
- **Cihaz başına okunanlar**: vendor/device/class/subclass/progif/header
  type/IRQ line (0x3C) + bridge'lerde secondary bus. Class/subclass →
  okunur adlar (ör. `storage/IDE`, `bridge/Host`, `display/VGA`,
  `serialbus/USB`, `storage/NVMe`); bilinen QEMU cihaz tablosu
  (8086:1237 PIIX3 host, 8086:7000 PIIX3 ISA, 8086:7010 PIIX3 IDE,
  1234:1111 QEMU VGA, 8086:100e e1000, 1AF4:* VirtIO...) isim verir.
- **Yüzey**: boot'ta `[ OK ] PCI: N devices found` + her cihaz bir `[ OK ]`
  satırı; `pci` komutu (exec köprüsünden shell'de) tam listeyi basar.
  COMMANDS tablosuna ve user help listesine eklendi.
- **Neden PIO değil de MMIO/ECAM değil**: `-machine pc`'de MCFG yok; PIO tüm
  QEMU hedeflerinde (BIOS + OVMF) birebir aynı sonucu verir ve kod en küçük
  olanıdır. ECAM eklenecekse değişiklik `config_read_u32`'nin içinde kalır.

## Sonuç

- BIOS + UEFI/OVMF E2E: ikisinde de 5 cihaz bulunuyor ve birebir aynı:
  00:00.0 8086:1237 host bridge, 00:01.0 8086:7000 ISA, 00:01.1 8086:7010
  IDE, 00:02.0 1234:1111 VGA, 00:03.0 8086:100e e1000. Boot satırları ile
  `pci` komut çıktısı aynı; version/whoami regresyonu temiz; panic yok.
- Bu liste 3b (AHCI) için doğrudan girdi: QEMU'ya `-device ich9-ahci`
  eklendiğinde AHCI controller'ı aynı tabloda `storage/SATA` olarak
  görünecek ve driver BAR'larla başlayacak.
