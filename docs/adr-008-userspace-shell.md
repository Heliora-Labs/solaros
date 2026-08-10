# ADR-008: Userspace shell + kernel service bridge (Phase 2e)

**Date:** 2026-08-10 · **Status:** Accepted · **Context:** TODO 2e

## Decision

- **The init process is now a user-space shell** (`user/` crate): banner + `solaros> `
  prompt + line editing (backspace, Ctrl-C cancel) + fixed-size 50-entry history (no heap,
  `static mut` arrays) entirely in ring 3. Builtin commands (help, echo, count, version,
  clear, color, history, help2) run in user space; the kernel `terminal::run()` loop was
  removed - task 0 is idle (hlt + tick-stall watchdog), the `init` task loads the ELF and
  enters ring 3 via `enter_user_mode`. The kernel banner moved into the shell.
- **New syscalls** (`kernel/src/syscall.rs`):
  - `4 read(fd, buf, cap)`: fd=0; waits for one decoded character from
    `input::read_char()` (UTF-8, single char, requires cap>=4; blocking - waits with
    `enable_and_hlt`, the scheduler may preempt), `is_user_range` guard.
  - `7 console(op, a)`: 0=clear, 1=set_fg (0xRRGGBB), 2=reset_colors, 3=backspace - the
    shell manages its own prompt/color/screen via syscalls.
  - `8 exec(ptr, len)`: **kernel service bridge** - forwards the command line (UTF-8,
    <=512 B) to `commands::execute`. Why: shell logic is user-side, but the
    fs/users/settings/ata/framebuffer services are not yet exposed as a syscall API; since
    the single address space is already an accepted limitation, the string-based bridge is
    the accepted transitional architecture until a real per-process/FD API (2f+). Output
    goes through the kernel `_print` (framebuffer+serial); unknown-command messages come
    from the kernel.
- **Unified console input** (`kernel/src/input.rs`): COM1 + PS/2 keyboard under a single
  `read_char()`. Serial: **RX interrupt** - FCR trigger 1 + IER (IRQ4, PIC offset 36,
  `serial_interrupt_handler` replaces ate_irq_36 in the IDT) -> lock-free ring buffer (the
  keyboard `RAW_BUF` atomic pattern; no spinlock in the IRQ) + `drain_receiver` poll
  fallback (bytes that arrived/were missed before the IRQ was armed). `serial::try_read_byte`
  LSR poll. Input is ASCII-only (CR/LF->`\n`, 0x08/0x03 pass through). The keyboard path is
  unchanged (IRQ->scancode ring->lazy decode). `terminal::read_line` (passwd/login
  prompts) uses the same `input::read_char` - the single-reader model is preserved.
- **E2E transport**: `-serial stdio` is unreliable on Windows (CR never delivered, char
  loss even with per-char writes) -> **`-serial tcp:127.0.0.1:5555,server=on,wait=off`** +
  .NET TcpClient (40ms per character, prevents QEMU 16550 FIFO overflow; connect after
  boot - UART init clears the FIFO, pre-connection input is lost).

## Result

- BIOS E2E: `help` (user-space list) -> `echo hello from ring 3` -> backspace editing
  (`echox[BS] backspace` -> "backspace") -> `version` -> `count 3` -> `history` (1..6,
  user-space) -> `whoami` (root) -> `solarfetch` (full layout) -> `xxbadcmd` ("Unknown
  command" - bridge) -> `color green` -> `passwd` (two masked prompts -> "password
  updated") -> `login root` (masked -> "logged in as root") -> `ls` (directory listing) ->
  Ctrl-C line cancel -> `help2`; the prompt is re-printed after every command,
  blinky/demo-count uninterrupted, no panics.
- UEFI/OVMF E2E: the whole flow passed, no panics.
- Fixed bugs: (1) `cmd_echo(&line[lens[0]..])` printed NUL characters -> `..len`;
  (2) E2E harness: stdio CR loss (qemu/Windows, not the product) -> TCP.
- Remaining: exec bridge is temporary - real per-process/FD syscall API (2f+); serial
  input is ASCII-only (Turkish characters only via PS/2); single address space (ADR-006/007
  limitations).
