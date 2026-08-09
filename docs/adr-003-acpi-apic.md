# ADR-003: ACPI + APIC (IRQ yönlendirme LAPIC/IOAPIC'e)

**Tarih:** 2026-08-09 · **Durum:** Kabul edildi (Faz 1/1e) · **Bağlam:** TODO 1e

## Karar

- Tüm fiziksel bellek erişimi tek bir sabit pencereden (PMO = 0xFFFF_8000_0000_0000) yapılır.
  Bootloader'ın sayfa tablosu high-kernel + framebuffer dışında fiziksel adres içermiyor;
  `BootloaderConfig.mappings.physical_memory = FixedAddress(PMO)` ile ACPI tabloları,
  LAPIC ve IOAPIC MMIO okunabilir hale gelir. Sayfa-tablo walk / scratch PT yaklaşımları
  denenip atıldı (kernel'de leaf-PT çerçeveleri identity'de yok, walk çalışıyordu ama çerçeveler
  G/çüktü).
- ACPI: RSDP (bootloader verisi önce; checksum 20; fallback `find_root`: Usable bölgelerin
  SON 64KB'ı 16-byte adımla — tüm bölge taraması BIOS'ta sonsuz döngüye gidiyor). XSDT (rev>=2)
  ya da RSDT; `Acpi` struct + `find(sig)`.
- MADT parse: kayıtlar **sabit 0x2C offset'inden** başlar; `Length` alanı toplam tablo boyutudur
  (header değil). ISO kayıtları IRQ->GSI+flags sağlar (QEMU pc: IRQ0->GSI2).
- IOAPIC: sadece PIT(32) ve klavye(33) pinleri açık; diğer pinler maskelenir. PIC tamamen
  maskelenir (`pic_mask_all`, 0x21/0xA1=0xFF).
- **LAPIC EOI handler içinde koşulsuz ve lock'suz**: sabit FEE000B0 adresine yazılır.
  İlk tasarım (`routed()` bayrağı + handler'da `STATE.lock()`) ölümcüldü: timer IRQ init'in
  kritik bölgesine denk geldiğinde handler kendi spinlock'unu bekler ve sistem sessizce kilitlenir
  (~%50 şansla başarısız boot). Konu: handler yığınında asla init ile paylaşılan spinlock olmamalı.

## Sonuç

- BIOS (TCG, -smp 4): 2G/4CPU, prompt ~8s, key E2E çalışıyor; UEFI/OVMF benzeri çalışıyor (52s).
- `[ OK ] APIC: ... IRQ0 -> GSI2 (PIT), IRQ1 -> GSI1 (kbd); PIC masked`
- 1f (LAPIC timer kalibrasyon) sıradaki aday; PIT fallback duruyor.