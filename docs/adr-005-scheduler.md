# ADR-005: Preemptive scheduler (Phase 2/2b)

**Date:** 2026-08-10 · **Status:** Accepted · **Context:** TODO 2b

## Decision

- Round-robin preemptive scheduler (`kernel/src/sched.rs`): `schedule()` runs inside the
  1kHz LAPIC timer IRQ (vec 32) - the current task goes to the back of the ready queue, the
  next Ready task is picked and switched to via `context_switch`. Task stacks live on the
  heap (64KB, `Box<[u8]>`); task 0 is the kernel (boot stack) and never sleeps.
- `sleep_until(target)`: task becomes `Sleeping(target)` + `sleepers` list; the timer
  handler moves woken tasks back to ready; a sleeping task waits with `enable_and_hlt()`
  (no busy-wait).
- **Lock discipline (critical):** `SCHED`/`SERIAL1`/heap locks are always taken inside
  `without_interrupts` (IRQ-reentrancy -> spin deadlock); **no lock may be held across
  `context_switch`** - `schedule()` picks the target, `drop(s)`s the lock, and only then
  switches. Holding the lock across a switch leaves it stuck in the kernel frame and
  `sleep_until` spins on it forever.
- `context_switch` is written in **`global_asm!`** (no prologue): with a normal Rust fn +
  `asm!`, the compiler emitted a 0x18-byte argument-spill prologue in debug builds; even
  with a symmetric `saved_rsp` frame, the restore `ret` read from the spill area (garbage
  address -> INSTRUCTION_FETCH PF). With `global_asm!` the frame is fully symmetric:
  6 callee-saved pushes -> `mov [rdi], rsp` -> `mov rsp, rsi` -> 6 pops -> `ret`. A new
  task frame is set up at spawn as `[0x6, entry]`.
- Kernel boot stack 1MB -> 4MB (`BootloaderConfig.kernel_stack_size`): in debug builds the
  boot (ps2/acpi/apic/disk/fat-ext4 probe) used to approach ~1MB; the first preemption IRQ
  frame sank below the stack base and `saved_rsp` pointed at garbage memory.
- `switches()` atomic counter (demo indicator); `MAX_TASKS=24`, spawn allocates heap stack
  + frame.

## Result

- BIOS + UEFI/OVMF E2E: `[blinky-a] 0s/1s/2s/...` every second (sleep+preempt),
  `[demo-count]` streams, kernel main continues cleanly between switches (live terminal
  prompt), no panics. Fixed bugs: lock-across-switch deadlock, `sleep_until` double lock,
  context_switch prologue shift, boot stack overflow.
