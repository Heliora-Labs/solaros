use spin::Mutex;

pub const MAX_USERS: usize = 16;
pub const NAME_CAP: usize = 16;
pub const PASS_CAP: usize = 32;

#[derive(Clone, Copy)]
pub struct User {
    name: [char; NAME_CAP],
    name_len: usize,
    pub uid: u32,
    pub gid: u32,
    pass: [char; PASS_CAP],
    pass_len: usize,
}

impl User {
    const fn new() -> Self {
        User {
            name: ['\0'; NAME_CAP],
            name_len: 0,
            uid: 0,
            gid: 0,
            pass: ['\0'; PASS_CAP],
            pass_len: 0,
        }
    }
}

static USERS: Mutex<[User; MAX_USERS]> = Mutex::new([User::new(); MAX_USERS]);
static USER_COUNT: Mutex<usize> = Mutex::new(0);
static CURRENT: Mutex<usize> = Mutex::new(0);

const PASSWD_FILE: [char; 11] = ['/', 'e', 't', 'c', '/', 'p', 'a', 's', 's', 'w', 'd'];

fn slice_eq(a: &[char], b: &[char]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// Resets the user table to a fresh state with only `root` (uid 0).
pub fn init() {
    {
        let mut us = USERS.lock();
        for u in us.iter_mut() {
            *u = User::new();
        }
    }
    *USER_COUNT.lock() = 0;
    *CURRENT.lock() = 0;
    let root = ['r', 'o', 'o', 't'];
    let _ = add_user_core(&root, 0, 0);
}

fn add_user_core(name: &[char], uid: u32, gid: u32) -> Result<usize, &'static str> {
    let mut us = USERS.lock();
    let count = *USER_COUNT.lock();
    if count >= MAX_USERS {
        return Err("user table is full");
    }
    for i in 0..count {
        if slice_eq(&us[i].name[..us[i].name_len], name) {
            return Err("user already exists");
        }
    }
    let n = name.len().min(NAME_CAP);
    let mut nu = User::new();
    nu.name[..n].copy_from_slice(&name[..n]);
    nu.name_len = n;
    nu.uid = uid;
    nu.gid = gid;
    us[count] = nu;
    *USER_COUNT.lock() = count + 1;
    Ok(count)
}

fn valid_name(name: &[char]) -> bool {
    if name.is_empty() || name.len() > NAME_CAP {
        return false;
    }
    for &c in name {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return false;
        }
    }
    true
}

fn next_uid() -> u32 {
    let us = USERS.lock();
    let count = *USER_COUNT.lock();
    let mut uid = 1000u32;
    'next: loop {
        for i in 0..count {
            if us[i].uid == uid {
                uid += 1;
                continue 'next;
            }
        }
        return uid;
    }
}

pub fn add_user(name: &[char]) -> Result<usize, &'static str> {
    if !valid_name(name) {
        return Err("invalid username (letters, digits, _ - . only)");
    }
    let uid = next_uid();
    let idx = add_user_core(name, uid, uid)?;
    save_to_fs();
    Ok(idx)
}

pub fn count() -> usize {
    *USER_COUNT.lock()
}

pub fn find(name: &[char]) -> Option<usize> {
    let us = USERS.lock();
    let count = *USER_COUNT.lock();
    for i in 0..count {
        if slice_eq(&us[i].name[..us[i].name_len], name) {
            return Some(i);
        }
    }
    None
}

pub fn current_user() -> usize {
    *CURRENT.lock()
}

pub fn set_current(i: usize) {
    let count = *USER_COUNT.lock();
    if i < count {
        *CURRENT.lock() = i;
    }
}

pub fn user_name(i: usize) -> ([char; NAME_CAP], usize) {
    let us = USERS.lock();
    let n = us[i].name_len;
    (us[i].name, n)
}

pub fn user_uid(i: usize) -> u32 {
    let us = USERS.lock();
    us[i].uid
}

pub fn user_gid(i: usize) -> u32 {
    let us = USERS.lock();
    us[i].gid
}

pub fn has_password(i: usize) -> bool {
    USERS.lock()[i].pass_len > 0
}

pub fn check_password(i: usize, pass: &[char]) -> bool {
    let us = USERS.lock();
    if us[i].pass_len == 0 {
        return true;
    }
    slice_eq(&us[i].pass[..us[i].pass_len], pass)
}

pub fn set_password(i: usize, pass: &[char]) -> Result<(), &'static str> {
    if pass.len() > PASS_CAP {
        return Err("password too long");
    }
    {
        let mut us = USERS.lock();
        us[i].pass[..pass.len()].copy_from_slice(pass);
        us[i].pass_len = pass.len();
    }
    save_to_fs();
    Ok(())
}

// ---------- /etc/passwd persistence ----------

fn push_byte(buf: &mut [u8], pos: &mut usize, b: u8) {
    if *pos < buf.len() {
        buf[*pos] = b;
        *pos += 1;
    }
}

fn push_u32(buf: &mut [u8], pos: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut n = 0usize;
    if v == 0 {
        push_byte(buf, pos, b'0');
        return;
    }
    while v > 0 {
        tmp[n] = (b'0' + (v % 10) as u8) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        push_byte(buf, pos, tmp[i]);
    }
}

pub fn save_to_fs() {
    if !crate::fs::mounted() {
        return;
    }
    let us = *USERS.lock();
    let count = *USER_COUNT.lock();
    let mut buf = [0u8; 2048];
    let mut pos = 0usize;
    for i in 0..count {
        for j in 0..us[i].name_len {
            push_byte(&mut buf, &mut pos, (us[i].name[j] as u32) as u8);
        }
        push_byte(&mut buf, &mut pos, b':');
        push_u32(&mut buf, &mut pos, us[i].uid);
        push_byte(&mut buf, &mut pos, b':');
        push_u32(&mut buf, &mut pos, us[i].gid);
        push_byte(&mut buf, &mut pos, b':');
        for j in 0..us[i].pass_len {
            push_byte(&mut buf, &mut pos, (us[i].pass[j] as u32) as u8);
        }
        push_byte(&mut buf, &mut pos, b'\n');
    }
    if pos > 0 {
        let _ = crate::fs::write_file(&PASSWD_FILE, &buf[..pos]);
    }
}

fn parse_u32(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(v)
}

fn bytes_to_chars(s: &[u8], out: &mut [char], out_len: &mut usize) -> bool {
    if s.len() > out.len() {
        return false;
    }
    for (i, &b) in s.iter().enumerate() {
        if b == 0 || b == b':' {
            return false;
        }
        out[i] = b as char;
    }
    *out_len = s.len();
    true
}

pub fn load_from_fs() {
    if !crate::fs::mounted() {
        return;
    }
    let mut buf = [0u8; 2048];
    let len = match crate::fs::read_file(&PASSWD_FILE, &mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    if len == 0 {
        return;
    }

    // parse all lines first, then rebuild the table
    let mut parsed: [( [char; NAME_CAP], usize, u32, u32, [char; PASS_CAP], usize ); MAX_USERS] =
        [([ '\0'; NAME_CAP ], 0, 0, 0, ['\0'; PASS_CAP], 0); MAX_USERS];
    let mut nparsed = 0usize;
    let mut line_start = 0usize;
    while line_start < len {
        let mut line_end = line_start;
        while line_end < len && buf[line_end] != b'\n' {
            line_end += 1;
        }
        let line = &buf[line_start..line_end];

        let mut fields: [&[u8]; 4] = [&[]; 4];
        let mut nfields = 0usize;
        let mut fstart = 0usize;
        for i in 0..=line.len() {
            if i == line.len() || line[i] == b':' {
                if nfields < 4 {
                    fields[nfields] = &line[fstart..i];
                    nfields += 1;
                }
                fstart = i + 1;
            }
        }
        if nfields >= 3 && nparsed < MAX_USERS {
            let mut nm = ['\0'; NAME_CAP];
            let mut nm_len = 0usize;
            let mut p = ['\0'; PASS_CAP];
            let mut p_len = 0usize;
            let uid = parse_u32(fields[1]).unwrap_or(0);
            let gid = parse_u32(fields[2]).unwrap_or(uid);
            if bytes_to_chars(fields[0], &mut nm, &mut nm_len) {
                if nfields >= 4 && bytes_to_chars(fields[3], &mut p, &mut p_len) {
                    parsed[nparsed] = (nm, nm_len, uid, gid, p, p_len);
                    nparsed += 1;
                }
            }
        }
        line_start = line_end + 1;
    }

    // rebuild
    {
        let mut us = USERS.lock();
        let mut count = 0usize;
        let mut has_root = false;
        for i in 0..nparsed {
            let (nm, nm_len, uid, gid, p, p_len) = &parsed[i];
            if slice_eq(&nm[..*nm_len], &['r', 'o', 'o', 't']) {
                has_root = true;
            }
            if count >= MAX_USERS {
                break;
            }
            let n = *nm_len;
            let mut nu = User::new();
            nu.name[..n].copy_from_slice(&nm[..n]);
            nu.name_len = n;
            nu.uid = *uid;
            nu.gid = *gid;
            nu.pass[..*p_len].copy_from_slice(&p[..*p_len]);
            nu.pass_len = *p_len;
            us[count] = nu;
            count += 1;
        }
        if !has_root && count < MAX_USERS {
            let mut nu = User::new();
            let root = ['r', 'o', 'o', 't'];
            nu.name[..4].copy_from_slice(&root);
            nu.name_len = 4;
            us[count] = nu;
            count += 1;
        }
        *USER_COUNT.lock() = count;
        drop(us);
    }
    save_to_fs();
}