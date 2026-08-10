# ADR-007: ELF64 PIE loader + write/read syscalls (Phase 2d)

**Date:** 2026-08-10 · **Status:** Accepted · **Context:** TODO 2d

## Decision

- **The userspace program is produced as a compiled ELF** (`user/` crate): no_std +
  `-pie` + `-nostdlib` + a custom linker script (base 0xffff800010000000) -> ET_DYN ELF64;
  build.rs output contains only `R_X86_64_RELATIVE` relocs (9, all in .data). Embedding
  via Cargo bindeps: kernel `Cargo.toml` has `[build-dependencies] user = { path =
  "../user", artifact = "bin", target = "x86_64-unknown-none" }`, `kernel/build.rs` maps
  `CARGO_BIN_FILE_USER*` to `cargo:rustc-env=USER_ELF`, and the kernel grabs it with
  `include_bytes!(env!("USER_ELF"))` - the hand-copied `USER_BLOB` (2c) and payload-elf
  copy-paste are gone; the image is part of the build pipeline. (`user` is excluded from
  the root workspace via `exclude = ["user"]`; it cannot carry its own `[workspace]` for
  standalone builds - double workspace root error.)
- **PIE loader** (`kernel/src/elf.rs`): PAGE-aligned block from the heap
  (`ALLOCATOR.alloc(Layout::from_size_align_unchecked(size, PAGE))`), `base = blk -
  min_vaddr`; segments copied with `copy_nonoverlapping`, extra bytes inside PT_LOAD (bss)
  zeroed; DT_RELA/DT_RELASZ/DT_RELAENT read from PT_DYNAMIC, R_X86_64_RELATIVE:
  `(base + r_offset)` <- `base + r_addend`; image + 64KiB user stack opened to user via
  `mark_user_pages`; entry = `base + e_entry`. The vaddr base is not fixed - it follows the
  heap position on every load (aligned like the 2c hand-rolled blob, but now a real ELF).
- **Syscall set** (`kernel/src/syscall.rs`): 2=exit (from 2c), 3=write - fd=1 (0
  rejected), len clamped to 256, `mem::is_user_range` guard (bad address -> u64::MAX
  return), data first copied to a kernel scratch buffer (no per-process page table, so the
  guard executed in the kernel page table suffices: the user window is part of the kernel
  map), then `crate::_print`; 4=read stub (guard + returns 0). Entry: the 15-GPR
  syscall trampoline from 2c is reused unchanged.
- **Guard helpers** (`kernel/src/mem.rs`): `is_user_range(vaddr, len)` + read-only
  `check_one_page` - verifies PRESENT|USER_ACCESSIBLE at every level of the page-table
  walk; recognizes PDPT (1GiB) and PD (2MiB) huge-page levels.
- **Console lock discipline** (`kernel/src/framebuffer.rs`): the `CONSOLE` spinlock is
  only taken inside `without_interrupts` (12 call sites). Reason: a lock held with
  interrupts enabled, when the timer IRQ preempts the owning task, put every other task's
  console write into an infinite spin (serial.rs has followed this discipline since 2b).

## Result

- BIOS E2E: `[elf] applied 9 relative relocs` -> `[elf] loaded PIE: 16384 bytes @
  0xffff80007f0190b0, entry 0xffff80007f0191e0` -> `[user] hello from ELF (write syscall)`
  -> `S O L A R O S 26.1` banner -> 5x `[user] loop iteration via write` -> `[user] ELF
  program done, exiting` -> `bye` -> `[sched] task 3 'demo-user' terminated`; live
  `solaros>` prompt, blinky-a/demo-count uninterrupted for 57s, no panics.
- UEFI/OVMF E2E: same flow, blinky-a up to 187s, no panics.
- Fixed bugs: (1) framebuffer CONSOLE lock IRQ-reentrancy - `-d int` diagnosis: right
  after ring-3 entry a spin at a fixed RIP/SP, RAX incrementing on every timer tick (lock
  wait loop); solved with `without_interrupts`. (2) On QEMU the boot image must be the
  first `-drive` (in second position, the serial log stays empty).
- Remaining: single address space (PMO window open to user), single core - accepted before
  2e/2f.
