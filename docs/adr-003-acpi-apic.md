# ADR-003: ACPI + APIC (interrupt routing to LAPIC/IOAPIC)

**Date:** 2026-08-09 · **Status:** Accepted (Phase 1/1e) · **Context:** TODO 1e

## Decision

- All physical memory access goes through a single fixed window (PMO = 0xFFFF_8000_0000_0000).
  The bootloader page table contains no physical addresses besides the high kernel and the
  framebuffer; `BootloaderConfig.mappings.physical_memory = FixedAddress(PMO)` makes ACPI
  tables, LAPIC and IOAPIC MMIO readable. Page-table walk / scratch PT approaches were tried
  and discarded (the kernel has no leaf-PT frames in identity mapping; the walk worked but
  the frames were I/O output).
- ACPI: RSDP first from bootloader data (checksum 20; fallback `find_root`: last 64KB of
  Usable regions in 16-byte steps - a full region scan loops forever on BIOS). XSDT
  (rev>=2) or RSDT; `Acpi` struct + `find(sig)`.
- MADT parsing: records start at a **fixed 0x2C offset**; the `Length` field is the total
  table size (not the header). ISO records provide IRQ->GSI+flags (QEMU pc: IRQ0->GSI2).
- IOAPIC: only PIT(32) and keyboard(33) pins are unmasked; the rest are masked. The PIC is
  fully masked (`pic_mask_all`, 0x21/0xA1=0xFF).
- **LAPIC EOI inside the handler is unconditional and lock-free**: written to the fixed
  FEE000B0 address. The first design (a `routed()` flag + `STATE.lock()` in the handler)
  was fatal: when the timer IRQ hit the init critical section, the handler waited on its
  own spinlock and the system silently hung (~50% boot failure). Rule: a handler's stack
  must never take a spinlock shared with init.

## Result

- BIOS (TCG, -smp 4): 2G/4CPU, prompt ~8s, key E2E works; UEFI/OVMF behaves the same (52s).
- `[ OK ] APIC: ... IRQ0 -> GSI2 (PIT), IRQ1 -> GSI1 (kbd); PIC masked`
- 1f (LAPIC timer calibration): `timer_init()` - masked-LVT 10/100ms PIT-measured calibration
  (wrap-protected), then periodic 1ms vector 32 + PIT GSI2 IOAPIC mask. Same vector/handler,
  TICKS unchanged; PIT fallback kept. BIOS ~1.16M ticks/ms, UEFI ~1.27M ticks/ms - varies
  with the TCG clock, calibration is mandatory.
