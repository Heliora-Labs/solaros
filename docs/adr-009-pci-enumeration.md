# ADR-009: PCI enumeration - legacy PIO config space (Phase 3a)

**Date:** 2026-08-10 · **Status:** Accepted · **Context:** TODO 3a

## Decision

- **Config space via the legacy PIO mechanism** (0xCF8/0xCFC): sufficient and simplest for
  QEMU `-machine pc` (PIIX3, bus 0 + optional PCI-PCI bridges); on real hardware bus 0 (and
  bridges via legacy routing) is also readable through PIO. ECAM (MCFG, ACPI) is
  deliberately deferred - it will be added when real machines + multi-root-port
  topologies (1c/3d) come up; the module will plug it in behind `config_read_*` then.
- **Enumeration**: start at bus 0; for each device/function read the vendor (0xFFFF =
  empty slot -> break the function loop), check the multifunction bit (header type bit 7)
  at function 0. For type-1 headers (bridges) read 0x19 (secondary bus) and enqueue it for
  scanning - topologies behind bridges are found too. Results live in a `Vec<PciDevice>`
  (spin::Mutex, MAX_DEVICES=64 cap).
- **Per-device reads**: vendor/device/class/subclass/progif/header type/IRQ line (0x3C) +
  secondary bus on bridges. Class/subclass -> human-readable names (e.g. `storage/IDE`,
  `bridge/Host`, `display/VGA`, `serialbus/USB`, `storage/NVMe`); a known-QEMU-device
  table (8086:1237 PIIX3 host, 8086:7000 PIIX3 ISA, 8086:7010 PIIX3 IDE, 1234:1111 QEMU
  VGA, 8086:100e e1000, 1AF4:* VirtIO...) provides names.
- **Surface**: `[ OK ] PCI: N devices found` at boot + one `[ OK ]` line per device; the
  `pci` command (from the shell via the exec bridge) prints the full list. Added to the
  COMMANDS table and the user help list.
- **Why PIO and not MMIO/ECAM**: `-machine pc` has no MCFG; PIO gives identical results
  on all QEMU targets (BIOS + OVMF) with the smallest code. Adding ECAM later stays
  inside `config_read_u32`.

## Result

- BIOS + UEFI/OVMF E2E: both find the same 5 devices: 00:00.0 8086:1237 host bridge,
  00:01.0 8086:7000 ISA, 00:01.1 8086:7010 IDE, 00:02.0 1234:1111 VGA, 00:03.0 8086:100e
  e1000. Boot lines and `pci` command output match; version/whoami regression clean; no
  panics.
- This list feeds 3b (AHCI) directly: when `-device ich9-ahci` is added to QEMU, the AHCI
  controller will appear in the same table as `storage/SATA` and the driver will start
  from its BARs.
