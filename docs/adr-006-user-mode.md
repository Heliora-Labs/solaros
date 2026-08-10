# ADR-006: User mode + syscall/sysret (Faz 2c)

**Tarih:** 2026-08-10 · **Durum:** Kabul edildi · **Bağlam:** TODO 2c

## Karar

- **Segmentler/GDT** (`kernel/src/gdt.rs`): kernel + user CS/DS segmentleri, TSS
  `rsp0` (her task switch'te `set_rsp0`) + `ist0` (DF_STACK, `static mut`). Ring-3'e
  giriş `enter_user_mode(entry, stack_top)`: iretq frame'i (SS=0x1B, RSP,
  RFLAGS=0x202, CS=0x23, RIP) `core::arch::asm!` push'ları + iretq. **Ring-3 data
  segmentleri ön-yüklenmez**: CPL0 iken USER_DS yüklemek #GP (ERR=0x18) verir; long
  mode'da DS/ES/FS/GS RPL kontrolü yapılmadığından user kod kernel selector'larıyla
  çalışır (syscall/sysret'e kadar).
- **Syscall** (`kernel/src/syscall.rs`): EFER.SCE + STAR (syscall/sysret segment
  çiftleri) + LSTAR + SFMASK(IF|DF). `global_asm!` entry: `swapgs` → user RSP
  kaydet (`SYS_USER_RSP`) → kernel stack (`SYS_KSTACK_TOP`, scheduler'ın
  `kstack_top`'ının yansıması) → 15-GPR frame push → `syscall_dispatch(frame)`
  (rax = syscall no, dönüş değeri frame.rax) → pop → `sysretq`. **Segment
  reload'ları yok**: entry/exit'te `mov ax, seg; mov ds, ax` rax'i (syscall no /
  dönüş değeri) eziyordu; long mode'da gereksiz olduğu için kaldırıldı.
- **Syscall frame düzeni**: `#[repr(C)]` struct top-of-stack-first (r9..r11);
  `sub rsp, 8` align boşluğu entry'nin başında.
- **Exception trampolinleri** (`kernel/src/interrupts.rs`): #DF/#GP/#PF için
  `global_asm!` trampoline (`mov rdi,[rsp]` / `lea rsi,[rsp+8]` / `call X_c` / `ud2`)
  + `unsafe extern "C"` gövde. **Neden:** bu nightly'de error-code ikinci argümanlı
  `extern "x86-interrupt"` handler'ları LLVM bug'ına takılıyor — `error: offset is
  not a multiple of 16` (rust-lang/rust#139679, F-abi_x86_interrupt). x86_64 crate
  `idt[8]` Index erişimi runtime'da "entry 8 is an exception with error code"
  panic'i verir → named field'lar (`idt.double_fault` vb.) + `set_handler_addr` +
  `set_stack_index(0)`.
- **Kullanıcı sayfaları** (`kernel/src/mem.rs`): `mark_user_pages` page-table
  walk'unda **her seviyeye** (PML4E dahil — PML4E supervisor kalırsa ring-3 erişimi
  her zaman #PF e=0x15) USER_ACCESSIBLE + NO_EXECUTE temizleme (bootloader'ın
  fiziksel-bellek penceresi NX işaretli; heap'ten kod çalıştırmak için NX gerekli)
  + **TLB flush** (flushed TLB'deki eski supervisor çevirileri düşürür). PMO
  penceresi bütünüyle user'a görünür — 2d'nin per-process page table'larına kadar
  kabul edilen sınırlama.
- **Writable static'ler**: `SYS_USER_RSP` (syscall asm yazıyor) ve `DF_STACK`
  (IST0) `static mut` — plain `static` .rodata'ya düşüp runtime'da #PF/#DF üretiyordu
  (CR2=blob adresi; `check_exception old: 0xe new 0xe`).

## Sonuç

- BIOS + UEFI/OVMF E2E: `[user] ring-3 blob @ ...` → `ping 1..5` → `bye (task
  exiting)` → `[sched] task 3 'demo-user' terminated`; blinky/demo-count
  preemption ile kesintisiz devam, terminal prompt canlı, panic yok.
- Sabitlenen hatalar: (1) LLVM x86-interrupt error-code build hatası (trampoline);
  (2) `.rodata` static yazma #PF→#DF; (3) U-bit eksikliği (e=0x15) — PML4E dahil
  her seviye; (4) NX yüzünden heap'ten fetch #PF (e=0x15, I/D=1); (5) enter_user_mode
  #GP (ERR=0x18) — segment ön-yükleme; (6) syscall rax clobber (segment reload);
  (7) test blob'da jne/jnz displacement'leri (jnz 0x33'e düşüyordu → `leave` → NULL
  erişim #PF e=0x4, CR2=0).
- Kalan: tek address space (PMO window user'a açık), tek çekirdek, user RFLAGS/GS
  tam korunmuyor (swapgs simetrik, GS base 0) — 2d/2e öncesi kabul.
