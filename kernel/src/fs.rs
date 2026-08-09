use spin::Mutex;

use crate::ata::{self, AtaDevice, MAX_DEVICES};

pub const SECTOR_SIZE: usize = 512;
pub(crate) const MAX_DEPTH: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsErr {
    NoDevice,
    NotFormatted,
    Unsupported,
    Io,
    NotFound,
    NotDir,
    IsDir,
    Exists,
    BadName,
    TooBig,
    NoSpace,
    NotEmpty,
}

pub fn err_str(e: FsErr) -> &'static str {
    match e {
        FsErr::NoDevice => "no data disk found",
        FsErr::NotFormatted => "data disk is not formatted (run \"mkfs\")",
        FsErr::Unsupported => "unsupported filesystem features",
        FsErr::Io => "I/O error",
        FsErr::NotFound => "no such file or directory",
        FsErr::NotDir => "not a directory",
        FsErr::IsDir => "is a directory",
        FsErr::Exists => "already exists",
        FsErr::BadName => "invalid name",
        FsErr::TooBig => "too many path components or file too big",
        FsErr::NoSpace => "out of space",
        FsErr::NotEmpty => "directory not empty",
    }
}

pub(crate) fn data_device() -> Option<AtaDevice> {
    let mut best: Option<AtaDevice> = None;
    for i in 0..MAX_DEVICES {
        let d = ata::device(i);
        if !d.present {
            continue;
        }
        if best.map_or(true, |b| d.sectors > b.sectors) {
            best = Some(d);
        }
    }
    best
}

// base offset (in sectors) of the currently mounted filesystem
static BASE: Mutex<u32> = Mutex::new(0);

pub(crate) fn set_base(b: u32) {
    *BASE.lock() = b;
}

pub(crate) fn raw_read(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> bool {
    let Some(dev) = data_device() else { return false };
    let base = if dev.is_secondary { 0x170 } else { 0x1F0 };
    ata::read_sector(base, false, dev.master, *BASE.lock() + lba, buf)
}

pub(crate) fn raw_write(lba: u32, buf: &[u8; SECTOR_SIZE]) -> bool {
    let Some(dev) = data_device() else { return false };
    let base = if dev.is_secondary { 0x170 } else { 0x1F0 };
    ata::write_sector(base, false, dev.master, *BASE.lock() + lba, buf)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    Fat,
    Ext4,
}

#[derive(Clone, Copy)]
struct Mounted {
    kind: FsKind,
}

static MOUNT: Mutex<Option<Mounted>> = Mutex::new(None);

pub fn mounted() -> bool {
    MOUNT.lock().is_some()
}

fn kind() -> Result<FsKind, FsErr> {
    MOUNT.lock().map(|m| m.kind).ok_or(FsErr::NotFormatted)
}

pub fn fs_name() -> &'static str {
    match kind() {
        Ok(FsKind::Fat) => "FAT32",
        Ok(FsKind::Ext4) => "ext4",
        Err(_) => "none",
    }
}

pub(crate) fn mark_mounted(kind: FsKind, _base: u32) {
    *MOUNT.lock() = Some(Mounted { kind });
}

pub fn mount() -> Result<(), FsErr> {
    mount_at(0)
}

pub fn mount_at(base: u32) -> Result<(), FsErr> {
    set_base(base);
    if data_device().is_none() {
        return Err(FsErr::NoDevice);
    }
    let mut sb = [0u8; SECTOR_SIZE];
    if !raw_read(2, &mut sb) {
        return Err(FsErr::Io);
    }
    if u16::from_le_bytes([sb[56], sb[57]]) == 0xEF53 {
        return crate::ext4::mount_at(base);
    }
    crate::fat::mount_at(base)
}

pub fn mkfs() -> Result<(), FsErr> {
    crate::fat::mkfs_at(0)
}

pub fn mkfs_ext4() -> Result<(), FsErr> {
    crate::ext4::mkfs_at(0)
}

// ---------- shared CWD (path based, fs agnostic) ----------

#[derive(Clone, Copy)]
pub(crate) struct Cwd {
    pub(crate) parts: [[u8; 12]; MAX_DEPTH],
    pub(crate) lens: [usize; MAX_DEPTH],
    pub(crate) n: usize,
}

impl Cwd {
    const fn new() -> Self {
        Cwd {
            parts: [[0; 12]; MAX_DEPTH],
            lens: [0; MAX_DEPTH],
            n: 0,
        }
    }
}

static CWD: Mutex<Cwd> = Mutex::new(Cwd::new());

pub(crate) fn cwd() -> Cwd {
    *CWD.lock()
}

fn strip_slashes(p: &[char]) -> &[char] {
    let mut s = 0;
    while s < p.len() && p[s] == '/' {
        s += 1;
    }
    &p[s..]
}

fn pop_comp(p: &[char]) -> Option<(&[char], &[char])> {
    let p = strip_slashes(p);
    if p.is_empty() {
        return None;
    }
    let mut s = 0;
    while s < p.len() && p[s] != '/' {
        s += 1;
    }
    Some((&p[..s], &p[s..]))
}

pub fn cd(path: &[char]) -> Result<(), FsErr> {
    let absolute = !path.is_empty() && path[0] == '/';
    let mut cand = *CWD.lock();
    let mut n = if absolute { 0 } else { cand.n };
    let mut rest = path;
    while let Some((comp, tail)) = pop_comp(rest) {
        if comp.is_empty() {
            rest = tail;
            continue;
        }
        if comp.len() == 1 && comp[0] == '.' {
            rest = tail;
            continue;
        }
        if comp.len() == 2 && comp[0] == '.' && comp[1] == '.' {
            if n > 0 {
                n -= 1;
            }
            rest = tail;
            continue;
        }
        if n >= MAX_DEPTH {
            return Err(FsErr::TooBig);
        }
        let mut part = [0u8; 12];
        let mut len = 0usize;
        for &c in comp {
            if len < 12 {
                part[len] = c as u8;
                len += 1;
            }
        }
        cand.parts[n] = part;
        cand.lens[n] = len;
        n += 1;
        rest = tail;
    }
    cand.n = n;
    match kind()? {
        FsKind::Fat => crate::fat::check_dir(&cand)?,
        FsKind::Ext4 => crate::ext4::check_dir(&cand)?,
    }
    *CWD.lock() = cand;
    Ok(())
}

pub fn pwd() {
    let cwd = *CWD.lock();
    if cwd.n == 0 {
        crate::println!("/");
        return;
    }
    for k in 0..cwd.n {
        crate::print!("/");
        for j in 0..cwd.lens[k] {
            crate::print!("{}", (cwd.parts[k][j] as char).to_ascii_lowercase());
        }
    }
    crate::println!();
}

// ---------- command dispatch ----------

// every mutating operation commits the journal transaction afterwards, so
// each command is crash-atomic on the ext4 filesystem
fn commit_after(r: Result<(), FsErr>) -> Result<(), FsErr> {
    if kind().ok() == Some(FsKind::Ext4) {
        crate::ext4::jbd_commit()?;
    }
    r
}

pub fn ls(path: &[char]) -> Result<(), FsErr> {
    match kind()? {
        FsKind::Fat => crate::fat::ls(path),
        FsKind::Ext4 => crate::ext4::ls(path),
    }
}

pub fn mkdir(path: &[char]) -> Result<(), FsErr> {
    let r = match kind()? {
        FsKind::Fat => crate::fat::mkdir(path),
        FsKind::Ext4 => crate::ext4::mkdir(path),
    };
    commit_after(r)
}

pub fn touch(path: &[char]) -> Result<(), FsErr> {
    let r = match kind()? {
        FsKind::Fat => crate::fat::touch(path),
        FsKind::Ext4 => crate::ext4::touch(path),
    };
    commit_after(r)
}

pub fn cat(path: &[char]) -> Result<(), FsErr> {
    match kind()? {
        FsKind::Fat => crate::fat::cat(path),
        FsKind::Ext4 => crate::ext4::cat(path),
    }
}

pub fn rm(path: &[char]) -> Result<(), FsErr> {
    let r = match kind()? {
        FsKind::Fat => crate::fat::rm(path),
        FsKind::Ext4 => crate::ext4::rm(path),
    };
    commit_after(r)
}

pub fn rmdir(path: &[char]) -> Result<(), FsErr> {
    let r = match kind()? {
        FsKind::Fat => crate::fat::rmdir(path),
        FsKind::Ext4 => crate::ext4::rmdir(path),
    };
    commit_after(r)
}

pub fn mv(old: &[char], new: &[char]) -> Result<(), FsErr> {
    let r = match kind()? {
        FsKind::Fat => crate::fat::mv(old, new),
        FsKind::Ext4 => crate::ext4::mv(old, new),
    };
    commit_after(r)
}

pub fn read_file(path: &[char], out: &mut [u8]) -> Result<usize, FsErr> {
    match kind()? {
        FsKind::Fat => crate::fat::read_file(path, out),
        FsKind::Ext4 => crate::ext4::read_file(path, out),
    }
}

pub fn write_file(path: &[char], data: &[u8]) -> Result<(), FsErr> {
    let r = match kind()? {
        FsKind::Fat => crate::fat::write_file(path, data),
        FsKind::Ext4 => crate::ext4::write_file(path, data),
    };
    commit_after(r)
}
