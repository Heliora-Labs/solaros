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
- [x] 2a: Heap allocator + global allocator + alloc crate (statik buffer'lardan kademeli geçiş)
  - `kernel/src/heap.rs`: first-fit free-list allocator (split+aligned rest, header-her-blok, dealloc deterministik — align_up(16, layout.align()) aynı fonksiyon), 16MB heap en büyük Usable bölgenin SON 16MB'ından (bootloader memory map, PMO penceresinde)
  - Düzeltilen panik: split'te rest adresi align değildi (String gibi tek boyutlu isteklerde total 16-mult olmuyor) → rest align_up(...,16) + bloğu genişlet
  - `history` komutu (Vec<String>, max 50, ardışık dup'ları atlar) — heap'in gerçek kullanımı; BIOS+UEFI E2E: selftest sum 6290432 ok, history listeler
- [x] 2b: Preemptive scheduler (timer -> context switch, idle task, lock disiplinleri)
  - `kernel/src/sched.rs`: round-robin preemptive (1kHz LAPIC timer -> schedule()), 64KB heap task stack'leri, `sleep_until` (sleepers listesi + hlt + timer wake), `switches` sayacı; kernel boot stack'i 1MB -> 4MB (`kernel_stack_size`; debug build boot ~1MB yiyordu)
  - Düzeltilen hatalar: (1) kilit disiplini — `SCHED`/`SERIAL1`/heap kilitleri IRQ-reentrancy'ye karşı `without_interrupts` içinde; hiçbir kilit `context_switch` boyunca tutulamaz (kilit switch'te asılı kalıyordu → sleep'te self-deadlock); (2) `sleep_until` çift `SCHED.lock()` (spin reentrant değil); (3) `context_switch` compiler prologue'u (debug build 0x18 spill) restore `ret`'ini kaydırıyordu → `global_asm!` ile prologue'suz simetrik frame; (4) kernel boot stack taşması (saved_rsp stack tabanının altına düşüyordu → restore'da garaj adresine INSTRUCTION_FETCH PF)
  - BIOS + UEFI/OVMF E2E: `[blinky-a] Ns` her saniye + `[demo-count]` akıyor, terminal prompt canlı, panic yok
- [x] 2c: User mode: GDT user seg + TSS/IST, syscall/sysret MSR'ları (STAR/LSTAR/FMASK)
  - `kernel/src/gdt.rs`: user CS/DS segmentleri, TSS rsp0 + IST0 (DF_STACK), segment reload'lar, `enter_user_mode` (iretq frame — ring-3 data segment ön-yüklemesi CPL0'da #GP verir, long mode DS/ES/FS/GS RPL'i yok sayar → gerek yok); `kernel/src/syscall.rs`: STAR/LSTAR/SFMASK + `global_asm!` syscall entry (swapgs, frame push, dispatch, sysret; segment reload'lar rax'i eziyordu → kaldırıldı); `kernel/src/mem.rs`: `mark_user_pages` (walk'un HER seviyesine U biti — PML4E dahil; NX temizleme; sonrasında TLB flush)
  - Sabitlenen hatalar: (1) LLVM "offset is not a multiple of 16" build hatası — error-code argümanlı `extern "x86-interrupt"` handler'ları bu nightly'de bozuk (rust-lang/rust#139679) → #DF/#GP/#PF asm trampoline + `extern "C"` gövde + `set_handler_addr` (x86_64 crate'te `idt[8]` Index erişimi "entry is an exception with error code" panic'i verir → named field'lar); (2) `SYS_USER_RSP`/`DF_STACK` immutable static → `.rodata` → yazma #PF/#DF → `static mut`; (3) U-bit eksikliği → #PF e=0x15 (user fetch); (4) test blob'da jne/jnz displacement hataları (jnz dec'in ortasına düşüyordu → `leave` → NULL erişimi)
  - BIOS + UEFI/OVMF E2E: `[user] ping 1..5` + `bye` + `[sched] task 3 'demo-user' terminated`, blinky/demo-count kesintisiz, panic yok
- [x] 2d: ELF64 statik yükleyici + minimal syscall seti (read/write/exit...) + ilk userspace program
  - `user/` crate: no_std + `-pie` + `-nostdlib` + özel linker.ld (taban 0xffff800010000000) ile ET_DYN ELF64; inline asm syscall trampoline (write=3, exit=2); build.rs + `[build-dependencies] user = { artifact = "bin" }` (bindeps) ile ELF kernel'e gömülü (`include_bytes!(env!("USER_ELF"))`)
  - `kernel/src/elf.rs`: PIE loader — heap'ten PAGE-hizalı blok, `base = blok - min_vaddr`, segment kopyalama + bss sıfırlama, PT_DYNAMIC'ten DT_RELA ile 9× R_X86_64_RELATIVE uygulama, görüntü + 64KB stack `mark_user_pages`
  - `kernel/src/syscall.rs`: write(3) — fd/len clamp 256 + `mem::is_user_range` guard + scratch kopya; read(4) stub; `kernel/src/mem.rs`: `is_user_range`/`check_one_page` (huge-page seviyelerini tanır)
  - Sabitlenen hata: framebuffer `CONSOLE` kilidi kesmeler açıkken alınıyordu → timer IRQ kilitli kernel task'ı preempt edince tüm konsol yazımları spinlock'ta asılı kalıyordu → 12 çağrının tümü `without_interrupts` içine alındı (serial.rs zaten doğruydu)
  - BIOS + UEFI/OVMF E2E: `[elf] loaded PIE` → `[user] hello from ELF (write syscall)` → 5× `loop iteration via write` → `bye` → `[sched] task 3 'demo-user' terminated`; blinky/demo-count/terminal prompt kesintisiz, panic yok
- [x] 2e: Shell'i userspace'e taşıma (init + terminal process)
  - `user/` crate'i tam interaktif shell oldu: ring-3'te banner/prompt, satır düzenleme (backspace, Ctrl-C iptal), 50'lik sabit-boy history (heap yok), builtin'ler (help/echo/count/version/clear/color/history/help2) tamamen user-space; kernel bağımlı komutlar (fs/users/settings/solarfetch...) exec syscall köprüsüyle
  - Yeni syscalls: read(4) — COM1/PS/2 birleşik bloklayan `input::read_char` (UTF-8 tek char); console(7) — clear/set_fg/reset/backspace; exec(8) — komut satırını kernel commands::execute'e köprü (len<=512, is_user_range guard)
  - `kernel/src/input.rs`: serial + klavye birleşik girdi; COM1 RX IRQ (IRQ4, FCR trigger 1 + IER) → lock-free ring (klavye RAW_BUF deseni); poll fallback; `serial::try_read_byte`; terminal::read_line (passwd/login prompt'ları) input üzerinden
  - `kernel/src/main.rs`: terminal::run() kaldırıldı → task 0 idle (hlt + tick-stall watchdog), init task shell'i ring-3'e sokuyor; banner kernel'den user shell'e taşındı
  - Sabitlenen hatalar: (1) cmd_echo `line[lens[0]..]` NUL'ları basıyordu → `..len`; (2) E2E: `-serial stdio` Windows'ta `\r`'ı hiç teslim etmiyor, per-char yazımda bile kayıplar → `-serial tcp:127.0.0.1:5555,server=on,wait=off` + .NET TcpClient (kök neden qemu/Windows stdio, ürün değil; UART init FIFO'yu sildiğinden boot-öncesi girdi de kaybolur)
  - BIOS + UEFI/OVMF E2E: help/echo/backspace-düzenleme/version/count/history(user-space)/whoami/solarfetch/xxbadcmd/color/passwd(maskeli)/login/ls/Ctrl-C/help2 — tümü çalışıyor, prompt her komuttan sonra yeniden basılıyor, blinky/demo-count kesintisiz, panic yok

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
