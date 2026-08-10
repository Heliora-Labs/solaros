# ADR-007: ELF64 PIE loader + write/read syscalls (Faz 2d)

**Tarih:** 2026-08-10 · **Durum:** Kabul edildi · **Bağlam:** TODO 2d

## Karar

- **Userspace programı derlenmiş ELF olarak üretilir** (`user/` crate): no_std +
  `-pie` + `-nostdlib` + özel linker script (taban 0xffff800010000000) ile
  ET_DYN ELF64; build.rs `R_X86_64_RELATIVE` (9 reloc, hepsi .data'da) dışında
  türsüz reloc içermiyor. Cargo bindeps ile gömme: kernel
  `Cargo.toml`'da `[build-dependencies] user = { path = "../user", artifact =
  "bin", target = "x86_64-unknown-none" }`, `kernel/build.rs`
  `CARGO_BIN_FILE_USER*`'ı `cargo:rustc-env=USER_ELF` yapıyor, kernel
  `include_bytes!(env!("USER_ELF"))` ile alıyor — elle kopyalanan `USER_BLOB`
  (2c) ve payload-elf copy-paste'ı kalktı, imaj derleme hattının parçası.
  (`user` kök workspace'e `exclude = ["user"]` ile dahil edilmez; bağımsız
  derleme için `[workspace]` taşınamaz — çift workspace kökü hatası.)
- **PIE loader** (`kernel/src/elf.rs`): heap'ten PAGE hizalı blok
  (`ALLOCATOR.alloc(Layout::from_size_align_unchecked(size, PAGE))`), `base =
  blok - min_vaddr`; segmentler `copy_nonoverlapping`, PT_LOAD içi fazlalık
  (bss) sıfırlanır; PT_DYNAMIC'ten DT_RELA/DT_RELASZ/DT_RELAENT okunur,
  R_X86_64_RELATIVE: `(base + r_offset)` ← `base + r_addend`; görüntü + 64 KiB
  user stack `mark_user_pages` ile user'a açılır; entry `base + e_entry`.
  vaddr tabanı sabit değildir — her yüklemede heap pozisyonuna göre değişir
  (2c'deki hand-rolled blob gibi hizalanmış, ama şimdi gerçek ELF).
- **Syscall seti** (`kernel/src/syscall.rs`): 2=exit (2c'den), 3=write —
  fd=1 (0 reddedilir), len 256'ya clamp, `mem::is_user_range` guard (yanlış
  adres → u64::MAX dönüşü), veri önce kernel scratch buffer'a kopyalanır
  (user page table'ı olmadığından, kernel page table'ında yürütülen guard
  yeterli: user penceresi kernel haritasının parçası), ardından
  `crate::_print`; 4=read stub (guard + 0 döner). Giriş: 2c'nin 15-GPR frame
  syscall trampoline'ı aynen kullanılır.
- **Guard yardımcıları** (`kernel/src/mem.rs`): `is_user_range(vaddr, len)` +
  salt-okunur `check_one_page` — page-table walk'unda her seviyede
  PRESENT|USER_ACCESSIBLE doğrulaması; PDPT (1 GiB) ve PD (2 MiB) huge-page
  seviyelerini tanır.
- **Konsol kilidi disiplini** (`kernel/src/framebuffer.rs`): `CONSOLE`
  spinlock'u yalnız `without_interrupts` içinde alınır (12 çağrı). Nedeni:
  kesmeler açıkken tutulan kilit, timer IRQ kilit sahibi task'ı preempt
  edince diğer tüm task'ların konsol yazımlarını sonsuz spin'e sokuyordu
  (serial.rs bu disiplini 2b'den beri uyguluyordu).

## Sonuç

- BIOS E2E: `[elf] applied 9 relative relocs` → `[elf] loaded PIE: 16384
  bytes @ 0xffff80007f0190b0, entry 0xffff80007f0191e0` → `[user] hello from
  ELF (write syscall)` → `S O L A R O S 26.1` banner → 5× `[user] loop
  iteration via write` → `[user] ELF program done, exiting` → `bye` →
  `[sched] task 3 'demo-user' terminated`; `solaros>` prompt canlı,
  blinky-a/demo-count 57s'ye kadar kesintisiz, panic yok.
- UEFI/OVMF E2E: aynı akış, blinky-a 187s'ye kadar, panic yok.
- Sabitlenen hata: (1) framebuffer CONSOLE lock IRQ-reentrancy — `-d int`
  teşhisi: ring-3 girişinden hemen sonra sabit RIP/SP'de spin, her timer
  tick'inde RAX artıyor (lock bekleme döngüsü); `without_interrupts` ile
  çözüldü. (2) QEMU'da boot imajı ilk `-drive` olmalı (ikinci sıraya
  konunca serial log boş kalıyor).
- Kalan: tek address space (PMO penceresi user'a açık), tek çekirdek —
  2e/2f öncesi kabul.
