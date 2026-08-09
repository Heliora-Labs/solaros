# SolarOS Yol Haritası (TODO)

> Kalıcı görev listesi. Her madde tamamlanınca üstüne `[x]` işareti koy.

## Faz 0 — Ext4/JBD2 kalan testler (doğrulama)
- [x] 0a: Çok bloklu dosya (>4KB) yaz/oku + extent birleştirme testi (qemu script)
- [x] 0b: Disk doldur -> NoSpace -> rm -> alan geri kazanımı doğrulama testi
- [x] 0c: Journal pend ring dolu -> commit zorla + start sarması (rollover) testi
- [x] 0d: Bozuk commit bloğu (CRC) -> txn atlama negatif testi
- [x] 0e: 8GiB FS tavanının kaldırılması (64 grup limiti; mkfs grup hesabı + 16GiB e2e testi)

## Faz 1 — Gerçek donanım boot + girdi
- [x] 1a: i8042 proper init (controller test, port enable/devre, timeout'lar, USB legacy emu disable) — `kernel/src/ps2.rs`; QEMU E2E: xlate+set1, IRQ1, help komutu kanıtlı; USB legacy 1e/ACPI'ye devredildi
- [x] 1b: UEFI imajı QEMU'da ilk test (solaros-uefi.img; Secure Boot kapalı) — OVMF prebuilt ile tam boot + shell + klavye E2E; BIOS regresyonu `status` (uptime s) ile doğrulandı
  - Gerekli düzeltmeler (kernel `interrupts.rs`): `pic_unmask_all()` (OVMF maskeli IRQ bırakıyor; BIOS'ta maskesiz), PIT 1kHz self-arm (`pit_set_rate`, 0x43/0x40), `sleep_ms` artık tick=ms tabanlı; uptime saniye hesabı `/1000`. BIOS'un PIT'i arm etmesine güven yok — kernel kendisi kuruyor.
- [ ] 1c: Gerçek makinede ilk boot denemesi (UEFI imaj -> USB; ekran/serial debug döngüsü)
- [ ] 1d: XHCI driver (USB klavye, EHCI fallback)
- [x] 1e: ACPI RSDP/RSDT + MADT parse; IOAPIC + LAPIC init; IRQ override'ları (IRQ0->GSI2, klavye); PIC kapatma
  - `kernel/src/acpi.rs`: PMO penceresi (0xFFFF_8000_0000_0000) ile RSDP/RSDT/XSDT + `Acpi::find`; `BootloaderConfig.mappings.physical_memory` ile tüm fiziksel bellek kernel'e açıldı (bootloader sayfa tablosu high-kernel+fb dışını identity'siz bırakıyordu; 0x1004000 vb. problar fault -> kök neden fiziksel adreslerin eşlenmemiş olmasıydı)
  - `kernel/src/apic.rs`: MADT (0x2C'den başlayan kayıtlar — Length toplam tablo boyutu, header değil!), ISO override (IRQ0->GSI2), IOAPIC pin mask + PIT->vec32/klavye->vec33, LAPIC SVR enable, PIC full mask; handler'larda koşulsuz LAPIC EOI (0xFEE000B0 sabit adres, lock'suz — handler-init lock ölümcül: STATE.lock() içinde IRQ düşerdi, ~%50 silent stall)
  - BIOS (smp4, 8s boot) + UEFI/OVMF (52s) E2E: `[ OK ] APIC:` satırları + klavye "status" komutu -> uptime tıklıyor
- [x] 1f: LAPIC timer kalibrasyonu ve zamanlama altyapısı (PIT fallback)
  - `apic::timer_init()`: LVT timer maskeli kalibrasyon — 10ms+100ms PIT ölçümü (sarma korumalı, kısa süre verisi otomatik), periyodik 1ms vektör 32 kurulumu, ardından PIT pinini IOAPIC'te maskeler (sırasız IRQ penceresi: önce PIT mask, sonra LVT aç). Aynı vektör+handler olduğundan TICKS/sleep_ms/uptime değişmedi; kalibrasyon başarısızsa PIT fallback + FAIL logu.
  - BIOS: 1.157.848 tik/ms; UEFI: 1.265.067 tik/ms; ikisinde prompt + "status" -> uptime tıklıyor

## Faz 2 — Bellek + süreçler
- [ ] 2a: Heap allocator + global allocator + alloc crate (statik buffer'lardan kademeli geçiş)
- [ ] 2b: Preemptive scheduler (timer -> context switch, idle task, lock disiplinleri)
- [ ] 2c: User mode: GDT user seg + TSS/IST, syscall/sysret MSR'ları (STAR/LSTAR/FMASK)
- [ ] 2d: ELF64 statik yükleyici + minimal syscall seti (read/write/exit...) + ilk userspace program
- [ ] 2e: Shell'i userspace'e taşıma (init + terminal process)

## Faz 3 — Gerçek diskler
- [ ] 3a: PCI enumeration (config space tarama) + cihaz listesi
- [ ] 3b: AHCI driver (HBA init, command list, DMA, 48-bit LBA, read+write)
- [ ] 3c: MBR/GPT bölümleme parse + veri bölümü mount (tüm ham diski ext4 biçimleme yerine)
- [ ] 3d: NVMe driver (AHCI sonrası, opsiyonel)

## Faz 4 — Günlük kullanım eşiği + kurulum
- [ ] 4a: ISO imajı üretimi (BIOS+UEFI çift boot, bootloader imajlarını paketleyen script) — "direkt sisteme yüklenebilir" hedef çıktı
- [ ] 4b: Windows install/kesme scripti (ISO'dan gerçek diske/USB'ye yazım) + kullanım kılavuzu
- [ ] 4c: reboot/shutdown (ACPI FADT reset, QEMU isa-debug-exit)
- [ ] 4d: RTC/CMOS saat (gerçek zaman; boot mesajı + uptime)
- [ ] 4e: Metin editörü + dosya araçları + login akışı süsleme

## Sürekli bakım
- [ ] Her faz sonunda ext4 e2e + JBD crash testlerini yeniden koş (regresyon)

## Bilinen sınırlamalar (kabul edilmiş)
- PIO ATA + PS/2 klavye dışında donanım desteği yok (AHCI/XHCI/NVMe gerekli)
- Tek çekirdek; PIC tabanlı kesintiler (APIC/IOAPIC yok)
- Veri diski tavanı: 128GiB (LBA28, 1024 grup); tüm ham disk ext4 olarak biçimlenir (partition parse yok)
- UEFI imajı OVMF/QEMU'da boot ediyor (kernel taraf); gerçek makine + Secure Boot henüz denenmedi (imzasız önyükleyici engeller)
- Saat yok (uptime yalnızca PIT ticks)
