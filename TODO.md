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
- [ ] 1b: UEFI imajı QEMU'da ilk test (solaros-uefi.img; Secure Boot kapalı)
- [ ] 1c: Gerçek makinede ilk boot denemesi (UEFI imaj -> USB; ekran/serial debug döngüsü)
- [ ] 1d: XHCI driver (USB klavye, EHCI fallback)
- [ ] 1e: ACPI RSDP/RSDT + MADT parse; IOAPIC + LAPIC init; IRQ override'ları (IRQ0->IRQ2, klavye polarite); PIC kapatma
- [ ] 1f: LAPIC timer kalibrasyonu ve zamanlama altyapısı (PIT fallback)

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
- UEFI imajı üretiliyor ama test edilmedi; Secure Boot imzasız önyükleyiciyi engeller
- Saat yok (uptime yalnızca PIT ticks)
