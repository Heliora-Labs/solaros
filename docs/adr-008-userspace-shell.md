# ADR-008: Userspace shell + kernel service bridge (Faz 2e)

**Tarih:** 2026-08-10 · **Durum:** Kabul edildi · **Bağlam:** TODO 2e

## Karar

- **Init process artık user-space shell** (`user/` crate): banner + `solaros> `
  prompt + satır düzenleme (backspace, Ctrl-C iptal) + 50 satırlık sabit-boy
  history (heap yok, `static mut` diziler) tamamen ring 3'te. Builtin komutlar
  (help, echo, count, version, clear, color, history, help2) user-space'te
  koşar; kernel `terminal::run()` döngüsü kaldırıldı — task 0 idle (hlt +
  tick-stall watchdog), `init` task'ı ELF'i yükleyip `enter_user_mode` ile
  ring 3'e giriyor. Kernel banner'ı shell'e taşındı.
- **Yeni syscall'lar** (`kernel/src/syscall.rs`):
  - `4 read(fd, buf, cap)`: fd=0; `input::read_char()` ile bir dekode karakter
    bekler (UTF-8, tek char, cap>=4 gereği; bloklayıcı — `enable_and_hlt` ile
    bekler, scheduler preempt edebilir), `is_user_range` guard.
  - `7 console(op, a)`: 0=clear, 1=set_fg (0xRRGGBB), 2=reset_colors,
    3=backspace — shell kendi prompt/renk/ekran yönetimini syscall ile yapar.
  - `8 exec(ptr, len)`: **kernel service bridge** — komut satırını (UTF-8,
    ≤512 B) `commands::execute`'a köprüler. Neden: shell logic'i user-side,
    ama fs/users/settings/ata/framebuffer servisleri henüz syscall API'sine
    dökülmedi; tek address space zaten kabul edilmiş sınırlama olduğundan
    string tabanlı köprü, gerçek per-process/FD API'sine (2f+) kadar kabul
    edilen geçiş mimarisidir. Output kernel `_print` ile (framebuffer+serial)
    gider, unknown komut mesajı kernel'den döner.
- **Birleşik konsol girdisi** (`kernel/src/input.rs`): COM1 + PS/2 klavye tek
  `read_char()` altında. Serial: **RX interrupt** — FCR trigger 1 + IER
  (IRQ4, PIC offset 36, IDT'de ate_irq_36 yerine `serial_interrupt_handler`)
  → lock-free halka tampon (klavye `RAW_BUF` atomic deseni; IRQ'da spinlock
  almamak için) + `drain_receiver` poll fallback (IRQ silahlanmadan önce
  gelen/kaçan baytlar). `serial::try_read_byte` LSR poll. Girdi ASCII-only
  (CR/LF→`\n`, 0x08/0x03 passthrough). Klavye yolu değişmedi (IRQ→scancode
  ring→lazy decode). `terminal::read_line` (passwd/login prompt'ları) da aynı
  `input::read_char`'ı kullanır — tek okuyucu modeli korunur.
- **E2E transport**: `-serial stdio` Windows'ta güvenilmez (CR hiç teslim
  edilmiyor, per-char yazımda bile karakter kaybı) → **`-serial
  tcp:127.0.0.1:5555,server=on,wait=off`** + .NET TcpClient (karakter
  başına 40 ms yazım, QEMU 16550 FIFO taşmasını önler; boot-sonrası bağlan —
  UART init FIFO'yu temizler, bağlantı-öncesi girdi kaybolur).

## Sonuç

- BIOS E2E: `help` (user-space liste) → `echo hello from ring 3` → backspace
  düzenleme (`echox[BS] backspace` → "backspace") → `version` → `count 3` →
  `history` (1..6, user-space) → `whoami` (root) → `solarfetch` (tam
  layout) → `xxbadcmd` ("Unknown command" — köprü) → `color green` → `passwd`
  (maskeli iki prompt → "password updated") → `login root` (maskeli →
  "logged in as root") → `ls` (dizin listesi) → Ctrl-C satır iptali →
  `help2`; her komuttan sonra prompt yeniden basılıyor, blinky/demo-count
  kesintisiz, panic yok.
- UEFI/OVMF E2E: aynı akışın tamamı geçti, panic yok.
- Sabitlenen hatalar: (1) `cmd_echo(&line[lens[0]..])` NUL karakterlerini
  basıyordu → `..len`; (2) E2E harness: stdio CR kaybı (qemu/Windows, ürün
  değil) → TCP.
- Kalan: exec köprüsü geçici — gerçek per-process/FD syscall API'si (2f+);
  serial girdi ASCII-only (Türkçe karakterler yalnız PS/2'den); tek address
  space (ADR-006/007 sınırlamaları).
