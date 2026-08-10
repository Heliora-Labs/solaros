# ADR-006: User mode + syscall/sysret (Phase 2c)

**Date:** 2026-08-10 · **Status:** Accepted · **Context:** TODO 2c

## Decision

- **Segments/GDT** (`kernel/src/gdt.rs`): kernel + user CS/DS segments, TSS `rsp0`
  (`set_rsp0` on every task switch) + `ist0` (DF_STACK, `static mut`). Entering ring 3 via
  `enter_user_mode(entry, stack_top)`: an iretq frame (SS=0x1B, RSP, RFLAGS=0x202, CS=0x23,
  RIP) pushed with `core::arch::asm!` + iretq. **Ring-3 data segments are NOT preloaded**:
  loading USER_DS while at CPL0 raises #GP (ERR=0x18); long mode does not check the RPL of
  DS/ES/FS/GS, so user code runs fine with the kernel selectors until syscall/sysret.
- **Syscall** (`kernel/src/syscall.rs`): EFER.SCE + STAR (syscall/sysret segment pairs) +
  LSTAR + SFMASK(IF|DF). `global_asm!` entry: `swapgs` -> save user RSP (`SYS_USER_RSP`) ->
  kernel stack (`SYS_KSTACK_TOP`, a mirror of the scheduler's `kstack_top`) -> push a
  15-GPR frame -> `syscall_dispatch(frame)` (rax = syscall number, return value into
  frame.rax) -> pop -> `sysretq`. **No segment reloads**: `mov ax, seg; mov ds, ax` on
  entry/exit clobbered rax (syscall number / return value); they are unnecessary in long
  mode and were removed.
- **Syscall frame layout**: `#[repr(C)]` struct top-of-stack-first (r9..r11);
  `sub rsp, 8` alignment gap at the start of the entry.
- **Exception trampolines** (`kernel/src/interrupts.rs`): `global_asm!` trampolines for
  #DF/#GP/#PF (`mov rdi,[rsp]` / `lea rsi,[rsp+8]` / `call X_c` / `ud2`) + `unsafe extern
  "C"` bodies. **Why:** on this nightly, `extern "x86-interrupt"` handlers with a second
  (error-code) argument hit an LLVM bug - `error: offset is not a multiple of 16`
  (rust-lang/rust#139679, F-abi_x86_interrupt). The x86_64 crate's `idt[8]` Index access
  panics at runtime with "entry 8 is an exception with error code" -> named fields
  (`idt.double_fault` etc.) + `set_handler_addr` + `set_stack_index(0)`.
- **User pages** (`kernel/src/mem.rs`): `mark_user_pages` sets **USER_ACCESSIBLE on every
  level** of the page-table walk (including the PML4E - if the PML4E stays supervisor,
  ring-3 access always #PFs with e=0x15) and clears NO_EXECUTE (the bootloader marks the
  physical-memory window NX; running code from the heap requires it) + **TLB flush**
  (drops stale supervisor translations in the flushed TLB). The whole PMO window becomes
  visible to user - an accepted limitation until 2d's per-process page tables.

## Result

- BIOS + UEFI/OVMF E2E: `[user] ring-3 blob @ ...` -> `ping 1..5` -> `bye (task exiting)`
  -> `[sched] task 3 'demo-user' terminated`; blinky/demo-count continue uninterrupted via
  preemption, live terminal prompt, no panics.
- Fixed bugs: (1) LLVM x86-interrupt error-code build error (trampoline); (2) `.rodata`
  static write #PF->#DF; (3) missing U-bit (e=0x15) - every level incl. PML4E; (4) heap
  fetch #PF due to NX (e=0x15, I/D=1); (5) enter_user_mode #GP (ERR=0x18) - segment
  preload; (6) syscall rax clobber (segment reload); (7) test-blob jne/jnz displacements
  (jnz fell into the middle of dec -> `leave` -> NULL access #PF e=0x4, CR2=0).
- Remaining: single address space (PMO window open to user), single core, user
  RFLAGS/GS not fully preserved (symmetric swapgs, GS base 0) - accepted before 2d/2e.
