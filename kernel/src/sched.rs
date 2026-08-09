use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use x86_64::instructions::interrupts::without_interrupts;

use crate::interrupts;

const STACK_BYTES: usize = 64 * 1024;
const MAX_TASKS: usize = 24;

#[derive(Clone, Copy, PartialEq)]
enum TaskState {
    Ready,
    Sleeping(u64),
}

struct Task {
    #[allow(dead_code)]
    name: &'static str,
    saved_rsp: usize,
    #[allow(dead_code)]
    stack: Option<Box<[u8]>>,
    state: TaskState,
    switches: u64,
}

struct SchedulerInner {
    tasks: Vec<Task>,
    ready: VecDeque<u32>,
    sleepers: Vec<(u32, u64)>,
    current: u32,
}

static SCHED: Mutex<SchedulerInner> = Mutex::new(SchedulerInner {
    tasks: Vec::new(),
    ready: VecDeque::new(),
    sleepers: Vec::new(),
    current: 0,
});
static INIT_DONE: AtomicBool = AtomicBool::new(false);
static TASK_SWITCHES: AtomicU64 = AtomicU64::new(0);

pub fn active() -> bool {
    INIT_DONE.load(Ordering::Acquire)
}

pub fn switches() -> u64 {
    TASK_SWITCHES.load(Ordering::Relaxed)
}

pub fn init() {
    without_interrupts(|| {
        let mut s = SCHED.lock();
        s.tasks.push(Task {
            name: "kernel",
            saved_rsp: 0,
            stack: None,
            state: TaskState::Ready,
            switches: 0,
        });
        s.ready.push_back(0);
        INIT_DONE.store(true, Ordering::Release);
        crate::boot::ok(format_args!(
            "Scheduler: round-robin, 1 task (kernel), {} KB stacks",
            STACK_BYTES / 1024
        ));
    });
}

pub fn spawn(name: &'static str, entry: fn()) -> u32 {
    without_interrupts(|| {
        let mut s = SCHED.lock();
        if s.tasks.len() >= MAX_TASKS {
            return u32::MAX;
        }
        let id = s.tasks.len() as u32;
        let stack: Box<[u8]> = alloc::vec![0u8; STACK_BYTES].into_boxed_slice();
        let top = stack.as_ptr() as usize + STACK_BYTES;
        let sp = (top & !15) - 7 * 8;
        unsafe {
            let p = sp as *mut usize;
            p.add(6).write(entry as usize);
        }
        s.tasks.push(Task {
            name,
            saved_rsp: sp,
            stack: Some(stack),
            state: TaskState::Ready,
            switches: 0,
        });
        s.ready.push_back(id);
        id
    })
}

core::arch::global_asm!(
    ".global context_switch",
    "context_switch:",
    "push r15",
    "push r14",
    "push r13",
    "push r12",
    "push rbx",
    "push rbp",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop rbp",
    "pop rbx",
    "pop r12",
    "pop r13",
    "pop r14",
    "pop r15",
    "ret"
);

unsafe extern "C" {
    fn context_switch(current_rsp_out: *mut usize, next_rsp: usize);
}

/// Called from the timer IRQ. Moves the current task to the back of the ready
/// queue and switches to the next ready task, if any.
pub fn schedule() {
    if !INIT_DONE.load(Ordering::Acquire) {
        return;
    }
    without_interrupts(|| {
        let now = interrupts::ticks();
        let mut s = SCHED.lock();

        let mut i = 0;
        while i < s.sleepers.len() {
            if s.sleepers[i].1 <= now {
                let (id, _) = s.sleepers[i];
                s.sleepers.swap_remove(i);
                s.tasks[id as usize].state = TaskState::Ready;
                s.ready.push_back(id);
            } else {
                i += 1;
            }
        }

        let cur = s.current;
        if s.tasks[cur as usize].state == TaskState::Ready {
            s.ready.push_back(cur);
        }

        let out_ptr = &mut s.tasks[cur as usize].saved_rsp as *mut usize;
        let mut next_id: Option<u32> = None;
        while let Some(id) = s.ready.pop_front() {
            if id == cur || s.tasks[id as usize].state != TaskState::Ready {
                continue;
            }
            next_id = Some(id);
            break;
        }
        let next_rsp = match next_id {
            Some(id) => {
                s.current = id;
                TASK_SWITCHES.fetch_add(1, Ordering::Relaxed);
                s.tasks[cur as usize].switches += 1;
                s.tasks[id as usize].switches += 1;
                s.tasks[id as usize].saved_rsp
            }
            None => 0,
        };
        drop(s);
        if let Some(_id) = next_id {
            unsafe {
                context_switch(out_ptr, next_rsp);
            }
        }
    });
}

/// Blocks the calling task until `target` ticks. Rewritten from the old
/// hlt-loop: the timer interrupt now triggers schedule(), which hands the CPU
/// to other tasks while we wait.
pub fn sleep_until(target: u64) {
    without_interrupts(|| {
        let mut s = SCHED.lock();
        let cur = s.current;
        s.tasks[cur as usize].state = TaskState::Sleeping(target);
        s.sleepers.push((cur, target));
    });
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
        if interrupts::ticks() >= target {
            break;
        }
    }
}