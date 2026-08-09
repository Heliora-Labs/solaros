use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;

use x86_64::instructions::interrupts::without_interrupts;

use crate::acpi::PMO;

const BLOCK_HEADER: usize = core::mem::size_of::<FreeBlock>();
const MIN_BLOCK: usize = 32;

struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

struct HeapInner {
    free: *mut FreeBlock,
    total: usize,
    used: usize,
    allocs: usize,
    frees: usize,
}

pub struct HeapAllocator {
    lock: Mutex<HeapInner>,
}

unsafe impl Send for HeapAllocator {}
unsafe impl Sync for HeapAllocator {}

#[global_allocator]
pub static ALLOCATOR: HeapAllocator = HeapAllocator::new();

fn align_up(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

impl HeapAllocator {
    pub const fn new() -> Self {
        Self {
            lock: Mutex::new(HeapInner {
                free: ptr::null_mut(),
                total: 0,
                used: 0,
                allocs: 0,
                frees: 0,
            }),
        }
    }

    pub fn init(&self, base_phys: usize, size: usize) {
        let mut inner = self.lock.lock();
        let base = align_up(base_phys, 16) + PMO as usize;
        let usable = size - (base - (base_phys + PMO as usize));
        let block = base as *mut FreeBlock;
        unsafe {
            (*block).size = usable - BLOCK_HEADER;
            (*block).next = ptr::null_mut();
        }
        inner.free = block;
        inner.total = usable;
        inner.used = 0;
        inner.allocs = 0;
        inner.frees = 0;
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let inner = self.lock.lock();
        (inner.total, inner.used, inner.allocs)
    }
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_inner(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            without_interrupts(|| {
                let mut inner = self.lock.lock();
                if ptr.is_null() {
                    return;
                }
                let off = align_up(BLOCK_HEADER, layout.align());
                let block = (ptr as usize - off) as *mut FreeBlock;
                let size = (*block).size;
                let head = inner.free;
                inner.free = block;
                (*block).next = head;
                inner.used = inner.used.saturating_sub(size);
                inner.frees += 1;
            });
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe {
            without_interrupts(|| {
                let new_ptr =
                    self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
                if new_ptr.is_null() {
                    return ptr::null_mut();
                }
                let copy = core::cmp::min(layout.size(), new_size);
                ptr::copy_nonoverlapping(ptr, new_ptr, copy);
                self.dealloc(ptr, layout);
                new_ptr
            })
        }
    }
}

impl HeapAllocator {
    fn alloc_inner(&self, layout: Layout) -> *mut u8 {
        without_interrupts(|| self.alloc_locked(layout))
    }

    fn alloc_locked(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let mut inner = self.lock.lock();
            let off = align_up(BLOCK_HEADER, layout.align());
            let total = off + layout.size();
            if inner.free.is_null() {
                return ptr::null_mut();
            }
            let mut prev: *mut FreeBlock = ptr::null_mut();
            let mut cur: *mut FreeBlock = inner.free;
            while !cur.is_null() {
                let b = &mut *cur;
                if b.size >= total {
                    let end = cur as usize + b.size;
                    let rest_addr = align_up(cur as usize + total, 16);
                    if rest_addr < end && end - rest_addr >= MIN_BLOCK {
                        let rest = rest_addr as *mut FreeBlock;
                        (*rest).size = end - rest_addr;
                        (*rest).next = b.next;
                        b.size = rest_addr - cur as usize;
                        b.next = ptr::null_mut();
                        if prev.is_null() {
                            inner.free = rest;
                        } else {
                            (*prev).next = rest;
                        }
                    } else {
                        if prev.is_null() {
                            inner.free = b.next;
                        } else {
                            (*prev).next = b.next;
                        }
                        b.next = ptr::null_mut();
                    }
                    inner.used += total;
                    inner.allocs += 1;
                    return (cur as usize + off) as *mut u8;
                }
                prev = cur;
                cur = b.next;
            }
            ptr::null_mut()
        }
    }
}

pub fn selftest() {
    use alloc::string::String;
    use alloc::vec::Vec;

    let mut v: Vec<u64> = Vec::new();
    for i in 0..2048u64 {
        v.push(i * 3 + 1);
    }
    let sum: u64 = v.iter().sum();
    let expect = 2048u64 * 6143 / 2;
    let mut s = String::new();
    s.push_str("heap-ok");
    s.push('!');
    let (total, used, allocs) = ALLOCATOR.stats();
    crate::boot::ok(format_args!(
        "Heap selftest: Vec<u64> x2048 sum {} ({}) String '{}' - {}/{} KB used, {} allocs",
        sum,
        if sum == expect { "ok" } else { "BAD" },
        s,
        used / 1024,
        total / 1024,
        allocs
    ));
}

pub fn init(boot_info: &'static bootloader_api::BootInfo) -> bool {
    const HEAP_MB: u64 = 16;
    const HEAP_BYTES: u64 = HEAP_MB * 1024 * 1024;

    let mut best_start: u64 = 0;
    let mut best_len: u64 = 0;
    for region in boot_info.memory_regions.iter() {
        if region.kind == bootloader_api::info::MemoryRegionKind::Usable {
            let len = region.end - region.start;
            if len > best_len {
                best_start = region.start;
                best_len = len;
            }
        }
    }
    if best_len < HEAP_BYTES {
        return false;
    }

    let heap_phys = (best_start + best_len - HEAP_BYTES) as usize;
    ALLOCATOR.init(heap_phys, HEAP_BYTES as usize);

    let (total, used, allocs) = ALLOCATOR.stats();
    crate::boot::ok(format_args!(
        "Heap: {} MB @ phys {:#x} (last {} MB of {} MB usable region) - {} used after init, {} allocs",
        total / 1024 / 1024,
        heap_phys,
        HEAP_MB,
        best_len / 1024 / 1024,
        used,
        allocs
    ));
    true
}