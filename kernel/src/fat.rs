use spin::Mutex;

use crate::fs::{self, FsErr, SECTOR_SIZE};

const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LFN: u8 = 0x0F;

const FAT_EOC_MASK: u32 = 0x0FFF_FFF8;

const MAX_FILE: usize = 65536;

#[derive(Clone, Copy)]
struct FsInfo {
    sec_per_clus: usize,
    rsvc: u32,
    num_fats: u32,
    fat_size: u32,
    root_clus: u32,
    data_start: u32,
    cluster_count: u32,
}

static INFO: Mutex<Option<FsInfo>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct DirEntry {
    name: [u8; 11],
    attr: u8,
    cluster: u32,
    size: u32,
}

fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> bool {
    fs::raw_read(lba, buf)
}

fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> bool {
    fs::raw_write(lba, buf)
}

pub fn mount_at(base: u32) -> Result<(), FsErr> {
    fs::set_base(base);
    if fs::data_device().is_none() {
        return Err(FsErr::NoDevice);
    }
    let mut boot = [0u8; SECTOR_SIZE];
    if !read_sector(0, &mut boot) {
        return Err(FsErr::Io);
    }
    if boot[510] != 0x55 || boot[511] != 0xAA {
        return Err(FsErr::NotFormatted);
    }

    let byts = u16::from_le_bytes([boot[11], boot[12]]);
    let sec_per_clus = boot[13] as usize;
    let rsvd_secs = u16::from_le_bytes([boot[14], boot[15]]) as u32;
    let num_fats = boot[16] as u32;
    let tot32 = u32::from_le_bytes([boot[32], boot[33], boot[34], boot[35]]);
    let fat16_sz = u16::from_le_bytes([boot[22], boot[23]]);
    let fat32_sz = u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]);
    let root_clus = u32::from_le_bytes([boot[44], boot[45], boot[46], boot[47]]);

    if byts != 512
        || sec_per_clus == 0
        || sec_per_clus > 128
        || rsvd_secs == 0
        || num_fats == 0
        || num_fats > 4
        || tot32 < 70000
        || fat16_sz != 0
        || fat32_sz == 0
        || root_clus < 2
    {
        return Err(FsErr::NotFormatted);
    }

    let data_start = rsvd_secs + num_fats * fat32_sz;
    if data_start >= tot32 {
        return Err(FsErr::NotFormatted);
    }
    let cluster_count = (tot32 - data_start) / sec_per_clus as u32;
    if cluster_count < 65525 {
        return Err(FsErr::NotFormatted);
    }

    *INFO.lock() = Some(FsInfo {
        sec_per_clus,
        rsvc: rsvd_secs,
        num_fats,
        fat_size: fat32_sz,
        root_clus,
        data_start,
        cluster_count,
    });
    fs::mark_mounted(fs::FsKind::Fat, base);
    Ok(())
}

fn info() -> Option<FsInfo> {
    *INFO.lock()
}

fn first_sector(clu: u32) -> u32 {
    let i = info().unwrap();
    i.data_start + (clu - 2) * i.sec_per_clus as u32
}

fn is_eoc(v: u32) -> bool {
    v & FAT_EOC_MASK >= FAT_EOC_MASK
}

fn read_fat(clu: u32) -> u32 {
    let i = info().unwrap();
    let off = clu as usize * 4;
    let mut b = [0u8; SECTOR_SIZE];
    if !read_sector(i.rsvc + (off / 512) as u32, &mut b) {
        return FAT_EOC_MASK;
    }
    let o = off % 512;
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn write_fat(clu: u32, val: u32) -> bool {
    let i = info().unwrap();
    let off = clu as usize * 4;
    let base = i.rsvc + (off / 512) as u32;
    let mut b = [0u8; SECTOR_SIZE];
    if !read_sector(base, &mut b) {
        return false;
    }
    let o = off % 512;
    b[o..o + 4].copy_from_slice(&val.to_le_bytes());
    for f in 0..i.num_fats {
        if !write_sector(base + f * i.fat_size, &b) {
            return false;
        }
    }
    true
}

fn alloc_cluster() -> Option<u32> {
    let i = info().unwrap();
    for c in 2..i.cluster_count + 2 {
        if read_fat(c) == 0 {
            if write_fat(c, FAT_EOC_MASK) {
                return Some(c);
            }
        }
    }
    None
}

fn free_chain(mut clu: u32) {
    if clu < 2 {
        return;
    }
    for _ in 0..2_000_000 {
        let nxt = read_fat(clu);
        write_fat(clu, 0);
        if is_eoc(nxt) || nxt < 2 {
            return;
        }
        clu = nxt;
    }
}

fn make_83(name: &[char]) -> Result<[u8; 11], FsErr> {
    let mut out = [b' '; 11];
    let mut seen_dot = false;
    let mut base = 0usize;
    let mut ext = 0usize;
    for &c in name {
        if c == '.' {
            if seen_dot {
                return Err(FsErr::BadName);
            }
            seen_dot = true;
            continue;
        }
        let b = c as u8;
        if !c.is_ascii() || b == 0 || b == b'/' || b == b'\\' || b == b' ' {
            return Err(FsErr::BadName);
        }
        let up = b.to_ascii_uppercase();
        if !seen_dot {
            if base >= 8 {
                return Err(FsErr::BadName);
            }
            out[base] = up;
            base += 1;
        } else {
            if ext >= 3 {
                return Err(FsErr::BadName);
            }
            out[8 + ext] = up;
            ext += 1;
        }
    }
    if base == 0 {
        return Err(FsErr::BadName);
    }
    if base == 1 && out[0] == b'.' {
        return Err(FsErr::BadName);
    }
    if base == 2 && out[0] == b'.' && out[1] == b'.' {
        return Err(FsErr::BadName);
    }
    Ok(out)
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

fn cmp83(a: &[u8; 11], b: &[u8; 11]) -> bool {
    a[..11] == b[..11]
}

fn walk_dir<F: FnMut(&DirEntry) -> bool>(dir: u32, mut f: F) {
    let Some(i) = info() else { return };
    let mut clu = dir;
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > i.cluster_count {
            return;
        }
        for s in 0..i.sec_per_clus {
            let mut b = [0u8; SECTOR_SIZE];
            if !read_sector(first_sector(clu) + s as u32, &mut b) {
                return;
            }
            for k in 0..16 {
                let o = k * 32;
                let first = b[o];
                if first == 0x00 {
                    return;
                }
                if first == 0xE5 || b[o + 11] == ATTR_LFN {
                    continue;
                }
                let mut e = DirEntry {
                    name: [0; 11],
                    attr: b[o + 11],
                    cluster: 0,
                    size: 0,
                };
                e.name.copy_from_slice(&b[o..o + 11]);
                e.cluster = (u32::from_le_bytes([b[o + 20], b[o + 21], 0, 0]) << 16)
                    | u32::from_le_bytes([b[o + 26], b[o + 27], 0, 0]);
                e.size = u32::from_le_bytes([b[o + 28], b[o + 29], b[o + 30], b[o + 31]]);
                if e.name[0] == b'.' && (e.name[1] == b' ' || e.name[1] == b'.') {
                    continue;
                }
                if f(&e) {
                    return;
                }
            }
        }
        let nxt = read_fat(clu);
        if is_eoc(nxt) || nxt < 2 {
            return;
        }
        clu = nxt;
    }
}

fn look_up(dir: u32, name: &[u8; 11]) -> Result<DirEntry, FsErr> {
    let mut found = None;
    walk_dir(dir, &mut |e: &DirEntry| {
        if cmp83(&e.name, name) {
            found = Some(*e);
            true
        } else {
            false
        }
    });
    found.ok_or(FsErr::NotFound)
}

fn lookup_cluster(dir: u32, name: &[u8; 11], dir_only: bool) -> Result<u32, FsErr> {
    let e = look_up(dir, name)?;
    if dir_only && e.attr & ATTR_DIRECTORY == 0 {
        return Err(FsErr::NotDir);
    }
    if !dir_only && e.attr & ATTR_DIRECTORY != 0 {
        return Err(FsErr::IsDir);
    }
    Ok(e.cluster)
}

fn parent_of(clu: u32) -> Result<u32, FsErr> {
    let root = info().unwrap().root_clus;
    if clu == root {
        return Ok(root);
    }
    let mut parent = None;
    let Some(i) = info() else { return Err(FsErr::Io) };
    let mut c = clu;
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > i.cluster_count {
            break;
        }
        for s in 0..i.sec_per_clus {
            let mut b = [0u8; SECTOR_SIZE];
            if !read_sector(first_sector(c) + s as u32, &mut b) {
                break;
            }
            for k in 0..16 {
                let o = k * 32;
                let first = b[o];
                if first == 0x00 || first == 0xE5 || b[o + 11] == ATTR_LFN {
                    continue;
                }
                if b[o] == b'.' && b[o + 1] == b'.' && b[o + 2] == b' ' {
                    parent = Some(
                        (u32::from_le_bytes([b[o + 20], b[o + 21], 0, 0]) << 16)
                            | u32::from_le_bytes([b[o + 26], b[o + 27], 0, 0]),
                    );
                    break;
                }
            }
            if parent.is_some() {
                break;
            }
        }
        if parent.is_some() {
            break;
        }
        let nxt = read_fat(c);
        if is_eoc(nxt) || nxt < 2 {
            break;
        }
        c = nxt;
    }
    match parent {
        Some(0) => Ok(root),
        Some(p) => Ok(p),
        None => Ok(root),
    }
}

fn resolve(from: u32, path: &[char]) -> Result<u32, FsErr> {
    let root = info().unwrap().root_clus;
    let mut clu = if !path.is_empty() && path[0] == '/' {
        root
    } else {
        from
    };
    let mut rest = path;
    while let Some((comp, tail)) = pop_comp(rest) {
        if comp.len() == 1 && comp[0] == '.' {
            rest = tail;
            continue;
        }
        if comp.len() == 2 && comp[0] == '.' && comp[1] == '.' {
            clu = parent_of(clu)?;
            rest = tail;
            continue;
        }
        if comp.is_empty() {
            rest = tail;
            continue;
        }
        let n83 = make_83(comp)?;
        clu = lookup_cluster(clu, &n83, true)?;
        rest = tail;
    }
    Ok(clu)
}

fn find_pos(dir: u32, name: &[u8; 11]) -> Result<(u32, usize), FsErr> {
    let Some(i) = info() else { return Err(FsErr::NotFormatted) };
    let mut clu = dir;
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > i.cluster_count + 2 {
            return Err(FsErr::Io);
        }
        for s in 0..i.sec_per_clus {
            let mut b = [0u8; SECTOR_SIZE];
            if !read_sector(first_sector(clu) + s as u32, &mut b) {
                return Err(FsErr::Io);
            }
            for k in 0..16 {
                let o = k * 32;
                let first = b[o];
                if first == 0x00 {
                    return Err(FsErr::NotFound);
                }
                if first == 0xE5 || b[o + 11] == ATTR_LFN {
                    continue;
                }
                let mut e = DirEntry {
                    name: [0; 11],
                    attr: b[o + 11],
                    cluster: 0,
                    size: 0,
                };
                e.name.copy_from_slice(&b[o..o + 11]);
                if cmp83(&e.name, name) {
                    return Ok((first_sector(clu) + s as u32, k));
                }
            }
        }
        let nxt = read_fat(clu);
        if is_eoc(nxt) || nxt < 2 {
            return Err(FsErr::NotFound);
        }
        clu = nxt;
    }
}

fn write_entry_at(lba: u32, slot: usize, e: &DirEntry) -> Result<(), FsErr> {
    let mut b = [0u8; SECTOR_SIZE];
    if !read_sector(lba, &mut b) {
        return Err(FsErr::Io);
    }
    let o = slot * 32;
    b[o..o + 11].copy_from_slice(&e.name);
    b[o + 11] = e.attr;
    b[o + 20..o + 22].copy_from_slice(&((e.cluster >> 16) as u16).to_le_bytes());
    b[o + 26..o + 28].copy_from_slice(&(e.cluster as u16).to_le_bytes());
    b[o + 28..o + 32].copy_from_slice(&e.size.to_le_bytes());
    if !write_sector(lba, &b) {
        return Err(FsErr::Io);
    }
    Ok(())
}

fn wipe_entry_at(lba: u32, slot: usize) -> Result<(), FsErr> {
    let mut b = [0u8; SECTOR_SIZE];
    if !read_sector(lba, &mut b) {
        return Err(FsErr::Io);
    }
    b[slot * 32] = 0xE5;
    if !write_sector(lba, &b) {
        return Err(FsErr::Io);
    }
    Ok(())
}

fn find_free_slot(dir: u32) -> Result<(u32, usize), FsErr> {
    let i = info().unwrap();
    let mut clu = dir;
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > i.cluster_count + 2 {
            return Err(FsErr::Io);
        }
        for s in 0..i.sec_per_clus {
            let mut b = [0u8; SECTOR_SIZE];
            if !read_sector(first_sector(clu) + s as u32, &mut b) {
                return Err(FsErr::Io);
            }
            for k in 0..16 {
                let first = b[k * 32];
                if first == 0x00 || first == 0xE5 {
                    return Ok((first_sector(clu) + s as u32, k));
                }
            }
        }
        let nxt = read_fat(clu);
        if is_eoc(nxt) {
            let nc = alloc_cluster().ok_or(FsErr::NoSpace)?;
            if !write_fat(clu, nc) {
                return Err(FsErr::Io);
            }
            let zero = [0u8; SECTOR_SIZE];
            for s in 0..i.sec_per_clus {
                write_sector(first_sector(nc) + s as u32, &zero);
            }
            return Ok((first_sector(nc), 0));
        }
        clu = nxt;
    }
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

// ---------- CWD ----------

fn cluster_from_cwd(cwd: &fs::Cwd) -> Result<u32, FsErr> {
    let i = info().ok_or(FsErr::NotFormatted)?;
    let mut clu = i.root_clus;
    for k in 0..cwd.n {
        let part = &cwd.parts[k];
        let len = cwd.lens[k];
        let mut n83 = [b' '; 11];
        for j in 0..len.min(11) {
            n83[j] = part[j].to_ascii_uppercase();
        }
        clu = lookup_cluster(clu, &n83, true)?;
    }
    Ok(clu)
}

pub(crate) fn cwd_cluster() -> Result<u32, FsErr> {
    cluster_from_cwd(&fs::cwd())
}

pub(crate) fn check_dir(cwd: &fs::Cwd) -> Result<(), FsErr> {
    let _ = cluster_from_cwd(cwd)?;
    Ok(())
}

fn resolve_from_cwd(path: &[char]) -> Result<u32, FsErr> {
    if !path.is_empty() && path[0] == '/' {
        resolve(info().unwrap().root_clus, path)
    } else {
        let from = cwd_cluster()?;
        resolve(from, path)
    }
}

// ---------- commands ----------

pub fn ls(path: &[char]) -> Result<(), FsErr> {
    let dir = resolve_from_cwd(path)?;
    walk_dir(dir, &mut |e: &DirEntry| {
        let mut name = [0u8; 16];
        let mut len = 0usize;
        let mut k = 0usize;
        while k < 8 && e.name[k] != b' ' {
            if len < 16 {
                name[len] = e.name[k].to_ascii_lowercase();
                len += 1;
            }
            k += 1;
        }
        if e.name[8] != b' ' {
            if len < 16 {
                name[len] = b'.';
                len += 1;
            }
            for j in 8..11 {
                if e.name[j] != b' ' && len < 16 {
                    name[len] = e.name[j].to_ascii_lowercase();
                    len += 1;
                }
            }
        }
        let mut disp = ['\0'; 16];
        for j in 0..len {
            disp[j] = name[j] as char;
        }
        if e.attr & ATTR_DIRECTORY != 0 {
            crate::println!("{:<24} {:<6} {}", crate::Utf8Chars(&disp[..len]), "<DIR>", "");
        } else {
            crate::println!(
                "{:<24} {:<6} {:>8}",
                crate::Utf8Chars(&disp[..len]),
                "file",
                e.size
            );
        }
        false
    });
    Ok(())
}

pub fn mkdir(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let n83 = make_83(leaf)?;
    if look_up(parent_clu, &n83).is_ok() {
        return Err(FsErr::Exists);
    }
    let (lba, slot) = find_free_slot(parent_clu)?;
    let Some(clus) = alloc_cluster() else { return Err(FsErr::NoSpace) };
    let i = info().unwrap();
    let zero = [0u8; SECTOR_SIZE];
    for s in 0..i.sec_per_clus {
        if !write_sector(first_sector(clus) + s as u32, &zero) {
            return Err(FsErr::Io);
        }
    }
    let dot = DirEntry {
        name: [b'.', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' '],
        attr: ATTR_DIRECTORY,
        cluster: clus,
        size: 0,
    };
    let dotdot = DirEntry {
        name: [b'.', b'.', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' '],
        attr: ATTR_DIRECTORY,
        cluster: if parent_clu == i.root_clus { 0 } else { parent_clu },
        size: 0,
    };
    write_entry_at(first_sector(clus), 0, &dot)?;
    write_entry_at(first_sector(clus), 1, &dotdot)?;
    let e = DirEntry {
        name: n83,
        attr: ATTR_DIRECTORY,
        cluster: clus,
        size: 0,
    };
    write_entry_at(lba, slot, &e)?;
    Ok(())
}

pub fn touch(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let leaf83 = make_83(leaf)?;
    if look_up(parent_clu, &leaf83).is_ok() {
        return Ok(());
    }
    let (lba, slot) = find_free_slot(parent_clu)?;
    let e = DirEntry {
        name: leaf83,
        attr: ATTR_ARCHIVE,
        cluster: 0,
        size: 0,
    };
    write_entry_at(lba, slot, &e)
}

pub fn cat(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let leaf83 = make_83(leaf)?;
    let e = look_up(parent_clu, &leaf83)?;
    if e.attr & ATTR_DIRECTORY != 0 {
        return Err(FsErr::IsDir);
    }
    let sz = e.size as usize;
    let mut clu = e.cluster;
    let i = info().unwrap();
    let mut left = sz;
    let mut guard = 0u32;
    while left > 0 {
        guard += 1;
        if guard > i.cluster_count {
            break;
        }
        let chunk = left.min(i.sec_per_clus * 512);
        for s in 0..(chunk + 511) / 512 {
            let mut b = [0u8; SECTOR_SIZE];
            if !read_sector(first_sector(clu) + s as u32, &mut b) {
                return Err(FsErr::Io);
            }
            let n = chunk.min((s + 1) * 512) - s * 512;
            for k in 0..n.min(512) {
                let c = b[k];
                if (0x20..0x7F).contains(&c) {
                    crate::print!("{}", c as char);
                } else {
                    crate::print!("?");
                }
            }
        }
        left -= chunk;
        let nxt = read_fat(clu);
        if is_eoc(nxt) || nxt < 2 {
            break;
        }
        clu = nxt;
    }
    Ok(())
}

pub fn rm(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let n83 = make_83(leaf)?;
    let e = look_up(parent_clu, &n83)?;
    if e.attr & ATTR_DIRECTORY != 0 {
        return Err(FsErr::IsDir);
    }
    let (lba, slot) = find_pos(parent_clu, &n83)?;
    if e.cluster != 0 {
        free_chain(e.cluster);
    }
    wipe_entry_at(lba, slot)
}

pub fn rmdir(path: &[char]) -> Result<(), FsErr> {
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let n83 = make_83(leaf)?;
    let e = look_up(parent_clu, &n83)?;
    if e.attr & ATTR_DIRECTORY == 0 {
        return Err(FsErr::NotDir);
    }
    let (lba, slot) = find_pos(parent_clu, &n83)?;
    let mut nonempty = false;
    walk_dir(e.cluster, &mut |_: &DirEntry| {
        nonempty = true;
        true
    });
    if nonempty {
        return Err(FsErr::NotEmpty);
    }
    if e.cluster != 0 {
        free_chain(e.cluster);
    }
    wipe_entry_at(lba, slot)
}

pub fn mv(old: &[char], new: &[char]) -> Result<(), FsErr> {
    let (op, ol) = split_parent(old);
    let op_clu = resolve_from_cwd(op)?;
    let o83 = make_83(ol)?;
    let e = look_up(op_clu, &o83)?;
    let (lba, slot) = find_pos(op_clu, &o83)?;

    let (np, nl) = split_parent(new);
    let np_clu = resolve_from_cwd(np)?;
    let n83 = make_83(nl)?;
    if look_up(np_clu, &n83).is_ok() {
        return Err(FsErr::Exists);
    }
    let (lba2, slot2) = find_free_slot(np_clu)?;
    let mut ne = e;
    ne.name = n83;
    write_entry_at(lba2, slot2, &ne)?;
    wipe_entry_at(lba, slot)?;
    Ok(())
}

// ---------- files ----------

pub fn read_file(path: &[char], out: &mut [u8]) -> Result<usize, FsErr> {
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let leaf83 = make_83(leaf)?;
    let e = look_up(parent_clu, &leaf83)?;
    if e.attr & ATTR_DIRECTORY != 0 {
        return Err(FsErr::IsDir);
    }
    let size = (e.size as usize).min(out.len());
    if size == 0 || e.cluster == 0 {
        return Ok(0);
    }
    let i = info().unwrap();
    let clu_size = i.sec_per_clus * 512;
    let mut clu = e.cluster;
    let mut written = 0usize;
    let mut guard = 0u32;
    while written < size {
        guard += 1;
        if guard > i.cluster_count {
            break;
        }
        let chunk = (size - written).min(clu_size);
        let mut off = 0usize;
        while off < chunk {
            let mut b = [0u8; SECTOR_SIZE];
            if !read_sector(first_sector(clu) + (off / 512) as u32, &mut b) {
                return Err(FsErr::Io);
            }
            let n = (chunk - off).min(512);
            out[written + off..written + off + n].copy_from_slice(&b[..n]);
            off += n;
        }
        written += chunk;
        let nxt = read_fat(clu);
        if is_eoc(nxt) || nxt < 2 {
            break;
        }
        clu = nxt;
    }
    Ok(written)
}

pub fn write_file(path: &[char], data: &[u8]) -> Result<(), FsErr> {
    if data.len() > MAX_FILE {
        return Err(FsErr::TooBig);
    }
    let (parent, leaf) = split_parent(path);
    let parent_clu = resolve_from_cwd(parent)?;
    let n83 = make_83(leaf)?;
    let existing = look_up(parent_clu, &n83).ok();

    let i = info().unwrap();
    let clu_size = i.sec_per_clus * 512;
    let need = (data.len() + clu_size - 1) / clu_size;

    let mut chain = [0u32; 128];

    let mut first_clu = 0u32;
    if need > 0 {
        first_clu = alloc_cluster().ok_or(FsErr::NoSpace)?;
        chain[0] = first_clu;
        for k in 1..need {
            let nc = alloc_cluster().ok_or(FsErr::NoSpace)?;
            write_fat(chain[k - 1], nc);
            chain[k] = nc;
        }
        write_fat(chain[need - 1], FAT_EOC_MASK);
    }

    if let Some(e) = existing {
        if e.cluster != 0 {
            free_chain(e.cluster);
        }
        let (lba, slot) = find_pos(parent_clu, &n83)?;
        let ne = DirEntry {
            name: n83,
            attr: e.attr,
            cluster: first_clu,
            size: data.len() as u32,
        };
        write_entry_at(lba, slot, &ne)?;
    } else {
        if need == 0 {
            let (lba, slot) = find_free_slot(parent_clu)?;
            let e = DirEntry {
                name: n83,
                attr: ATTR_ARCHIVE,
                cluster: 0,
                size: 0,
            };
            write_entry_at(lba, slot, &e)?;
            return Ok(());
        }
        let (lba, slot) = find_free_slot(parent_clu)?;
        let e = DirEntry {
            name: n83,
            attr: ATTR_ARCHIVE,
            cluster: first_clu,
            size: data.len() as u32,
        };
        write_entry_at(lba, slot, &e)?;
    }

    let mut off = 0usize;
    let mut ci = 0usize;
    while off < data.len() {
        let chunk = (data.len() - off).min(clu_size);
        let mut co = 0usize;
        while co < chunk {
            let mut b = [0u8; SECTOR_SIZE];
            let n = (chunk - co).min(512);
            b[..n].copy_from_slice(&data[off + co..off + co + n]);
            if !write_sector(first_sector(chain[ci]) + (co / 512) as u32, &b) {
                return Err(FsErr::Io);
            }
            co += n;
        }
        off += chunk;
        ci += 1;
    }
    Ok(())
}

// ---------- mkfs ----------

pub fn mkfs_at(base: u32) -> Result<(), FsErr> {
    fs::set_base(base);
    let Some(dev) = fs::data_device() else { return Err(FsErr::NoDevice) };
    let tot = dev.sectors.min(0xFFFF_FFF0) as u32;
    if tot < 70000 {
        return Err(FsErr::NoSpace);
    }

    let spc: u32 = 1;
    let rsvd: u32 = 32;
    let nfats: u32 = 2;
    let mut fat_sz: u32 = 2;
    for _ in 0..32 {
        let data_sec = tot - rsvd - nfats * fat_sz;
        let clus = data_sec / spc;
        let need = (clus + 2) * 4 / 512 + 1;
        if need <= fat_sz {
            fat_sz = need;
            break;
        }
        fat_sz = need;
    }
    let data_start = rsvd + nfats * fat_sz;
    let cluster_count = (tot - data_start) / spc;
    if cluster_count < 65525 {
        return Err(FsErr::NoSpace);
    }

    // boot sector
    let mut b = [0u8; 512];
    b[0] = 0xEB;
    b[1] = 0x58;
    b[2] = 0x90;
    b[3..11].copy_from_slice(b"SOLAROS ");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = 1;
    b[14..16].copy_from_slice(&(rsvd as u16).to_le_bytes());
    b[16] = nfats as u8;
    b[21] = 0xF8;
    b[24..26].copy_from_slice(&63u16.to_le_bytes());
    b[26..28].copy_from_slice(&255u16.to_le_bytes());
    b[32..36].copy_from_slice(&tot.to_le_bytes());
    b[36..40].copy_from_slice(&fat_sz.to_le_bytes());
    b[44..48].copy_from_slice(&2u32.to_le_bytes());
    b[48..50].copy_from_slice(&0u16.to_le_bytes());
    b[50..52].copy_from_slice(&6u16.to_le_bytes());
    b[66] = 0x29;
    b[67..71].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    b[71..82].copy_from_slice(b"SOLAROS    ");
    b[82..90].copy_from_slice(b"FAT32   ");
    b[510] = 0x55;
    b[511] = 0xAA;

    // zero reserved + FATs + first data sector, then restore boot/fsinfo
    let zero = [0u8; 512];
    for s in 0..rsvd + nfats * fat_sz + 1 {
        if !write_sector(s, &zero) {
            return Err(FsErr::Io);
        }
    }
    if !write_sector(0, &b) {
        return Err(FsErr::Io);
    }
    let mut f = [0u8; 512];
    f[0..4].copy_from_slice(b"RRaA");
    f[484..488].copy_from_slice(b"rrAa");
    f[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    f[492..496].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    f[508..512].copy_from_slice(b"RRaA");
    if !write_sector(1, &f) {
        return Err(FsErr::Io);
    }

    let mut fat = [0u8; 512];
    fat[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    fat[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    fat[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    if !write_sector(rsvd, &fat) {
        return Err(FsErr::Io);
    }
    if !write_sector(rsvd + fat_sz, &fat) {
        return Err(FsErr::Io);
    }

    // register layout so allocation helpers work
    *INFO.lock() = Some(FsInfo {
        sec_per_clus: 1,
        rsvc: rsvd,
        num_fats: nfats,
        fat_size: fat_sz,
        root_clus: 2,
        data_start,
        cluster_count,
    });

    let root_sector = data_start;
    let seeds: [&[u8]; 6] = [b"ETC", b"ROOT", b"HOME", b"USR", b"BOOT", b"TMP"];
    let mut slot = 0usize;
    for name in seeds {
        let Some(nc) = alloc_cluster() else { return Err(FsErr::NoSpace) };
        let first = first_sector(nc);
        let zero = [0u8; 512];
        if !write_sector(first, &zero) {
            return Err(FsErr::Io);
        }
        let dot = DirEntry {
            name: [b'.', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' '],
            attr: ATTR_DIRECTORY,
            cluster: nc,
            size: 0,
        };
        let dotdot = DirEntry {
            name: [b'.', b'.', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' '],
            attr: ATTR_DIRECTORY,
            cluster: 0,
            size: 0,
        };
        write_entry_at(first, 0, &dot)?;
        write_entry_at(first, 1, &dotdot)?;
        let mut n83 = [b' '; 11];
        for (k, &ch) in name.iter().enumerate() {
            n83[k] = ch;
        }
        let e = DirEntry {
            name: n83,
            attr: ATTR_DIRECTORY,
            cluster: nc,
            size: 0,
        };
        write_entry_at(root_sector, slot, &e)?;
        slot += 1;
    }

    mount_at(base)?;
    Ok(())
}
