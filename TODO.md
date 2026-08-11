# SolarOS Roadmap (TODO)

> Persistent task list. Mark each item with `[x]` when complete.

## Phase 0 - Remaining Ext4/JBD2 tests (verification)
- [x] 0a: Multi-block file (>4KB) write/read + extent merge test (qemu script)
- [x] 0b: Disk full -> NoSpace -> rm -> space reclaim verification test
- [x] 0c: Journal pend ring full -> forced commit + start wrap-around (rollover) test
- [x] 0d: Corrupt commit block (CRC) -> txn skip negative test
- [x] 0e: Remove the 8GiB FS ceiling (64 group limit; mkfs group accounting + 16GiB e2e test)

## Phase 1 - Real hardware boot + input
- [x] 1a: i8042 proper init (controller test, port enable/disable, timeouts, USB legacy emu disable) - `kernel/src/ps2.rs`; QEMU E2E: xlate+set1, IRQ1, help command proves it; USB legacy deferred to 1e/ACPI
- [x] 1b: First UEFI image test in QEMU (solaros-uefi.img; Secure Boot off) - full boot + shell + keyboard E2E with prebuilt OVMF; BIOS regression verified via `status` (uptime s)
  - Required fixes (kernel `interrupts.rs`): `pic_unmask_all()` (OVMF leaves IRQs masked; unmasked on BIOS), PIT 1kHz self-arm (`pit_set_rate`, 0x43/0x40), `sleep_ms` now tick=ms based; uptime seconds calculated `/1000`. We don't trust BIOS to arm the PIT - the kernel sets it up itself.
- [ ] 1c: First boot attempt on a real machine (UEFI image -> USB; screen/serial debug loop)
- [ ] 1d: XHCI driver (USB keyboard, EHCI fallback)
- [x] 1e: ACPI RSDP/RSDT + MADT parsing; IOAPIC + LAPIC init; IRQ overrides (IRQ0->GSI2, keyboard); PIC shutdown
  - `kernel/src/acpi.rs`: PMO window (0xFFFF_8000_0000_0000) with RSDP/RSDT/XSDT + `Acpi::find`; `BootloaderConfig.mappings.physical_memory` opened all physical memory to the kernel (the bootloader page table left everything except the high kernel+fb unmapped; 0x1004000-style probes faulted -> root cause: physical addresses not mapped)
  - `kernel/src/apic.rs`: MADT (records start at 0x2C - Length is total table size, not the header!), ISO override (IRQ0->GSI2), IOAPIC pin mask + PIT->vec32/keyboard->vec33, LAPIC SVR enable, full PIC mask; unconditional LAPIC EOI in handlers (fixed 0xFEE000B0 address, lock-free - a lock in the handler init was fatal: IRQ dropped inside STATE.lock(), ~50% silent stall)
  - BIOS (smp4, 8s boot) + UEFI/OVMF (52s) E2E: `[ OK ] APIC:` lines + keyboard "status" command -> uptime ticking
- [x] 1f: LAPIC timer calibration and timing infrastructure (PIT fallback)
  - `apic::timer_init()`: masked-LVT calibration - 10ms+100ms PIT measurement (wrap-protected, short-run data automatic), periodic 1ms vector 32 setup, then masks the PIT pin at the IOAPIC (out-of-order IRQ window: PIT mask first, then LVT enable). Same vector+handler, so TICKS/sleep_ms/uptime unchanged; PIT fallback + FAIL log if calibration fails.
  - BIOS: 1.157.848 ticks/ms; UEFI: 1.265.067 ticks/ms; prompt + "status" -> uptime ticking on both

## Phase 2 - Memory + processes
- [x] 2a: Heap allocator + global allocator + alloc crate (gradual migration from static buffers)
  - `kernel/src/heap.rs`: first-fit free-list allocator (split+aligned remainder, header-per-block, deterministic dealloc - align_up(16, layout.align()) same function), 16MB heap from the LAST 16MB of the largest Usable region (bootloader memory map, in the PMO window)
  - Fixed panic: split remainder address wasn't aligned (single-size requests like String didn't total 16-multiples) -> align_up(...,16) the remainder and extend the block
  - `history` command (Vec<String>, max 50, skips consecutive dups) - real heap usage; BIOS+UEFI E2E: selftest sum 6290432 ok, history lists
- [x] 2b: Preemptive scheduler (timer -> context switch, idle task, lock disciplines)
  - `kernel/src/sched.rs`: round-robin preemptive (1kHz LAPIC timer -> schedule()), 64KB heap task stacks, `sleep_until` (sleepers list + hlt + timer wake), `switches` counter; kernel boot stack 1MB -> 4MB (`kernel_stack_size`; debug build boot ate ~1MB)
  - Fixed bugs: (1) lock discipline - `SCHED`/`SERIAL1`/heap locks inside `without_interrupts` against IRQ-reentrancy; no lock may be held across `context_switch` (a lock held across a switch hung it - self-deadlock in sleep); (2) `sleep_until` double `SCHED.lock()` (spin is not reentrant); (3) `context_switch` compiler prologue (0x18 spill in debug builds) shifted the restore `ret` -> `global_asm!` prologue-less symmetric frame; (4) kernel boot stack overflow (saved_rsp fell below the stack base -> INSTRUCTION_FETCH PF on a garbage address at restore)
  - BIOS + UEFI/OVMF E2E: `[blinky-a] Ns` every second + `[demo-count]` streaming, live terminal prompt, no panics
- [x] 2c: User mode: GDT user seg + TSS/IST, syscall/sysret MSRs (STAR/LSTAR/FMASK)
  - `kernel/src/gdt.rs`: user CS/DS segments, TSS rsp0 + IST0 (DF_STACK), segment reloads, `enter_user_mode` (iretq frame - preloading ring-3 data segments at CPL0 gives #GP; long mode ignores DS/ES/FS/GS RPL -> not needed); `kernel/src/syscall.rs`: STAR/LSTAR/SFMASK + `global_asm!` syscall entry (swapgs, frame push, dispatch, sysret; segment reloads clobbered rax -> removed); `kernel/src/mem.rs`: `mark_user_pages` (U bit on EVERY level of the walk - incl. PML4E; NX clearing; TLB flush after)
  - Fixed bugs: (1) LLVM "offset is not a multiple of 16" build error - error-code-argument `extern "x86-interrupt"` handlers are broken on this nightly (rust-lang/rust#139679) -> #DF/#GP/#PF asm trampolines + `extern "C"` bodies + `set_handler_addr` (in the x86_64 crate, `idt[8]` Index access panics with "entry is an exception with error code" -> named fields); (2) `SYS_USER_RSP`/`DF_STACK` immutable static -> `.rodata` -> write #PF/#DF -> `static mut`; (3) missing U-bit -> #PF e=0x15 (user fetch); (4) test blob jne/jnz displacement bugs (jnz fell into the middle of dec -> `leave` -> NULL access)
  - BIOS + UEFI/OVMF E2E: `[user] ping 1..5` + `bye` + `[sched] task 3 'demo-user' terminated`, blinky/demo-count uninterrupted, no panics
- [x] 2d: ELF64 static loader + minimal syscall set (read/write/exit...) + first userspace program
  - `user/` crate: no_std + `-pie` + `-nostdlib` + custom linker.ld (base 0xffff800010000000) -> ET_DYN ELF64; inline asm syscall trampoline (write=3, exit=2); build.rs + `[build-dependencies] user = { artifact = "bin" }` (bindeps) embeds the ELF into the kernel (`include_bytes!(env!("USER_ELF"))`)
  - `kernel/src/elf.rs`: PIE loader - PAGE-aligned heap block, `base = blk - min_vaddr`, segment copy + bss zeroing, DT_RELA from PT_DYNAMIC applies 9x R_X86_64_RELATIVE, image + 64KB stack `mark_user_pages`
  - `kernel/src/syscall.rs`: write(3) - fd/len clamp 256 + `mem::is_user_range` guard + scratch copy; read(4) stub; `kernel/src/mem.rs`: `is_user_range`/`check_one_page` (recognizes huge-page levels)
  - Fixed bug: framebuffer `CONSOLE` lock taken with interrupts enabled -> timer IRQ preempting the lock-holding kernel task hung every console write on the spinlock -> all 12 call sites wrapped in `without_interrupts` (serial.rs was already correct)
  - BIOS + UEFI/OVMF E2E: `[elf] loaded PIE` -> `[user] hello from ELF (write syscall)` -> 5x `loop iteration via write` -> `bye` -> `[sched] task 3 'demo-user' terminated`; blinky/demo-count/terminal prompt uninterrupted, no panics
- [x] 2e: Move the shell to userspace (init + terminal process)
  - `user/` crate became a full interactive shell: banner/prompt, line editing (backspace, Ctrl-C cancel), fixed-size 50-entry history (no heap), builtins (help/echo/count/version/clear/color/history/help2) entirely user-space; kernel-dependent commands (fs/users/settings/solarfetch...) via the exec syscall bridge
  - New syscalls: read(4) - blocking unified `input::read_char` for COM1/PS/2 (UTF-8 single char); console(7) - clear/set_fg/reset/backspace; exec(8) - bridges the command line to kernel `commands::execute` (len<=512, is_user_range guard)
  - `kernel/src/input.rs`: unified serial + keyboard input; COM1 RX IRQ (IRQ4, FCR trigger 1 + IER) -> lock-free ring (keyboard RAW_BUF pattern); poll fallback; `serial::try_read_byte`; terminal::read_line (passwd/login prompts) via input
  - `kernel/src/main.rs`: `terminal::run()` removed -> task 0 idle (hlt + tick-stall watchdog), init task enters ring 3 with the shell; banner moved from kernel to user shell
  - Fixed bugs: (1) cmd_echo `line[lens[0]..]` printed NULs -> `..len`; (2) E2E: `-serial stdio` on Windows never delivers `\r`, loses chars even with per-char writes -> `-serial tcp:127.0.0.1:5555,server=on,wait=off` + .NET TcpClient (root cause is qemu/Windows stdio, not the product; UART init clears the FIFO so pre-boot input is also lost)
  - BIOS + UEFI/OVMF E2E: help/echo/backspace-editing/version/count/history(user-space)/whoami/solarfetch/xxbadcmd/color/passwd(masked)/login/ls/Ctrl-C/help2 - all working, prompt re-printed after every command, blinky/demo-count uninterrupted, no panics

## Phase 3 - Real disks
- [x] 3a: PCI enumeration (config space scan) + device list  - `kernel/src/pci.rs`: legacy PIO config space (0xCF8/0xCFC) full enumeration - bus 0 + secondary buses behind PCI-PCI bridges (type-1 header, secondary bus 0x19; discovered-bus queue, MAX_DEVICES=64)
  - Per device: vendor/device/class/subclass/progif/header_type/IRQ line; class/subclass -> readable names (storage/IDE, bridge/Host, display/VGA, network, serialbus/USB...), known-QEMU-device table (8086:1237 PIIX3 host, 7000 ISA, 7010 IDE, 1234:1111 VGA, 8086:100e e1000, VirtIO...)
  - Boot prints `[ OK ] PCI: 5 devices found` + one line per device; `pci` command (from the shell via the exec bridge) prints the full list; added to COMMANDS + user help
  - ECAM (MCFG) deliberately not done - PIO suffices for QEMU `pc` and bus 0 on real hardware; needed for real multi-root-port (1c/3d)
  - BIOS + UEFI/OVMF E2E: boot lines + `pci` command list the same 5 devices, version/whoami regression clean, no panics
- [x] 3b: AHCI driver (HBA init, command list, DMA, 48-bit LBA, read+write)
  - `kernel/src/ahci.rs`: class 01/06 SATA controller from the PCI list, BAR5 (ABAR) -> HBA reset + AHCI enable, per-port DMA memory (CLB/FIS/cmd table via the heap/PMO window), link bring-up (SSTS DET=3), IDENTIFY DMA, READ/WRITE DMA EXT (0x25/0x35), devices registered in ATA slots 4..7 (`via_ahci` flag); the fs layer routes raw read/write through AHCI transparently
  - **Not yet verified in QEMU/Linux:** code carries an in-progress POST-IDENTIFY IO test with debug prints (`[AHCI] dma-read-ok ...`) - the PIO-vs-DMA state question was unresolved at commit time; needs a real `-device ich9-ahci` boot + fs `wtest` regression before calling it solid
- [ ] 3c: MBR/GPT partition parsing + data partition mount (instead of formatting the whole raw disk as ext4)
- [ ] 3d: NVMe driver (after AHCI, optional)

## Phase 3.5 - Linux dev environment + E2E harness (in progress)
- [x] 3.5a: Switch dev host to Linux/Arch: `rustup` + pinned `nightly-2026-08-05` (rust-src, llvm-tools-preview, x86_64-unknown-none/uefi), `qemu-full` + `edk2-ovmf`, `xorriso`/`mtools`; full workspace `cargo build` produces BIOS + UEFI images (bindeps via the pinned nightly)
- [x] 3.5b: ELF loader debug-build panic fix - user PIE is linked high-half (0xffff800010000000); `block - min_vaddr` overflow-checked in dev profile panicked at init. All base-relative math now uses `wrapping_*` (commit 824d46a); dev (fast) builds are the primary test profile
- [ ] 3.5c: Linux QEMU boot + logging regression: BIOS (KVM) and UEFI (OVMF) boot logs, AHCI SATA validation, fs/users E2E - NOT done yet; local-only helpers (`run-qemu.sh`, `e2e.py`, `e2e-run.sh`, gitignored) exist but are UNFINISHED
  - Known harness issues to fix next time: QEMU unix-serial drops the early boot bytes once the demo-task flood (`[demo-count]` every 200ms) exceeds the chardev buffer, so the shell prompt is lost for a late client; the e2e client needs a bounded final drain (the flood never yields silence) and an early-connect runner (present) to capture the prompt before it scrolls out
- [ ] 3.5d: settle the `.cargo/config.toml` `[unstable] bindeps = true` vs stable cargo: the workspace only builds under the pinned nightly toolchain (rust-toolchain.toml), which is fine, but bumpers should `cargo +nightly-2026-08-05 build` explicitly if CARGO_BIN_FILE_* envs are missing

## Phase 4 - Daily-use threshold + installation
- [ ] 4a: ISO image production (BIOS+UEFI dual boot, script packaging bootloader images) - "directly installable" target output
- [ ] 4b: Windows install/write script (ISO -> real disk/USB) + usage guide
- [ ] 4c: reboot/shutdown (ACPI FADT reset, QEMU isa-debug-exit)
- [ ] 4d: RTC/CMOS clock (real time; boot message + uptime)
- [ ] 4e: Text editor + file tools + login flow polish

## Ongoing maintenance
- [ ] After each phase re-run ext4 e2e + JBD crash tests (regression)

## Known limitations (accepted)
- Real hardware boot (1c) still untried; UEFI image boots only in OVMF/QEMU so far (Secure Boot blocked until the bootloader is signed)
- Single core only; no SMP (IOAPIC/LAPIC drives the timer/IRQs on the BSP at CPU0)
- Any USB (XHCI/EHCI) and NVMe sticks remain unsupported; keyboard is PS/2 only (1d)
- Data disk ceiling ~128GiB (LBA28); the whole raw disk is formatted as one filesystem (no MBR/GPT partition parsing yet, 3c)
- No RTC/CMOS clock - uptime is tick-based only (4d)
- Single address space: user mode can still reach the PMO window until per-process page tables (mem.rs marks the PML4E as user-accessible).
