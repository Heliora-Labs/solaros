// ext4 driver (metadata_csum + 64bit + extents, no htree/flex_bg).
// JBD2-style journal: write-ahead redo log with commit/replay/checkpoint.

use spin::Mutex;

use crate::crc::crc32c;
use crate::fs::{self, FsErr};

pub const BLOCK: usize = 4096;
const SECTORS_PER_BLOCK: u32 = (BLOCK / 512) as u32;

// ---- on-disk constants ----
const MAGIC: u16 = 0xEF53;
const EXTENT_MAGIC: u16 = 0xF30A;

const INCOMPAT_FILETYPE: u32 = 0x0000_0002;
const INCOMPAT_EXTENTS: u32 = 0x0000_0040;
const INCOMPAT_64BIT: u32 = 0x0000_0080;
const INCOMPAT_META_CSUM: u32 = 0x0000_0400;
const INCOMPAT_SUPPORTED: u32 =
    INCOMPAT_FILETYPE | INCOMPAT_EXTENTS | INCOMPAT_64BIT | INCOMPAT_META_CSUM;

const RO_GDT_CSUM: u32 = 0x0000_0010;
const RO_EXTRA_ISIZE: u32 = 0x0000_0200;
const RO_SUPPORTED: u32 = RO_GDT_CSUM | RO_EXTRA_ISIZE;

const EXT4_EXTENTS_FL: u32 = 0x0008_0000;

const S_IFDIR: u16 = 0x4000;
const S_IFREG: u16 = 0x8000;
const MODE_DIR: u16 = S_IFDIR | 0o755;
const MODE_FILE: u16 = S_IFREG | 0o644;
const MODE_JOURNAL: u16 = S_IFREG | 0o600;

const DIR_FT_FILE: u8 = 1;
const DIR_FT_DIR: u8 = 2;

const INODE_ROOT: u32 = 2;
const INODE_JOURNAL: u32 = 8;
const FIRST_INO: u32 = 11;

const INODE_SIZE: u32 = 256;
const EXTRA_ISIZE: u16 = 32;
const DESC_SIZE: u32 = 64;
const BPG: u32 = 32768;
const IPG: u32 = 8192;
const ITABLE_BLOCKS: u32 = IPG * INODE_SIZE / BLOCK as u32;
const META_BLOCKS: u32 = 5 + ITABLE_BLOCKS; // boot/sb + bgd + bmap + imap + itable
// LBA28 ceiling: 2^28 sectors / 8 sectors-per-block / 32768 blocks-per-group
const MAX_GROUPS: u32 = 1024;

// ---- JBD2 journal (write-ahead redo log) ----
// Journal area: 1024 blocks starting at the journal inode's first extent.
//   slot 0            : journal superblock (magic, blocktype=1, seq, start)
//   slots 1..1023     : ring of transactions.
// A transaction on disk:
//   descriptor block  : magic(0) type=2(4) seq(8) nblocks(12) + nblocks u32 block numbers (16..)
//   nblocks data      : copies of the new block contents
//   commit block      : magic(0) type=3(4) seq(8) chksum(12) io_blocks(16)
// The checksum (crc32c) covers the descriptor block and all data blocks.
// Commit order: log data (write-ahead, at write time) -> descriptor -> commit
// block -> apply blocks to final locations -> advance journal superblock.
// Every block write in a txn is deferred to the log; the real write happens
// at commit (redo), so a crash before the commit block is ignored at replay.
const JOURNAL_BLOCKS: u32 = 1024;
const JBD_MAGIC: u32 = 0xC03B_3998;
const JBD_BLOCK_DESC: u32 = 2;
const JBD_BLOCK_COMMIT: u32 = 3;
// max block numbers that fit one 4K descriptor block: (4096 - 16) / 4
const MAX_PEND: usize = 1020;

const UUID: [u8; 16] = *b"SOLAROS-EXT4-261";

#[derive(Clone, Copy)]
struct JbdPend {
    blk: u32, // final destination block
    pos: u32, // journal-relative slot where the copy was logged
}

struct JbdState {
    base: u32, // physical block where the journal area starts
    start: u32, // journal-relative slot of the next descriptor (>= 1)
    seq: u32, // sequence number of the next transaction
    active: bool,
    pend: [JbdPend; MAX_PEND],
    pend_len: usize,
}

static JBD: Mutex<Option<JbdState>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct Ext4Info {
    groups: u32,
    total_blocks: u32,
    has_csum: bool,
}

static INFO: Mutex<Option<Ext4Info>> = Mutex::new(None);

fn info() -> Result<Ext4Info, FsErr> {
    INFO.lock().ok_or(FsErr::NotFormatted)
}

pub fn groups() -> u32 {
    INFO.lock().map(|i| i.groups).unwrap_or(0)
}

// ---------- block I/O ----------
// read_block/write_block are journal-aware (write-ahead redo log, see JBD
// section below); read_block_raw/write_block_raw are the raw disk paths.

fn read_block_raw(blk: u32, buf: &mut [u8; BLOCK]) -> bool {
    for i in 0..SECTORS_PER_BLOCK {
        let lba = blk * SECTORS_PER_BLOCK + i;
        let off = i as usize * 512;
        let sec: &mut [u8; 512] = (&mut buf[off..off + 512]).try_into().unwrap();
        if !fs::raw_read(lba, sec) {
            crate::println!("[DBG] read FAIL blk={} lba={}", blk, lba);
            return false;
        }
    }
    true
}

fn write_block_raw(blk: u32, buf: &[u8; BLOCK]) -> bool {
    for i in 0..SECTORS_PER_BLOCK {
        let lba = blk * SECTORS_PER_BLOCK + i;
        let off = i as usize * 512;
        let sec: &[u8; 512] = (&buf[off..off + 512]).try_into().unwrap();
        if !fs::raw_write(lba, sec) {
            crate::println!("[DBG] write FAIL blk={} lba={}", blk, lba);
            return false;
        }
    }
    true
}

fn is_jbd_block(base: u32, blk: u32) -> bool {
    blk >= base && blk < base + JOURNAL_BLOCKS
}

fn read_block(blk: u32, buf: &mut [u8; BLOCK]) -> bool {
    if let Some(j) = JBD.lock().as_ref() {
        if j.active {
            for k in 0..j.pend_len {
                if j.pend[k].blk == blk {
                    return read_block_raw(j.base + j.pend[k].pos, buf);
                }
            }
        }
    }
    read_block_raw(blk, buf)
}

fn write_block(blk: u32, buf: &[u8; BLOCK]) -> bool {
    if let Some(j) = JBD.lock().as_mut() {
        if j.active && !is_jbd_block(j.base, blk) {
            // a block may be written several times within one txn; re-log it
            // into the same slot so later reads see the newest copy
            for k in 0..j.pend_len {
                if j.pend[k].blk == blk {
                    return write_block_raw(j.base + j.pend[k].pos, buf);
                }
            }
            // The whole transaction (descriptor + n data blocks + commit block)
            // must fit in ring slots [1..JOURNAL_BLOCKS-1] from the current
            // start. If the next block would push the commit block past the
            // last ring slot, checkpoint what we have first (splits a very
            // large command into multiple atomic txns) or -- when the pend
            // array is empty -- wrap cleanly to the start of the ring.
            if j.pend_len == 0 {
                if j.start + 2 > JOURNAL_BLOCKS - 1 {
                    j.start = 1;
                }
            } else if (j.pend_len == MAX_PEND
                || j.start + j.pend_len as u32 + 2 > JOURNAL_BLOCKS - 1)
                && jbd_commit_locked(j).is_err()
            {
                return false;
            }
            let pos = j.start + 1 + j.pend_len as u32;
            if !write_block_raw(j.base + pos, buf) {
                return false;
            }
            j.pend[j.pend_len] = JbdPend { blk, pos };
            j.pend_len += 1;
            return true;
        }
    }
    write_block_raw(blk, buf)
}

// ---------- little-endian helpers ----------

fn r16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn r32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn w16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn w32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ---------- superblock (lives at byte 1024 of block 0) ----------

fn sb_off() -> usize {
    1024
}

fn sb_read() -> Result<[u8; 1024], FsErr> {
    let mut b = [0u8; BLOCK];
    if !read_block(0, &mut b) {
        return Err(FsErr::Io);
    }
    let mut sb = [0u8; 1024];
    sb.copy_from_slice(&b[sb_off()..sb_off() + 1024]);
    Ok(sb)
}

fn sb_write(sb: &mut [u8; 1024]) -> Result<(), FsErr> {
    if info()?.has_csum {
        let saved = r32(sb, 628);
        sb[628..632].fill(0);
        let c = crc32c(!0, &*sb);
        sb[628..632].copy_from_slice(&c.to_le_bytes());
        let _ = saved;
    }
    let mut b = [0u8; BLOCK];
    if !read_block(0, &mut b) {
        return Err(FsErr::Io);
    }
    b[sb_off()..sb_off() + 1024].copy_from_slice(sb);
    if !write_block(0, &b) {
        return Err(FsErr::Io);
    }
    Ok(())
}

fn sb_update(f: impl FnOnce(&mut [u8; 1024])) -> Result<(), FsErr> {
    let mut sb = sb_read()?;
    f(&mut sb);
    sb_write(&mut sb)
}

fn sb_free_blocks(sb: &[u8; 1024]) -> u64 {
    (r32(sb, 12) as u64) | ((r32(sb, 340) as u64) << 32)
}

// ---------- group descriptors ----------

fn desc_block_of(g: u32) -> u32 {
    1 + g * DESC_SIZE / BLOCK as u32
}

fn desc_off_of(g: u32) -> usize {
    (g * DESC_SIZE % BLOCK as u32) as usize
}

fn desc_read(g: u32, d: &mut [u8; 64]) -> Result<(), FsErr> {
    let mut b = [0u8; BLOCK];
    if !read_block(desc_block_of(g), &mut b) {
        return Err(FsErr::Io);
    }
    let o = desc_off_of(g);
    d.copy_from_slice(&b[o..o + 64]);
    Ok(())
}

fn desc_csum(g: u32, d: &[u8; 64]) -> u16 {
    let mut seed = crc32c(!0, &UUID);
    seed = crc32c(seed, &g.to_le_bytes());
    let mut tmp = [0u8; 64];
    tmp.copy_from_slice(d);
    tmp[30..32].fill(0);
    (crc32c(seed, &tmp) & 0xFFFF) as u16
}

fn desc_write(g: u32, d: &mut [u8; 64]) -> Result<(), FsErr> {
    let c = desc_csum(g, d);
    w16(d, 30, c);
    let mut b = [0u8; BLOCK];
    if !read_block(desc_block_of(g), &mut b) {
        return Err(FsErr::Io);
    }
    let o = desc_off_of(g);
    b[o..o + 64].copy_from_slice(d);
    if !write_block(desc_block_of(g), &b) {
        return Err(FsErr::Io);
    }
    Ok(())
}

fn desc_bmap_block(d: &[u8; 64]) -> u64 {
    r32(d, 0) as u64 | ((r32(d, 32) as u64) << 32)
}

fn desc_imap_block(d: &[u8; 64]) -> u64 {
    r32(d, 4) as u64 | ((r32(d, 36) as u64) << 32)
}

fn desc_itable_block(d: &[u8; 64]) -> u64 {
    r32(d, 8) as u64 | ((r32(d, 40) as u64) << 32)
}

fn desc_free_blocks(d: &[u8; 64]) -> u64 {
    r16(d, 12) as u64 | ((r16(d, 44) as u64) << 16)
}

fn desc_set_free_blocks(d: &mut [u8; 64], v: u64) {
    w16(d, 12, (v & 0xFFFF) as u16);
    w16(d, 44, ((v >> 16) & 0xFFFF) as u16);
}

fn desc_free_inodes(d: &[u8; 64]) -> u64 {
    r16(d, 14) as u64 | ((r16(d, 46) as u64) << 16)
}

fn desc_set_free_inodes(d: &mut [u8; 64], v: u64) {
    w16(d, 14, (v & 0xFFFF) as u16);
    w16(d, 46, ((v >> 16) & 0xFFFF) as u16);
}

fn desc_used_dirs(d: &[u8; 64]) -> u64 {
    r16(d, 16) as u64 | ((r16(d, 48) as u64) << 16)
}

fn desc_set_used_dirs(d: &mut [u8; 64], v: u64) {
    w16(d, 16, (v & 0xFFFF) as u16);
    w16(d, 48, ((v >> 16) & 0xFFFF) as u16);
}

fn bitmap_csum(data: &[u8; BLOCK]) -> u16 {
    let seed = crc32c(!0, &UUID);
    (crc32c(seed, data) & 0xFFFF) as u16
}

fn set_bitmap_csums(d: &mut [u8; 64], bmap: &[u8; BLOCK], imap: &[u8; BLOCK]) {
    let bc = bitmap_csum(bmap);
    w16(d, 24, bc);
    let ic = bitmap_csum(imap);
    w16(d, 26, ic);
    d[56..60].fill(0);
}

// ---------- inodes ----------

struct Inode {
    raw: [u8; 256],
}

impl Inode {
    fn load(ino: u32) -> Result<Inode, FsErr> {
        let i = info()?;
        let g = (ino - 1) / IPG;
        let idx = (ino - 1) % IPG;
        let mut d = [0u8; 64];
        desc_read(g, &mut d)?;
        let tbl = desc_itable_block(&d);
        let blk = tbl + (idx * INODE_SIZE / BLOCK as u32) as u64;
        let off = (idx * INODE_SIZE % BLOCK as u32) as usize;
        let mut b = [0u8; BLOCK];
        if !read_block(blk as u32, &mut b) {
            return Err(FsErr::Io);
        }
        let mut raw = [0u8; 256];
        raw.copy_from_slice(&b[off..off + 256]);
        if i.has_csum {
            let extra = r16(&raw, 128);
            if extra > 0 {
                let want = r16(&raw, 124) as u32 | ((r16(&raw, 130) as u32) << 16);
                let got = inode_csum(ino, &raw, extra);
                if want != got {
                    return Err(FsErr::Io);
                }
            }
        }
        Ok(Inode { raw })
    }

    fn save(&self, ino: u32) -> Result<(), FsErr> {
        let i = info()?;
        let g = (ino - 1) / IPG;
        let idx = (ino - 1) % IPG;
        let mut d = [0u8; 64];
        desc_read(g, &mut d)?;
        let tbl = desc_itable_block(&d);
        let blk = tbl + (idx * INODE_SIZE / BLOCK as u32) as u64;
        let off = (idx * INODE_SIZE % BLOCK as u32) as usize;
        let mut b = [0u8; BLOCK];
        if !read_block(blk as u32, &mut b) {
            return Err(FsErr::Io);
        }
        let mut raw = self.raw;
        if i.has_csum {
            let extra = r16(&raw, 128);
            if extra > 0 {
                let c = inode_csum(ino, &raw, extra);
                w16(&mut raw, 124, (c & 0xFFFF) as u16);
                w16(&mut raw, 130, ((c >> 16) & 0xFFFF) as u16);
            }
        }
        b[off..off + 256].copy_from_slice(&raw);
        if !write_block(blk as u32, &b) {
            return Err(FsErr::Io);
        }
        Ok(())
    }
}

fn inode_csum(ino: u32, raw: &[u8; 256], extra_isize: u16) -> u32 {
    let mut seed = crc32c(!0, &UUID);
    seed = crc32c(seed, &ino.to_le_bytes());
    let mut tmp = [0u8; 256];
    tmp.copy_from_slice(raw);
    tmp[124..126].fill(0);
    tmp[130..132].fill(0);
    let len = 128 + extra_isize as usize;
    crc32c(seed, &tmp[..len])
}

fn ino_mode(raw: &[u8; 256]) -> u16 {
    r16(raw, 0)
}

fn ino_links(raw: &[u8; 256]) -> u16 {
    r16(raw, 26)
}

fn ino_size(raw: &[u8; 256]) -> u64 {
    r32(raw, 4) as u64 | ((r32(raw, 108) as u64) << 32)
}

fn ino_blocks(raw: &[u8; 256]) -> u32 {
    r32(raw, 28)
}

fn ino_extents(raw: &[u8; 256]) -> [u8; 60] {
    let mut ib = [0u8; 60];
    ib.copy_from_slice(&raw[40..100]);
    ib
}

fn new_inode(mode: u16, links: u16) -> [u8; 256] {
    let mut raw = [0u8; 256];
    w16(&mut raw, 0, mode);
    w16(&mut raw, 2, 0); // uid
    w16(&mut raw, 24, 0); // gid
    w16(&mut raw, 26, links);
    w32(&mut raw, 32, EXT4_EXTENTS_FL);
    w32(&mut raw, 36, 1); // l_i_version
    w16(&mut raw, 128, EXTRA_ISIZE);
    w16(&mut raw, 132, 1); // ctime_extra (nanos 1)
    w16(&mut raw, 146, 0);
    w32(&mut raw, 100, 0); // generation
    w32(&mut raw, 152, 0); // version_hi
    raw
}

// ---------- extents ----------

// maps logical block -> physical block (depth 0 and 1)
fn extent_map(hdr: &[u8], logical: u32) -> Option<u32> {
    let mut buf: [u8; BLOCK] = [0u8; BLOCK];
    let mut data: &[u8] = hdr;
    loop {
        let magic = r16(data, 0);
        if magic != EXTENT_MAGIC {
            return None;
        }
        let entries = r16(data, 2);
        let depth = r16(data, 6);
        if depth == 0 {
            for e in 0..entries {
                let o = 12 + e as usize * 12;
                let eblk = r32(data, o);
                let elen = (r16(data, o + 4) & 0x7FFF) as u32;
                let estart = r32(data, o + 8) | ((r32(data, o + 6) as u32) << 16);
                if logical >= eblk && logical < eblk + elen {
                    return Some(estart + (logical - eblk));
                }
            }
            return None;
        }
        // depth > 0: descend through index entries
        let mut leaf = None;
        for e in 0..entries {
            let o = 12 + e as usize * 12;
            let eblk = r32(data, o);
            if eblk <= logical {
                leaf = Some((r32(data, o + 4) as u64) | ((r16(data, o + 6) as u64) << 32));
            }
        }
        let Some(l) = leaf else { return None };
        if !read_block(l as u32, &mut buf) {
            return None;
        }
        data = &buf;
    }
}

// append a physical block at the current end of the file's extent tree
fn extent_append(raw: &mut [u8; 256], phys: u32) -> Result<(), FsErr> {
    let mut ib = ino_extents(raw);
    if r16(&ib, 0) != EXTENT_MAGIC {
        ib.fill(0);
        w16(&mut ib, 0, EXTENT_MAGIC);
        w16(&mut ib, 2, 0);
        w16(&mut ib, 4, 4);
        w16(&mut ib, 6, 0);
    }
    let depth = r16(&ib, 6);
    if depth != 0 {
        return Err(FsErr::Unsupported);
    }
    let entries = r16(&ib, 2);
    let last_logical = if entries > 0 {
        let o = 12 + (entries - 1) as usize * 12;
        let eblk = r32(&ib, o);
        let elen = (r16(&ib, o + 4) & 0x7FFF) as u32;
        eblk + elen
    } else {
        0
    };
    if entries > 0 {
        let o = 12 + (entries - 1) as usize * 12;
        let eblk = r32(&ib, o);
        let elen = r16(&ib, o + 4) & 0x7FFF;
        let estart = r32(&ib, o + 8) | ((r32(&ib, o + 6) as u32) << 16);
        if eblk + elen as u32 == last_logical && estart + elen as u32 == phys && elen < 0x7FFF {
            w16(&mut ib, o + 4, elen + 1);
            raw[40..100].copy_from_slice(&ib);
            return Ok(());
        }
    }
    if entries >= 4 {
        return Err(FsErr::Unsupported);
    }
    let o = 12 + entries as usize * 12;
    w32(&mut ib, o, last_logical);
    w16(&mut ib, o + 4, 1);
    w16(&mut ib, o + 6, (phys >> 16) as u16);
    w32(&mut ib, o + 8, phys);
    w16(&mut ib, 2, entries + 1);
    raw[40..100].copy_from_slice(&ib);
    Ok(())
}

// free all physical blocks referenced by a file's extent tree
fn free_extents(raw: &[u8; 256]) -> Result<(), FsErr> {
    let ib = ino_extents(raw);
    if r16(&ib, 0) != EXTENT_MAGIC {
        return Ok(());
    }
    let depth = r16(&ib, 6);
    if depth != 0 {
        return Err(FsErr::Unsupported);
    }
    let entries = r16(&ib, 2);
    for e in 0..entries {
        let o = 12 + e as usize * 12;
        let elen = (r16(&ib, o + 4) & 0x7FFF) as u32;
        let estart = r32(&ib, o + 8) | ((r32(&ib, o + 6) as u32) << 16);
        for k in 0..elen {
            free_block(estart + k)?;
        }
    }
    Ok(())
}

// ---------- allocation ----------

fn update_sb_free_blocks(delta: i64) -> Result<(), FsErr> {
    sb_update(|sb| {
        let cur = sb_free_blocks(sb) as i64 + delta;
        let cur = cur.max(0) as u64;
        w32(sb, 12, (cur & 0xFFFF_FFFF) as u32);
        w32(sb, 340, ((cur >> 32) & 0xFFFF_FFFF) as u32);
    })
}

fn alloc_block() -> Result<u32, FsErr> {
    let i = info()?;
    for g in 0..i.groups {
        let mut d = [0u8; 64];
        desc_read(g, &mut d)?;
        let bmap_blk = desc_bmap_block(&d) as u32;
        let mut bmap = [0u8; BLOCK];
        if !read_block(bmap_blk, &mut bmap) {
            return Err(FsErr::Io);
        }
        let g_start = g * BPG;
        let g_blocks = (i.total_blocks - g_start).min(BPG);
        for bit in 0..g_blocks {
            let b = bit as usize;
            if bmap[b / 8] & (1 << (b % 8)) == 0 {
                bmap[b / 8] |= 1 << (b % 8);
                if !write_block(bmap_blk, &bmap) {
                    return Err(FsErr::Io);
                }
                let mut imap = [0u8; BLOCK];
                if !read_block(desc_imap_block(&d) as u32, &mut imap) {
                    return Err(FsErr::Io);
                }
                set_bitmap_csums(&mut d, &bmap, &imap);
                let cur = desc_free_blocks(&d);
                if cur == 0 {
                    return Err(FsErr::NoSpace);
                }
                let cur = cur - 1;
                desc_set_free_blocks(&mut d, cur);
                desc_write(g, &mut d)?;
                update_sb_free_blocks(-1)?;
                return Ok(g_start + bit);
            }
        }
    }
    Err(FsErr::NoSpace)
}

fn free_block(blk: u32) -> Result<(), FsErr> {
    let g = blk / BPG;
    let bit = (blk % BPG) as usize;
    let mut d = [0u8; 64];
    desc_read(g, &mut d)?;
    let bmap_blk = desc_bmap_block(&d) as u32;
    let mut bmap = [0u8; BLOCK];
    if !read_block(bmap_blk, &mut bmap) {
        return Err(FsErr::Io);
    }
    bmap[bit / 8] &= !(1 << (bit % 8));
    if !write_block(bmap_blk, &bmap) {
        return Err(FsErr::Io);
    }
    let mut imap = [0u8; BLOCK];
    if !read_block(desc_imap_block(&d) as u32, &mut imap) {
        return Err(FsErr::Io);
    }
    set_bitmap_csums(&mut d, &bmap, &imap);
    let cur = desc_free_blocks(&d) + 1;
    desc_set_free_blocks(&mut d, cur);
    desc_write(g, &mut d)?;
    update_sb_free_blocks(1)
}

fn alloc_inode() -> Result<u32, FsErr> {
    let i = info()?;
    for g in 0..i.groups {
        let mut d = [0u8; 64];
        desc_read(g, &mut d)?;
        let imap_blk = desc_imap_block(&d) as u32;
        let mut imap = [0u8; BLOCK];
        if !read_block(imap_blk, &mut imap) {
            return Err(FsErr::Io);
        }
        for bit in 0..IPG {
            let b = bit as usize;
            if imap[b / 8] & (1 << (b % 8)) == 0 {
                imap[b / 8] |= 1 << (b % 8);
                if !write_block(imap_blk, &imap) {
                    return Err(FsErr::Io);
                }
                let mut bmap = [0u8; BLOCK];
                if !read_block(desc_bmap_block(&d) as u32, &mut bmap) {
                    return Err(FsErr::Io);
                }
                set_bitmap_csums(&mut d, &bmap, &imap);
                let cur = desc_free_inodes(&d) - 1;
                desc_set_free_inodes(&mut d, cur);
                desc_write(g, &mut d)?;
                sb_update(|sb| {
                    let cur = r32(sb, 16) - 1;
                    w32(sb, 16, cur);
                })?;
                return Ok(g * IPG + bit + 1);
            }
        }
    }
    Err(FsErr::NoSpace)
}

fn free_inode(ino: u32) -> Result<(), FsErr> {
    let g = (ino - 1) / IPG;
    let bit = ((ino - 1) % IPG) as usize;
    let mut d = [0u8; 64];
    desc_read(g, &mut d)?;
    let imap_blk = desc_imap_block(&d) as u32;
    let mut imap = [0u8; BLOCK];
    if !read_block(imap_blk, &mut imap) {
        return Err(FsErr::Io);
    }
    imap[bit / 8] &= !(1 << (bit % 8));
    if !write_block(imap_blk, &imap) {
        return Err(FsErr::Io);
    }
    let mut bmap = [0u8; BLOCK];
    if !read_block(desc_bmap_block(&d) as u32, &mut bmap) {
        return Err(FsErr::Io);
    }
    set_bitmap_csums(&mut d, &bmap, &imap);
    let cur = desc_free_inodes(&d) + 1;
    desc_set_free_inodes(&mut d, cur);
    desc_write(g, &mut d)?;
    sb_update(|sb| {
        let cur = r32(sb, 16) + 1;
        w32(sb, 16, cur);
    })
}

fn bump_used_dirs(g: u32, delta: i64) -> Result<(), FsErr> {
    let mut d = [0u8; 64];
    desc_read(g, &mut d)?;
    let cur = desc_used_dirs(&d) as i64 + delta;
    desc_set_used_dirs(&mut d, cur.max(0) as u64);
    desc_write(g, &mut d)
}

// ---------- directories ----------

struct DirEnt {
    ino: u32,
    ftype: u8,
    name: [u8; 255],
    name_len: u8,
}

// walk all entries of a directory; f returns true to stop early
fn walk_dir(dir_ino: u32, f: &mut dyn FnMut(&DirEnt) -> bool) -> Result<(), FsErr> {
    let inode = Inode::load(dir_ino)?;
    let raw = inode.raw;
    if ino_mode(&raw) & S_IFDIR == 0 {
        return Err(FsErr::NotDir);
    }
    let size = ino_size(&raw) as usize;
    if size == 0 {
        return Ok(());
    }
    let ib = ino_extents(&raw);
    let mut off = 0usize;
    while off < size {
        let logical = (off / BLOCK) as u32;
        let boff = off % BLOCK;
        let Some(phys) = extent_map(&ib, logical) else {
            return Err(FsErr::Io);
        };
        let mut b = [0u8; BLOCK];
        if !read_block(phys, &mut b) {
            return Err(FsErr::Io);
        }
        let mut o = boff;
        while o + 8 <= BLOCK {
            let ino = r32(&b, o);
            let rec = r16(&b, o + 4) as usize;
            if rec == 0 || o + rec > BLOCK {
                break;
            }
            if ino != 0 {
                let nl = b[o + 6] as usize;
                if nl > 255 || o + 8 + nl > BLOCK {
                    return Err(FsErr::Io);
                }
                let mut e = DirEnt {
                    ino,
                    ftype: b[o + 7],
                    name: [0; 255],
                    name_len: nl as u8,
                };
                e.name[..nl].copy_from_slice(&b[o + 8..o + 8 + nl]);
                if f(&e) {
                    return Ok(());
                }
            }
            o += rec;
        }
        off += BLOCK - boff;
    }
    Ok(())
}

fn lookup_dir(dir_ino: u32, name: &[u8]) -> Result<(u32, u8), FsErr> {
    let mut found = None;
    walk_dir(dir_ino, &mut |e| {
        if e.name_len as usize == name.len() && &e.name[..e.name_len as usize] == name {
            found = Some((e.ino, e.ftype));
            true
        } else {
            false
        }
    })?;
    found.ok_or(FsErr::NotFound)
}

fn name_bytes(comp: &[char]) -> Result<[u8; 255], FsErr> {
    let mut out = [0u8; 255];
    if comp.is_empty() {
        return Err(FsErr::BadName);
    }
    if comp.len() > 255 {
        return Err(FsErr::BadName);
    }
    for (k, &c) in comp.iter().enumerate() {
        if !c.is_ascii() || (c as u32) < 0x20 || c == '/' || c == '\0' {
            return Err(FsErr::BadName);
        }
        out[k] = c as u8;
    }
    Ok(out)
}

fn resolve(dir_ino: u32, path: &[char]) -> Result<u32, FsErr> {
    let mut ino = dir_ino;
    let mut rest = path;
    while let Some((comp, tail)) = pop_comp(rest) {
        if comp.is_empty() {
            rest = tail;
            continue;
        }
        let name = name_bytes(comp)?;
        let (i, ft) = lookup_dir(ino, &name[..comp.len()])?;
        if ft != DIR_FT_DIR {
            return Err(FsErr::NotDir);
        }
        ino = i;
        rest = tail;
    }
    Ok(ino)
}

fn cwd_ino() -> Result<u32, FsErr> {
    let mut ino = INODE_ROOT;
    let cwd = fs::cwd();
    for k in 0..cwd.n {
        let name = &cwd.parts[k][..cwd.lens[k]];
        let (i, ft) = lookup_dir(ino, name)?;
        if ft != DIR_FT_DIR {
            return Err(FsErr::NotDir);
        }
        ino = i;
    }
    Ok(ino)
}

fn resolve_from_cwd(path: &[char]) -> Result<u32, FsErr> {
    if !path.is_empty() && path[0] == '/' {
        resolve(INODE_ROOT, path)
    } else {
        resolve(cwd_ino()?, path)
    }
}

pub(crate) fn check_dir(cwd: &fs::Cwd) -> Result<(), FsErr> {
    let mut ino = INODE_ROOT;
    for k in 0..cwd.n {
        let name = &cwd.parts[k][..cwd.lens[k]];
        let (i, ft) = lookup_dir(ino, name)?;
        if ft != DIR_FT_DIR {
            return Err(FsErr::NotDir);
        }
        ino = i;
    }
    Ok(())
}

fn pop_comp(p: &[char]) -> Option<(&[char], &[char])> {
    let mut p = p;
    while !p.is_empty() && p[0] == '/' {
        p = &p[1..];
    }
    if p.is_empty() {
        return None;
    }
    let mut s = 0;
    while s < p.len() && p[s] != '/' {
        s += 1;
    }
    Some((&p[..s], &p[s..]))
}

fn split_parent(path: &[char]) -> (&[char], &[char]) {
    let mut last = None;
    for (i, &c) in path.iter().enumerate() {
        if c == '/' {
            last = Some(i);
        }
    }
    match last {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (&[][..], path),
    }
}

// find a free slot for an entry of the given size in a directory;
// returns (phys_block, offset, available_bytes_at_offset)
fn dir_find_slot(dir_ino: u32, need: usize) -> Result<(u32, usize, usize), FsErr> {
    let inode = Inode::load(dir_ino)?;
    let raw = inode.raw;
    let size = ino_size(&raw) as usize;
    let ib = ino_extents(&raw);
    let mut off = 0usize;
    while off < size {
        let logical = (off / BLOCK) as u32;
        let boff = off % BLOCK;
        let Some(phys) = extent_map(&ib, logical) else {
            return Err(FsErr::Io);
        };
        let mut b = [0u8; BLOCK];
        if !read_block(phys, &mut b) {
            return Err(FsErr::Io);
        }
        let mut o = boff;
        while o + 8 <= BLOCK {
            let ino = r32(&b, o);
            let rec = r16(&b, o + 4) as usize;
            if ino == 0 && rec == 0 {
                let avail = BLOCK - o;
                if avail >= need {
                    return Ok((phys, o, avail));
                }
                break;
            }
            if rec == 0 || o + rec > BLOCK {
                break;
            }
            if ino == 0 && rec >= need {
                return Ok((phys, o, rec));
            }
            o += rec;
        }
        off += BLOCK - boff;
    }
    Err(FsErr::NoSpace)
}

// grow a directory by one block
fn dir_grow(dir_ino: u32) -> Result<(), FsErr> {
    let mut inode = Inode::load(dir_ino)?;
    let blk = alloc_block()?;
    let zero = [0u8; BLOCK];
    if !write_block(blk, &zero) {
        return Err(FsErr::Io);
    }
    extent_append(&mut inode.raw, blk)?;
    let size = ino_size(&inode.raw) + BLOCK as u64;
    w32(&mut inode.raw, 4, (size & 0xFFFF_FFFF) as u32);
    w32(&mut inode.raw, 108, ((size >> 32) & 0xFFFF_FFFF) as u32);
    let blocks = ino_blocks(&inode.raw) + 8;
    w32(&mut inode.raw, 28, blocks);
    inode.save(dir_ino)
}

fn dir_add(dir_ino: u32, name: &[u8], child: u32, ftype: u8) -> Result<(), FsErr> {
    let need = align4(8 + name.len());
    let (phys, off, avail) = match dir_find_slot(dir_ino, need) {
        Ok(slot) => slot,
        Err(FsErr::NoSpace) => {
            dir_grow(dir_ino)?;
            let s = dir_find_slot(dir_ino, need)?;
            s
        }
        Err(e) => return Err(e),
    };
    let mut b = [0u8; BLOCK];
    if !read_block(phys, &mut b) {
        return Err(FsErr::Io);
    }
    let rec = if avail - need >= 8 { need } else { avail };
    write_dir_entry(&mut b[off..off + rec], child, name, ftype, rec);
    if avail - need >= 8 {
        // leave the remainder as an unused entry
        let rest = avail - need;
        write_dir_entry(&mut b[off + need..off + need + rest], 0, &[], 0, rest);
    }
    if !write_block(phys, &b) {
        return Err(FsErr::Io);
    }
    Ok(())
}

fn write_dir_entry(area: &mut [u8], ino: u32, name: &[u8], ftype: u8, rec: usize) {
    w32(area, 0, ino);
    w16(area, 4, rec as u16);
    area[6] = name.len() as u8;
    area[7] = ftype;
    area[8..8 + name.len()].copy_from_slice(name);
}

fn align4(v: usize) -> usize {
    (v + 3) & !3
}

fn dir_del(dir_ino: u32, name: &[u8]) -> Result<(), FsErr> {
    let inode = Inode::load(dir_ino)?;
    let raw = inode.raw;
    let size = ino_size(&raw) as usize;
    let ib = ino_extents(&raw);
    let mut off = 0usize;
    while off < size {
        let logical = (off / BLOCK) as u32;
        let boff = off % BLOCK;
        let Some(phys) = extent_map(&ib, logical) else {
            return Err(FsErr::Io);
        };
        let mut b = [0u8; BLOCK];
        if !read_block(phys, &mut b) {
            return Err(FsErr::Io);
        }
        let mut o = boff;
        while o + 8 <= BLOCK {
            let ino = r32(&b, o);
            let rec = r16(&b, o + 4) as usize;
            if rec == 0 || o + rec > BLOCK {
                break;
            }
            if ino != 0 {
                let nl = b[o + 6] as usize;
                if nl == name.len() && &b[o + 8..o + 8 + nl] == name {
                    b[o..o + 4].fill(0);
                    if !write_block(phys, &b) {
                        return Err(FsErr::Io);
                    }
                    return Ok(());
                }
            }
            o += rec;
        }
        off += BLOCK - boff;
    }
    Err(FsErr::NotFound)
}

fn read_file_data(ino: u32, out: &mut [u8]) -> Result<usize, FsErr> {
    let inode = Inode::load(ino)?;
    let raw = inode.raw;
    if ino_mode(&raw) & S_IFREG == 0 {
        return Err(FsErr::IsDir);
    }
    let size = ino_size(&raw) as usize;
    let want = size.min(out.len());
    if want == 0 {
        return Ok(0);
    }
    let ib = ino_extents(&raw);
    let mut written = 0usize;
    while written < want {
        let logical = (written / BLOCK) as u32;
        let off = written % BLOCK;
        let n = (want - written).min(BLOCK - off);
        let Some(phys) = extent_map(&ib, logical) else {
            return Err(FsErr::Io);
        };
        let mut b = [0u8; BLOCK];
        if !read_block(phys, &mut b) {
            return Err(FsErr::Io);
        }
        out[written..written + n].copy_from_slice(&b[off..off + n]);
        written += n;
    }
    Ok(written)
}

// ---------- JBD2 journal: commit / replay / init ----------

fn jbd_write_super(j: &JbdState) -> Result<(), FsErr> {
    let mut sb = [0u8; BLOCK];
    if !read_block_raw(j.base, &mut sb) {
        return Err(FsErr::Io);
    }
    w32(&mut sb, 8, j.seq);
    w32(&mut sb, 12, j.start);
    if !write_block_raw(j.base, &sb) {
        return Err(FsErr::Io);
    }
    Ok(())
}

// checkpoint the pending transaction: descriptor -> commit block (seal) ->
// apply blocks to final locations -> advance the journal superblock.
fn jbd_commit_locked(j: &mut JbdState) -> Result<(), FsErr> {
    if !j.active || j.pend_len == 0 {
        return Ok(());
    }
    let n = j.pend_len;
    let start = if j.start == 0 { 1 } else { j.start };
    let mut d = [0u8; BLOCK];
    w32(&mut d, 0, JBD_MAGIC);
    w32(&mut d, 4, JBD_BLOCK_DESC);
    w32(&mut d, 8, j.seq);
    w32(&mut d, 12, n as u32);
    for k in 0..n {
        w32(&mut d, 16 + 4 * k, j.pend[k].blk);
    }
    let mut chk = crc32c(!0, &d);
    for k in 0..n {
        let mut db = [0u8; BLOCK];
        if !read_block_raw(j.base + start + 1 + k as u32, &mut db) {
            return Err(FsErr::Io);
        }
        chk = crc32c(chk, &db);
    }
    let mut c = [0u8; BLOCK];
    w32(&mut c, 0, JBD_MAGIC);
    w32(&mut c, 4, JBD_BLOCK_COMMIT);
    w32(&mut c, 8, j.seq);
    w32(&mut c, 12, chk);
    w32(&mut c, 16, n as u32);
    // 1. descriptor
    if !write_block_raw(j.base + start, &d) {
        return Err(FsErr::Io);
    }
    // 2. commit block (the seal)
    if !write_block_raw(j.base + start + 1 + n as u32, &c) {
        return Err(FsErr::Io);
    }
    // 3. redo: apply the logged blocks to their final locations
    for k in 0..n {
        let mut db = [0u8; BLOCK];
        if !read_block_raw(j.base + start + 1 + k as u32, &mut db) {
            return Err(FsErr::Io);
        }
        if !write_block_raw(j.pend[k].blk, &db) {
            return Err(FsErr::Io);
        }
    }
    // 4. advance: the transaction is checkpointed, its slots are reusable
    j.start = start + 2 + n as u32;
    if j.start > JOURNAL_BLOCKS - 1 {
        j.start = 1;
    }
    j.seq += 1;
    j.pend_len = 0;
    jbd_write_super(j)
}

// called at the end of every mutating filesystem command
pub fn jbd_commit() -> Result<(), FsErr> {
    let mut g = JBD.lock();
    if let Some(j) = g.as_mut() {
        return jbd_commit_locked(j);
    }
    Ok(())
}

// apply any committed-but-not-checkpointed transaction found in the log.
fn jbd_replay(j: &mut JbdState) -> Result<(), FsErr> {
    loop {
        let start = if j.start == 0 { 1 } else { j.start };
        if start + 2 > JOURNAL_BLOCKS {
            break;
        }
        let mut d = [0u8; BLOCK];
        if !read_block_raw(j.base + start, &mut d) {
            return Err(FsErr::Io);
        }
        if r32(&d, 0) != JBD_MAGIC || r32(&d, 4) != JBD_BLOCK_DESC || r32(&d, 8) != j.seq {
            break; // incomplete or stale transaction
        }
        let n = r32(&d, 12) as usize;
        if n > MAX_PEND || start + 2 + n as u32 > JOURNAL_BLOCKS {
            break; // descriptor damaged
        }
        let mut c = [0u8; BLOCK];
        if !read_block_raw(j.base + start + 1 + n as u32, &mut c) {
            return Err(FsErr::Io);
        }
        if r32(&c, 0) != JBD_MAGIC || r32(&c, 4) != JBD_BLOCK_COMMIT || r32(&c, 8) != j.seq {
            break; // crash before the commit block was written
        }
        let mut chk = crc32c(!0, &d);
        for k in 0..n {
            let mut db = [0u8; BLOCK];
            if !read_block_raw(j.base + start + 1 + k as u32, &mut db) {
                return Err(FsErr::Io);
            }
            chk = crc32c(chk, &db);
        }
        if r32(&c, 12) != chk {
            break; // torn or corrupt data blocks
        }
        for k in 0..n {
            let mut db = [0u8; BLOCK];
            if !read_block_raw(j.base + start + 1 + k as u32, &mut db) {
                return Err(FsErr::Io);
            }
            if !write_block_raw(r32(&d, 16 + 4 * k), &db) {
                return Err(FsErr::Io);
            }
        }
        j.start = start + 2 + n as u32;
        if j.start > JOURNAL_BLOCKS - 1 {
            j.start = 1;
        }
        j.seq += 1;
    }
    Ok(())
}

// locate the journal area (inode 8), read its superblock, replay pending
// transactions and arm write-ahead journaling. No journal -> plain passthrough.
fn jbd_init() -> Result<(), FsErr> {
    let ino = Inode::load(INODE_JOURNAL)?;
    let ib = ino_extents(&ino.raw);
    let Some(base) = extent_map(&ib, 0) else {
        return Ok(()); // journal inode absent: journaling disabled
    };
    let mut sb = [0u8; BLOCK];
    if !read_block_raw(base, &mut sb) {
        return Err(FsErr::Io);
    }
    if r32(&sb, 0) != JBD_MAGIC {
        return Ok(()); // not a journaled image: journaling disabled
    }
    let seq = r32(&sb, 8);
    let mut start = r32(&sb, 12);
    if start == 0 || start >= JOURNAL_BLOCKS {
        start = 1;
    }
    let mut j = JbdState {
        base,
        start,
        seq,
        active: true,
        pend: [JbdPend { blk: 0, pos: 0 }; MAX_PEND],
        pend_len: 0,
    };
    let before = j.seq;
    jbd_replay(&mut j)?;
    if j.seq != before {
        crate::println!(
            "[ OK ] Journal: {} transaction(s) replayed",
            j.seq - before
        );
    }
    *JBD.lock() = Some(j);
    Ok(())
}

// ---------- mount ----------

pub fn mount_at(base: u32) -> Result<(), FsErr> {
    fs::set_base(base);
    let mut b = [0u8; BLOCK];
    if !read_block(0, &mut b) {
        return Err(FsErr::Io);
    }
    let o = sb_off();
    if r16(&b, o + 56) != MAGIC {
        return Err(FsErr::NotFormatted);
    }
    let incompat = r32(&b, o + 96);
    let ro = r32(&b, o + 100);
    if incompat & !INCOMPAT_SUPPORTED != 0 {
        return Err(FsErr::Unsupported);
    }
    if ro & !RO_SUPPORTED != 0 {
        return Err(FsErr::Unsupported);
    }
    let has_csum = incompat & INCOMPAT_META_CSUM != 0;
    if has_csum {
        let saved = r32(&b, o + 628);
        b[o + 628..o + 632].fill(0);
        let c = crc32c(!0, &b[o..o + 1024]);
        if c != saved {
            return Err(FsErr::Io);
        }
    }
    let log_block = r32(&b, o + 24);
    if log_block != 2 {
        return Err(FsErr::Unsupported);
    }
    let blocks_lo = r32(&b, o + 4);
    let blocks_hi = r32(&b, o + 332);
    let total_blocks = blocks_lo as u64 | ((blocks_hi as u64) << 32);
    let bpg = r32(&b, o + 32);
    if bpg == 0 || bpg > BPG {
        return Err(FsErr::Unsupported);
    }
    let groups = ((total_blocks + bpg as u64 - 1) / bpg as u64) as u32;
    if groups == 0 || groups > MAX_GROUPS {
        return Err(FsErr::Unsupported);
    }
    *INFO.lock() = Some(Ext4Info {
        groups,
        total_blocks: total_blocks as u32,
        has_csum,
    });
    jbd_init()?;
    fs::mark_mounted(fs::FsKind::Ext4, base);
    Ok(())
}

// ---------- public commands ----------

pub fn ls(path: &[char]) -> Result<(), FsErr> {
    let dir = resolve_from_cwd(path)?;
    let inode = Inode::load(dir)?;
    if ino_mode(&inode.raw) & S_IFDIR == 0 {
        return Err(FsErr::NotDir);
    }
    match walk_dir(dir, &mut |e| {
        let mut disp = ['\0'; 64];
        let mut len = 0usize;
        for k in 0..e.name_len as usize {
            let c = e.name[k];
            if c >= 0x20 && c < 0x7F && len < 64 {
                disp[len] = c as char;
                len += 1;
            } else if len < 64 {
                disp[len] = '?';
                len += 1;
            }
        }
        let kind_s = if e.ftype == DIR_FT_DIR { "<DIR>" } else { "file" };
        let size = match Inode::load(e.ino) {
            Ok(ie) => ino_size(&ie.raw),
            Err(_) => 0,
        };
        crate::println!("{:<24} {:<6} {:>8}", crate::Utf8Chars(&disp[..len]), kind_s, size);
        false
    }) {
        Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn cat(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    let (ino, ft) = lookup_dir(p, &name[..leaf.len()])?;
    if ft != DIR_FT_FILE {
        return Err(FsErr::IsDir);
    }
    let mut buf = [0u8; 8192];
    let n = read_file_data(ino, &mut buf)?;
    for k in 0..n {
        let c = buf[k];
        if (0x20..0x7F).contains(&c) {
            crate::print!("{}", c as char);
        } else {
            crate::print!("?");
        }
    }
    Ok(())
}

pub fn read_file(path: &[char], out: &mut [u8]) -> Result<usize, FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    let (ino, ft) = lookup_dir(p, &name[..leaf.len()])?;
    if ft != DIR_FT_FILE {
        return Err(FsErr::IsDir);
    }
    read_file_data(ino, out)
}

pub fn touch(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    if lookup_dir(p, &name[..leaf.len()]).is_ok() {
        return Ok(());
    }
    let ino = alloc_inode()?;
    let raw = new_inode(MODE_FILE, 1);
    Inode { raw }.save(ino)?;
    dir_add(p, &name[..leaf.len()], ino, DIR_FT_FILE)
}

pub fn mkdir(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    if lookup_dir(p, &name[..leaf.len()]).is_ok() {
        return Err(FsErr::Exists);
    }
    let nino = alloc_inode()?;
    let dblk = alloc_block()?;
    let mut b = [0u8; BLOCK];
    // "." and ".." entries
    write_dir_entry(&mut b[0..12], nino, b".", DIR_FT_DIR, 12);
    write_dir_entry(&mut b[12..BLOCK], p, b"..", DIR_FT_DIR, BLOCK - 12);
    if !write_block(dblk, &b) {
        return Err(FsErr::Io);
    }
    let mut raw = new_inode(MODE_DIR, 2);
    w32(&mut raw, 4, BLOCK as u32);
    w32(&mut raw, 28, 8);
    let mut ib = [0u8; 60];
    w16(&mut ib, 0, EXTENT_MAGIC);
    w16(&mut ib, 2, 1);
    w16(&mut ib, 4, 4);
    w32(&mut ib, 12, 0);
    w16(&mut ib, 16, 1);
    w16(&mut ib, 18, (dblk >> 16) as u16);
    w32(&mut ib, 20, dblk);
    raw[40..100].copy_from_slice(&ib);
    Inode { raw }.save(nino)?;
    dir_add(p, &name[..leaf.len()], nino, DIR_FT_DIR)?;
    // parent link count +1
    let mut pin = Inode::load(p)?;
    let links = ino_links(&pin.raw) + 1;
    w16(&mut pin.raw, 26, links);
    pin.save(p)?;
    bump_used_dirs((nino - 1) / IPG, 1)
}

pub fn write_file(path: &[char], data: &[u8]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    let existing = lookup_dir(p, &name[..leaf.len()]).ok();
    let ino = match existing {
        Some((i, DIR_FT_FILE)) => i,
        Some(_) => return Err(FsErr::IsDir),
        None => alloc_inode()?,
    };
    if existing.is_none() {
        dir_add(p, &name[..leaf.len()], ino, DIR_FT_FILE)?;
    }
    let mut raw = if existing.is_none() {
        new_inode(MODE_FILE, 1)
    } else {
        let inode = Inode::load(ino)?;
        free_extents(&inode.raw)?;
        inode.raw
    };
    // the result must always be a regular file: the previous inode may have
    // been left behind by an aborted write (mode 0), so force it here
    w16(&mut raw, 0, MODE_FILE);
    // reset inode: size 0, no blocks, no extents
    w32(&mut raw, 4, 0);
    w32(&mut raw, 108, 0);
    w32(&mut raw, 28, 0);
    raw[40..100].fill(0);
    if data.is_empty() {
        w16(&mut raw, 26, 1);
        Inode { raw }.save(ino)?;
        return Ok(());
    }
    let nblocks = (data.len() + BLOCK - 1) / BLOCK;
    let mut written = 0usize;
    // save the (empty) inode up front so an aborted write leaves a valid
    // empty file instead of a dangling zeroed inode
    w16(&mut raw, 26, 1);
    Inode { raw }.save(ino)?;
    let res = (|| -> Result<(), FsErr> {
        for _ in 0..nblocks {
            let n = (data.len() - written).min(BLOCK);
            let blk = alloc_block()?;
            let mut b = [0u8; BLOCK];
            b[..n].copy_from_slice(&data[written..written + n]);
            if !write_block(blk, &b) {
                return Err(FsErr::Io);
            }
            extent_append(&mut raw, blk)?;
            written += n;
        }
        Ok(())
    })();
    if let Err(e) = res {
        free_extents(&raw)?;
        raw[40..100].fill(0);
        w32(&mut raw, 108, 0);
        w32(&mut raw, 28, 0);
        Inode { raw }.save(ino)?;
        return Err(e);
    }
    w32(&mut raw, 4, data.len() as u32);
    w32(&mut raw, 28, (nblocks * 8) as u32);
    w16(&mut raw, 26, 1);
    Inode { raw }.save(ino)
}

pub fn rm(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    let (ino, ft) = lookup_dir(p, &name[..leaf.len()])?;
    if ft != DIR_FT_FILE {
        return Err(FsErr::IsDir);
    }
    let inode = Inode::load(ino)?;
    free_extents(&inode.raw)?;
    free_inode(ino)?;
    dir_del(p, &name[..leaf.len()])
}

pub fn rmdir(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let p = resolve_from_cwd(parent)?;
    let name = name_bytes(leaf)?;
    let (ino, ft) = lookup_dir(p, &name[..leaf.len()])?;
    if ft != DIR_FT_DIR {
        return Err(FsErr::NotDir);
    }
    let mut nonempty = false;
    walk_dir(ino, &mut |e| {
        if e.name_len == 1 && e.name[0] == b'.' {
            false
        } else if e.name_len == 2 && e.name[0] == b'.' && e.name[1] == b'.' {
            false
        } else {
            nonempty = true;
            true
        }
    })?;
    if nonempty {
        return Err(FsErr::NotEmpty);
    }
    let inode = Inode::load(ino)?;
    free_extents(&inode.raw)?;
    free_inode(ino)?;
    dir_del(p, &name[..leaf.len()])?;
    let mut pin = Inode::load(p)?;
    let links = ino_links(&pin.raw).saturating_sub(1);
    w16(&mut pin.raw, 26, links);
    pin.save(p)?;
    bump_used_dirs((ino - 1) / IPG, -1)
}

pub fn mv(old: &[char], new: &[char]) -> Result<(), FsErr> {
    let (op, ol) = split_parent(old);
    let opc = resolve_from_cwd(op)?;
    let oname = name_bytes(ol)?;
    let (ino, ft) = lookup_dir(opc, &oname[..ol.len()])?;
    let (np, nl) = split_parent(new);
    let npc = resolve_from_cwd(np)?;
    let nname = name_bytes(nl)?;
    if lookup_dir(npc, &nname[..nl.len()]).is_ok() {
        return Err(FsErr::Exists);
    }
    dir_add(npc, &nname[..nl.len()], ino, ft)?;
    dir_del(opc, &oname[..ol.len()])
}

// ---------- mkfs ----------

pub fn mkfs_at(base: u32) -> Result<(), FsErr> {
    *JBD.lock() = None; // mkfs runs without a journal; re-init after
    fs::set_base(base);
    let Some(dev) = fs::data_device() else { return Err(FsErr::NoDevice) };
    let total_blocks = (dev.sectors / SECTORS_PER_BLOCK as u64) as u32;
    if total_blocks < 8192 {
        return Err(FsErr::NoSpace);
    }
    let groups = (total_blocks + BPG - 1) / BPG;
    if groups > MAX_GROUPS {
        return Err(FsErr::Unsupported);
    }
    let desc_blocks = (groups * DESC_SIZE + BLOCK as u32 - 1) / BLOCK as u32;
    let _ = desc_blocks;

    // 1. zero metadata areas
    let zero = [0u8; BLOCK];
    for blk in 0..META_BLOCKS {
        if !write_block(blk, &zero) {
            return Err(FsErr::Io);
        }
    }
    for g in 1..groups {
        let s = g * BPG;
        for blk in s..s + META_BLOCKS {
            if !write_block(blk, &zero) {
                return Err(FsErr::Io);
            }
        }
    }

    // 2. build bitmaps
    let mut bmap_csums = [0u16; MAX_GROUPS as usize];
    let mut imap_csums = [0u16; MAX_GROUPS as usize];
    for g in 0..groups {
        let g_start = g * BPG;
        let g_blocks = (total_blocks - g_start).min(BPG);
        let mut bmap = [0u8; BLOCK];
        // mark metadata used: g0: blocks 0..516 ; g>0: g_start..g_start+516
        let meta_end = (g_start + META_BLOCKS).min(total_blocks);
        for blk in g_start..meta_end {
            let b = (blk - g_start) as usize;
            bmap[b / 8] |= 1 << (b % 8);
        }
        // mark blocks beyond the filesystem as used
        for blk in g_blocks..BPG {
            let b = blk as usize;
            bmap[b / 8] |= 1 << (b % 8);
        }
        let bmap_blk = g_start + 2;
        if !write_block(bmap_blk, &bmap) {
            return Err(FsErr::Io);
        }
        let mut imap = [0u8; BLOCK];
        if g == 0 {
            // reserve inodes 1..10
            for b in 0..10 {
                imap[b / 8] |= 1 << (b % 8);
            }
        }
        let imap_blk = g_start + 3;
        if !write_block(imap_blk, &imap) {
            return Err(FsErr::Io);
        }
        bmap_csums[g as usize] = bitmap_csum(&bmap);
        imap_csums[g as usize] = bitmap_csum(&imap);
    }

    // 3. group descriptors
    let mut table = [0u8; BLOCK];
    for g in 0..groups {
        let g_start = g * BPG;
        let g_blocks = (total_blocks - g_start).min(BPG);
        let used = META_BLOCKS.min(g_blocks);
        let free_blocks = (g_blocks - used) as u64;
        let free_inodes = if g == 0 { IPG as u64 - 10 } else { IPG as u64 };
        let o = desc_off_of(g);
        let d: &mut [u8; 64] = (&mut table[o..o + 64]).try_into().unwrap();
        w32(d, 0, g_start + 2); // block bitmap
        w32(d, 4, g_start + 3); // inode bitmap
        w32(d, 8, g_start + 4); // inode table
        desc_set_free_blocks(d, free_blocks);
        desc_set_free_inodes(d, free_inodes);
        desc_set_used_dirs(d, 0);
        w16(d, 24, bmap_csums[g as usize]);
        w16(d, 26, imap_csums[g as usize]);
        w16(d, 28, 0); // itable_unused
        w32(d, 32, 0); // hi fields
        w32(d, 36, 0);
        w32(d, 40, 0);
        w16(d, 44, 0);
        w16(d, 46, 0);
        w16(d, 48, 0);
        w16(d, 50, 0);
        w32(d, 56, 0);
        let c = desc_csum(g, d);
        w16(d, 30, c);
    }
    if !write_block(1, &table) {
        return Err(FsErr::Io);
    }

    // 4. superblock
    let mut sb = [0u8; 1024];
    w32(&mut sb, 0, groups * IPG); // inodes_count
    w32(&mut sb, 4, total_blocks);
    w32(&mut sb, 12, total_blocks - META_BLOCKS * groups + 10 * 0); // free blocks (approx, fixed later)
    w32(&mut sb, 16, groups * IPG - 10); // free inodes
    w32(&mut sb, 20, 0); // first_data_block
    w32(&mut sb, 24, 2); // log_block_size (4096)
    w32(&mut sb, 28, 2); // log_cluster_size
    w32(&mut sb, 32, BPG);
    w32(&mut sb, 36, BPG);
    w32(&mut sb, 40, IPG);
    w32(&mut sb, 44, 0); // mtime
    w32(&mut sb, 48, 0); // wtime
    w16(&mut sb, 52, 0); // mnt_count
    w16(&mut sb, 54, 0xFFFF); // max_mnt_count
    w16(&mut sb, 56, MAGIC);
    w16(&mut sb, 58, 1); // state: valid
    w16(&mut sb, 60, 0); // errors
    w16(&mut sb, 62, 0); // minor_rev
    w32(&mut sb, 64, 0); // lastcheck
    w32(&mut sb, 68, 0); // checkinterval
    w32(&mut sb, 72, 0); // creator_os: linux
    w32(&mut sb, 76, 1); // rev_level: dynamic
    w16(&mut sb, 80, 0); // def_resuid
    w16(&mut sb, 82, 0); // def_resgid
    w32(&mut sb, 84, FIRST_INO);
    w16(&mut sb, 88, INODE_SIZE as u16);
    w16(&mut sb, 90, 0); // block_group_nr
    w32(&mut sb, 92, 0); // feature_compat (no journal yet)
    w32(
        &mut sb,
        96,
        INCOMPAT_FILETYPE | INCOMPAT_EXTENTS | INCOMPAT_64BIT | INCOMPAT_META_CSUM,
    );
    w32(&mut sb, 100, RO_GDT_CSUM | RO_EXTRA_ISIZE);
    sb[104..120].copy_from_slice(&UUID);
    sb[120..136].copy_from_slice(b"SOLAROS         ");
    w32(&mut sb, 256, 0); // prealloc
    w32(&mut sb, 260, 0);
    w32(&mut sb, 280, INODE_JOURNAL); // journal_inum
    w32(&mut sb, 284, 0); // journal_dev
    w32(&mut sb, 288, 0); // last_orphan
    sb[308] = 1; // def_hash_version
    w16(&mut sb, 310, DESC_SIZE as u16);
    w32(&mut sb, 320, 0); // mkfs_time
    w32(&mut sb, 324, JOURNAL_BLOCKS); // journal_blocks
    w32(&mut sb, 328, 0); // total_blocks_hi
    w32(&mut sb, 332, 0); // blocks_count_hi
    w32(&mut sb, 336, 0); // r_blocks_count_hi
    w32(&mut sb, 340, 0); // free_blocks_hi
    w16(&mut sb, 344, 28); // min_extra_isize
    w16(&mut sb, 346, EXTRA_ISIZE); // want_extra_isize
    sb[370] = 1; // checksum_type: crc32c

    // set INFO so helpers work, then recompute free counts via updates
    *INFO.lock() = Some(Ext4Info {
        groups,
        total_blocks,
        has_csum: true,
    });
    let initial_free = total_blocks - META_BLOCKS * groups;
    w32(&mut sb, 12, initial_free);
    sb_write(&mut sb)?;

    // 5. backup superblocks + descriptor tables
    for g in 1..groups {
        let s = g * BPG;
        let mut b = [0u8; BLOCK];
        if !read_block(0, &mut b) {
            return Err(FsErr::Io);
        }
        if !write_block(s, &b) {
            return Err(FsErr::Io);
        }
        if !write_block(s + 1, &table) {
            return Err(FsErr::Io);
        }
    }

    // 6. journal area (reserved; runtime journaling comes later)
    let jblk = alloc_block()?;
    let mut j = [0u8; BLOCK];
    w32(&mut j, 0, 0xC03B_3998); // magic
    w32(&mut j, 4, 1); // blocktype: superblock v1
    w32(&mut j, 8, 1); // sequence
    w32(&mut j, 12, 0); // start
    w32(&mut j, 16, 0); // errno
    w32(&mut j, 64, BLOCK as u32); // blocksize
    w32(&mut j, 68, JOURNAL_BLOCKS); // max_len
    w32(&mut j, 72, 1); // first
    w32(&mut j, 76, JOURNAL_BLOCKS - 1); // last
    if !write_block(jblk, &j) {
        return Err(FsErr::Io);
    }
    for _ in 1..JOURNAL_BLOCKS {
        alloc_block()?;
    }
    let mut jraw = new_inode(MODE_JOURNAL, 1);
    w32(&mut jraw, 4, JOURNAL_BLOCKS * BLOCK as u32);
    w32(&mut jraw, 28, JOURNAL_BLOCKS * 8);
    let mut ib = [0u8; 60];
    w16(&mut ib, 0, EXTENT_MAGIC);
    w16(&mut ib, 2, 1);
    w16(&mut ib, 4, 4);
    w32(&mut ib, 12, 0);
    w16(&mut ib, 16, JOURNAL_BLOCKS as u16);
    w16(&mut ib, 18, (jblk >> 16) as u16);
    w32(&mut ib, 20, jblk);
    jraw[40..100].copy_from_slice(&ib);
    Inode { raw: jraw }.save(INODE_JOURNAL)?;

    // 7. seed directories: /etc /root /home /usr /boot /tmp
    let seeds: [&[u8]; 6] = [b"etc", b"root", b"home", b"usr", b"boot", b"tmp"];
    let mut child_inos = [0u32; 6];
    for (k, _name) in seeds.iter().enumerate() {
        let nino = alloc_inode()?;
        child_inos[k] = nino;
        let dblk = alloc_block()?;
        let mut b = [0u8; BLOCK];
        write_dir_entry(&mut b[0..12], nino, b".", DIR_FT_DIR, 12);
        write_dir_entry(&mut b[12..BLOCK], INODE_ROOT, b"..", DIR_FT_DIR, BLOCK - 12);
        if !write_block(dblk, &b) {
            return Err(FsErr::Io);
        }
        let mut raw = new_inode(MODE_DIR, 2);
        w32(&mut raw, 4, BLOCK as u32);
        w32(&mut raw, 28, 8);
        let mut ib = [0u8; 60];
        w16(&mut ib, 0, EXTENT_MAGIC);
        w16(&mut ib, 2, 1);
        w16(&mut ib, 4, 4);
        w32(&mut ib, 12, 0);
        w16(&mut ib, 16, 1);
        w16(&mut ib, 18, (dblk >> 16) as u16);
        w32(&mut ib, 20, dblk);
        raw[40..100].copy_from_slice(&ib);
        Inode { raw }.save(nino)?;
        bump_used_dirs(0, 1)?;
    }

    // 8. root directory
    let rblk = alloc_block()?;
    let mut b = [0u8; BLOCK];
    let mut o = 0usize;
    for (k, name) in seeds.iter().enumerate() {
        let need = align4(8 + name.len());
        let last = k == seeds.len() - 1;
        let rec = if last { BLOCK - o } else { need };
        write_dir_entry(&mut b[o..o + rec], child_inos[k], name, DIR_FT_DIR, rec);
        o += rec;
    }
    if !write_block(rblk, &b) {
        return Err(FsErr::Io);
    }
    let mut rraw = new_inode(MODE_DIR, 2 + seeds.len() as u16);
    w32(&mut rraw, 4, BLOCK as u32);
    w32(&mut rraw, 28, 8);
    let mut ib = [0u8; 60];
    w16(&mut ib, 0, EXTENT_MAGIC);
    w16(&mut ib, 2, 1);
    w16(&mut ib, 4, 4);
    w32(&mut ib, 12, 0);
    w16(&mut ib, 16, 1);
    w16(&mut ib, 18, (rblk >> 16) as u16);
    w32(&mut ib, 20, rblk);
    rraw[40..100].copy_from_slice(&ib);
    Inode { raw: rraw }.save(INODE_ROOT)?;
    bump_used_dirs(0, 1)?;

    mount_at(base)?;
    Ok(())
}
