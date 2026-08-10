# ADR-004: Heap allocator (Phase 2/2a)

**Date:** 2026-08-09 · **Status:** Accepted · **Context:** TODO 2a

## Decision

- `#[global_allocator]` first-fit free-list allocator (`kernel/src/heap.rs`), inside a
  `spin::Mutex`.
- The 16MB heap is carved from the **last 16MB of the largest Usable region** in the
  bootloader memory map (high memory; lowest collision risk). Pointers live in the
  physical + PMO (0xFFFF_8000_0000_0000) window - the page table is left untouched.
- Block layout: every block (used/free) carries a `{size, next}` header; dealloc finds the
  pointer deterministically via `ptr - align_up(16, layout.align())` (layout comes from the
  same function).
- On split, the remainder address is rounded to 16 bytes (the first version used only
  `cur+total` - single-size requests like String produced non-16-multiple addresses where
  8-byte derefs hit a misaligned panic).
- `realloc`: alloc+copy+dealloc fallback (only seen on Vec growth).
- Gradual migration: proven by the boot selftest (+`history` command, Vec<String>); static
  buffers (fs/ext4, ata) move over one by one in later work.

## Result

- BIOS/UEFI boot: `[ OK ] Heap: 16 MB @ phys 0x7efe0000 ...` + selftest sum 6290432 (ok).
- `history` command runs on the heap; the PANIC case (misaligned) is fixed.
