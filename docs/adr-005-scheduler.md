# ADR-005: Preemptive scheduler (Faz 2/2b)

**Tarih:** 2026-08-10 · **Durum:** Kabul edildi · **Bağlam:** TODO 2b

## Karar

- Round-robin preemptive scheduler (`kernel/src/sched.rs`): 1kHz LAPIC timer IRQ (vec 32)
  içinde `schedule()` — current task ready kuyruğunun sonuna, sıradaki Ready task seçilir,
  `context_switch` ile geçilir. Task stack'leri heap'te (64KB, `Box<[u8]>`); task 0 = kernel
  (boot yığını), asla uyumaz.
- `sleep_until(target)`: task `Sleeping(target)` + `sleepers` listesi; timer handler uyananları
  ready'ye alır; uyuyan task `enable_and_hlt()` ile bekler (busy-wait yok).
- **Kilit disiplini (kritik):** `SCHED`/`SERIAL1`/heap kilitleri daima `without_interrupts`
  içinde alınır (IRQ-reentrancy → spin deadlock); **hiçbir kilit `context_switch` boyunca
  tutulamaz** — `schedule()` hedefi seçip `drop(s)` eder, switch öyle yapılır. Kilidi
  switch boyunca tutmak: kilit kernel frame'inde asılı kalır, `sleep_until`'in kilidi sonsuz
  spin yapar.
- `context_switch` **`global_asm!`** ile (prologue'suz): normal Rust fn + `asm!` kullanımında
  derleyici debug build'de 0x18'lık arg-spill prologue'u ekliyordu; `saved_rsp` frame'i
  simetrik olsa da restore `ret`'i spill bölgesinden okuyordu (garaj adres → INSTRUCTION_FETCH
  PF). `global_asm!` ile frame tam simetrik: 6 callee-saved push → `mov [rdi], rsp` →
  `mov rsp, rsi` → 6 pop → `ret`. Yeni task frame'i spawn'da `[0×6, entry]` olarak kurulur.
- Kernel boot stack'i 1MB → 4MB (`BootloaderConfig.kernel_stack_size`): debug build'de boot
  (ps2/acpi/apic/disk/fat-ext4 probe) ~1MB yığına yaklaşıyordu; ilk preemption IRQ frame'i
  stack tabanının altına iniyor, saved_rsp garaj belleğe işaret ediyordu.
- `switches()` atomik sayaç (demo göstergesi); `MAX_TASKS=24`, spawn heap stack + frame.

## Sonuç

- BIOS + UEFI/OVMF E2E: `[blinky-a] 0s/1s/2s/...` her saniye (sleep+preempt), `[demo-count]`
  akıyor, kernel main switch'ler arasında sorunsuz devam ediyor (terminal prompt canlı),
  panic yok. Sabitlenen hatalar: lock-across-switch deadlock, `sleep_until` çift lock,
  context_switch prologue kayması, boot stack taşması.
