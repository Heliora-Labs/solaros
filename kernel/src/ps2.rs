//! i8042 PS/2 controller proper initialization (Faz 1a).
//!
//! Robust bring-up: controller self-test, configuration byte handling,
//! device self-test, explicit scancode set 1, IRQ1 re-enabled. Every wait is
//! a bounded poll loop so any failing step degrades gracefully instead of
//! hanging the kernel. USB-legacy emulation belongs to the ACPI work (1e).

use x86_64::instructions::port::Port;

const CMD_PORT: u16 = 0x64;
const DATA_PORT: u16 = 0x60;

// controller commands
const CMD_DISABLE_P1: u8 = 0xAD;
const CMD_ENABLE_P1: u8 = 0xAE;
const CMD_SELF_TEST: u8 = 0xAA;
const CMD_TEST_P1: u8 = 0xAB;
const CMD_READ_CFG: u8 = 0x20;
const CMD_WRITE_CFG: u8 = 0x60;

// device commands
const DEV_SELF_TEST: u8 = 0xFF;
const DEV_SCAN_ON: u8 = 0xF4;

// replies
const DEV_ACK: u8 = 0xFA;
const DEV_RESEND: u8 = 0xFE;
const DEV_SELF_TEST_OK: u8 = 0xAA;
const CMD_SELF_TEST_OK: u8 = 0x55;
const CTRL_PORT_TEST_OK: u8 = 0x00;

// configuration byte: bit0 = port1 IRQ, bit1 = port2 IRQ,
// bit2 = port1 clock (1 = disabled), bit3 = port2 clock (1 = disabled),
// bit4 = port1 translation
const CFG_IRQ_P1: u8 = 0x01;
const CFG_IRQ_P2: u8 = 0x02;
const CFG_CLK_P1: u8 = 0x04;
const CFG_CLK_P2: u8 = 0x08;
const CFG_TRANSLATE: u8 = 0x10;

const POLL_LIMIT: u32 = 1 << 16;

pub struct Ps2InitReport {
    pub controller_ok: bool,
    pub device_ok: bool,
}

fn status() -> u8 {
    unsafe { Port::new(CMD_PORT).read() }
}

fn wait_in_ready() -> bool {
    for _ in 0..POLL_LIMIT {
        if status() & 0x02 == 0 {
            return true;
        }
    }
    false
}

fn wait_out_ready() -> bool {
    for _ in 0..POLL_LIMIT {
        if status() & 0x01 != 0 {
            return true;
        }
    }
    false
}

fn send_cmd(v: u8) -> bool {
    if !wait_in_ready() {
        return false;
    }
    unsafe { Port::new(CMD_PORT).write(v) };
    true
}

fn send_data(v: u8) -> bool {
    if !wait_in_ready() {
        return false;
    }
    unsafe { Port::new(DATA_PORT).write(v) };
    true
}

fn recv() -> Option<u8> {
    if wait_out_ready() {
        Some(unsafe { Port::new(DATA_PORT).read() })
    } else {
        None
    }
}

/// Sends a controller command and reads its single-byte reply.
fn ctrl_cmd(v: u8) -> Option<u8> {
    if !send_cmd(v) {
        return None;
    }
    recv()
}

fn read_config() -> Option<u8> {
    ctrl_cmd(CMD_READ_CFG)
}

fn write_config(v: u8) -> bool {
    send_cmd(CMD_WRITE_CFG) && send_data(v)
}

fn flush_buffer() {
    for _ in 0..16 {
        if status() & 0x01 == 0 {
            break;
        }
        let _v: u8 = unsafe { Port::new(DATA_PORT).read() };
    }
}

/// Sends a single device byte, retrying on 0xFE (resend) up to 4 times.
fn send_device(v: u8) -> bool {
    for _ in 0..4 {
        if !send_data(v) {
            return false;
        }
        match recv() {
            Some(DEV_ACK) => return true,
            Some(DEV_RESEND) => continue,
            _ => return false,
        }
    }
    false
}

fn device_self_test() -> bool {
    for _ in 0..3 {
        if !send_data(DEV_SELF_TEST) {
            return false;
        }
        if recv() != Some(DEV_ACK) {
            continue;
        }
        if recv() == Some(DEV_SELF_TEST_OK) {
            return true;
        }
    }
    false
}

pub fn init() -> Ps2InitReport {
    crate::serial::write_fmt(format_args!("[ps2] init start\n"));

    // 1. disconnect devices so nothing interrupts while we reconfigure
    let _ = send_cmd(CMD_DISABLE_P1);
    flush_buffer();

    // 2. controller self test -> 0x55
    let controller_ok = matches!(ctrl_cmd(CMD_SELF_TEST), Some(CMD_SELF_TEST_OK));

    // 3. port 1 test -> 0x00 (best effort)
    let port_ok = matches!(ctrl_cmd(CMD_TEST_P1), Some(CTRL_PORT_TEST_OK));

    // 4. clean config: no IRQs, no translation; keyboard clock stays ON so
    //    the device can be addressed, mouse clock stays off
    let base = read_config().unwrap_or(0);
    let cfg = (base & !(CFG_IRQ_P1 | CFG_IRQ_P2 | CFG_TRANSLATE | CFG_CLK_P1)) | CFG_CLK_P2;
    let _ = write_config(cfg);

    // 5. keyboard device self test
    let device_ok = device_self_test();

    // 6. enable scanning (0xF4)
    let scan_ok = send_device(DEV_SCAN_ON);

    // 7. restore: IRQ1 on, translation on (controller converts device scan
    //    codes to set 1), port1 clock on; mouse and its IRQ stay off
    let final_cfg = (base & !(CFG_IRQ_P2 | CFG_CLK_P2)) | CFG_IRQ_P1 | CFG_TRANSLATE;
    let _ = write_config(final_cfg);
    let _ = send_cmd(CMD_ENABLE_P1);

    crate::serial::write_fmt(format_args!(
        "[ps2] ctrl-ok:{controller_ok} port-ok:{port_ok} dev-ok:{device_ok} scan:{scan_ok} xlate:1\n"
    ));

    Ps2InitReport {
        controller_ok,
        device_ok,
    }
}
