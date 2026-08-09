# ADR-004: Heap allocator (Faz 2/2a)

**Tarih:** 2026-08-09 · **Durum:** Kabul edildi · **Bağlam:** TODO 2a

## Karar

- `#[global_allocator]` first-fit free-list allocator (`kernel/src/heap.rs`), `spin::Mutex` içinde.
- 16MB heap, bootloader memory map'inin en büyük Usable bölgesinin **son 16MB'ından** ayrılır
  (üst bellek; çakışma riski en düşük). Pointer'lar fiziksel + PMO (0xFFFF_8000_0000_0000)
  penceresinde yaşar — sayfa tablosu dokunulmaz.
- Blok yapısı: her blok (dolu/boş) `{size, next}` header taşır; dealloc pointer'ı
  `ptr - align_up(16, layout.align())` ile deterministik bulur (layout aynı fonksiyondan).
- Split'te kalıntı adres 16-byte'a yuvarlanır (ilk sürümde rest yalnızca `cur+total` idi —
  String gibi tek boyutlu isteklerde 16-mult olmayan adreslere 8-byte deref → misaligned panik).
- `realloc`: alloc+copy+dealloc fallback (sadece Vec büyümelerinde görülür).
- Kademeli geçiş: boot selftest (+`history` komutu, Vec<String>) ile kanıtlandı; statik
  buffer'lar (fs/ext4, ata) ileriki işlerde birebir taşınır.

## Sonuç

- BIOS/UEFI boot: `[ OK ] Heap: 16 MB @ phys 0x7efe0000 ...` + selftest sum 6290432 (ok).
- `history` komutu heap üzerinde çalışır; PANIC durumu (misaligned) düzeltildi.