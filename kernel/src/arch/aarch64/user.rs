//! EL0 entry/exit and the lower-EL trap path.
//!
//! A user thread is a normal scheduler thread whose entry `eret`s to EL0.
//! Traps land on SP_EL1 — wherever this thread's kernel stack pointer was at
//! eret time — so syscalls run as ordinary Rust on the thread's own kernel
//! stack and may block/yield like any kernel thread. The full register file
//! (GPRs + SIMD: kernel Rust and user code both use vector registers) is
//! saved in a TrapFrame on that stack.
//!
//! Preemption exists only here: entering EL0 arms a timer slice, and the
//! lower-EL IRQ vector may reschedule — safe because user code holds no
//! kernel locks. EL1 stays cooperative.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};

use super::{paging, timer};

/// EL0 timer slice before a forced reschedule.
const SLICE_US: u64 = 10_000;

/// Saved register file for a lower-EL trap. Layout is load-bearing: the asm
/// in this file uses fixed offsets.
#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 30], // x0..x29           0x000
    pub x30: u64,     //                   0x0F0
    pub sp_el0: u64,  //                   0x0F8
    pub elr: u64,     //                   0x100
    pub spsr: u64,    //                   0x108
    pub fpsr: u64,    //                   0x110
    pub fpcr: u64,    //                   0x118
    pub q: [u128; 32], // q0..q31          0x120..0x320
}

global_asm!(
    r#"
// Lower-EL (EL0) synchronous exception: save everything, hand the frame to
// Rust. Frame size 0x320 must match TrapFrame.
.global __user_sync
__user_sync:
    sub sp, sp, #0x320
    stp x0, x1, [sp, #0x000]
    stp x2, x3, [sp, #0x010]
    stp x4, x5, [sp, #0x020]
    stp x6, x7, [sp, #0x030]
    stp x8, x9, [sp, #0x040]
    stp x10, x11, [sp, #0x050]
    stp x12, x13, [sp, #0x060]
    stp x14, x15, [sp, #0x070]
    stp x16, x17, [sp, #0x080]
    stp x18, x19, [sp, #0x090]
    stp x20, x21, [sp, #0x0A0]
    stp x22, x23, [sp, #0x0B0]
    stp x24, x25, [sp, #0x0C0]
    stp x26, x27, [sp, #0x0D0]
    stp x28, x29, [sp, #0x0E0]
    mrs x0, sp_el0
    stp x30, x0, [sp, #0x0F0]
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x0, x1, [sp, #0x100]
    mrs x0, fpsr
    mrs x1, fpcr
    stp x0, x1, [sp, #0x110]
    stp q0, q1, [sp, #0x120]
    stp q2, q3, [sp, #0x140]
    stp q4, q5, [sp, #0x160]
    stp q6, q7, [sp, #0x180]
    stp q8, q9, [sp, #0x1A0]
    stp q10, q11, [sp, #0x1C0]
    stp q12, q13, [sp, #0x1E0]
    stp q14, q15, [sp, #0x200]
    stp q16, q17, [sp, #0x220]
    stp q18, q19, [sp, #0x240]
    stp q20, q21, [sp, #0x260]
    stp q22, q23, [sp, #0x280]
    stp q24, q25, [sp, #0x2A0]
    stp q26, q27, [sp, #0x2C0]
    stp q28, q29, [sp, #0x2E0]
    stp q30, q31, [sp, #0x300]
    mov x0, sp
    bl user_sync_entry
    b __user_return

.global __user_irq
__user_irq:
    sub sp, sp, #0x320
    stp x0, x1, [sp, #0x000]
    stp x2, x3, [sp, #0x010]
    stp x4, x5, [sp, #0x020]
    stp x6, x7, [sp, #0x030]
    stp x8, x9, [sp, #0x040]
    stp x10, x11, [sp, #0x050]
    stp x12, x13, [sp, #0x060]
    stp x14, x15, [sp, #0x070]
    stp x16, x17, [sp, #0x080]
    stp x18, x19, [sp, #0x090]
    stp x20, x21, [sp, #0x0A0]
    stp x22, x23, [sp, #0x0B0]
    stp x24, x25, [sp, #0x0C0]
    stp x26, x27, [sp, #0x0D0]
    stp x28, x29, [sp, #0x0E0]
    mrs x0, sp_el0
    stp x30, x0, [sp, #0x0F0]
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x0, x1, [sp, #0x100]
    mrs x0, fpsr
    mrs x1, fpcr
    stp x0, x1, [sp, #0x110]
    stp q0, q1, [sp, #0x120]
    stp q2, q3, [sp, #0x140]
    stp q4, q5, [sp, #0x160]
    stp q6, q7, [sp, #0x180]
    stp q8, q9, [sp, #0x1A0]
    stp q10, q11, [sp, #0x1C0]
    stp q12, q13, [sp, #0x1E0]
    stp q14, q15, [sp, #0x200]
    stp q16, q17, [sp, #0x220]
    stp q18, q19, [sp, #0x240]
    stp q20, q21, [sp, #0x260]
    stp q22, q23, [sp, #0x280]
    stp q24, q25, [sp, #0x2A0]
    stp q26, q27, [sp, #0x2C0]
    stp q28, q29, [sp, #0x2E0]
    stp q30, q31, [sp, #0x300]
    mov x0, sp
    bl user_irq_entry
    b __user_return

// Restore the frame and return to EL0. Also the tail of every trap.
__user_return:
    ldp q0, q1, [sp, #0x120]
    ldp q2, q3, [sp, #0x140]
    ldp q4, q5, [sp, #0x160]
    ldp q6, q7, [sp, #0x180]
    ldp q8, q9, [sp, #0x1A0]
    ldp q10, q11, [sp, #0x1C0]
    ldp q12, q13, [sp, #0x1E0]
    ldp q14, q15, [sp, #0x200]
    ldp q16, q17, [sp, #0x220]
    ldp q18, q19, [sp, #0x240]
    ldp q20, q21, [sp, #0x260]
    ldp q22, q23, [sp, #0x280]
    ldp q24, q25, [sp, #0x2A0]
    ldp q26, q27, [sp, #0x2C0]
    ldp q28, q29, [sp, #0x2E0]
    ldp q30, q31, [sp, #0x300]
    ldp x0, x1, [sp, #0x110]
    msr fpsr, x0
    msr fpcr, x1
    ldp x0, x1, [sp, #0x100]
    msr elr_el1, x0
    msr spsr_el1, x1
    ldp x30, x0, [sp, #0x0F0]
    msr sp_el0, x0
    ldp x2, x3, [sp, #0x010]
    ldp x4, x5, [sp, #0x020]
    ldp x6, x7, [sp, #0x030]
    ldp x8, x9, [sp, #0x040]
    ldp x10, x11, [sp, #0x050]
    ldp x12, x13, [sp, #0x060]
    ldp x14, x15, [sp, #0x070]
    ldp x16, x17, [sp, #0x080]
    ldp x18, x19, [sp, #0x090]
    ldp x20, x21, [sp, #0x0A0]
    ldp x22, x23, [sp, #0x0B0]
    ldp x24, x25, [sp, #0x0C0]
    ldp x26, x27, [sp, #0x0D0]
    ldp x28, x29, [sp, #0x0E0]
    ldp x0, x1, [sp, #0x000]
    add sp, sp, #0x320
    eret
"#
);

/// Enter EL0 for the first time on this thread. SP_EL1 stays where it is —
/// that's this thread's kernel stack, where all future traps will land.
///
/// # Safety
/// The thread's address space must be active and (pc, sp) mapped in it.
pub unsafe fn enter_user(pc: u64, sp: u64, arg: u64) -> ! {
    timer::set_timer_us(timer::uptime_us() + SLICE_US);
    unsafe {
        asm!(
            "msr elr_el1, {pc}",
            "msr spsr_el1, xzr", // EL0t, DAIF clear: IRQs enabled in user mode
            "msr sp_el0, {sp}",
            "eret",
            pc = in(reg) pc,
            sp = in(reg) sp,
            in("x0") arg,
            options(noreturn)
        );
    }
}

static CUR_TTBR1: [AtomicU64; super::MAX_CPUS] = [const { AtomicU64::new(0) }; super::MAX_CPUS];

/// Make `ttbr1` (0 = no user space) current on this CPU. Called by the
/// scheduler right before every context switch; skips the register write
/// when unchanged. ASIDs make this flush-free.
pub fn activate(ttbr1: u64) {
    let want = if ttbr1 == 0 { paging::null_ttbr1() } else { ttbr1 };
    if CUR_TTBR1[super::cpu_id()].swap(want, Ordering::Relaxed) != want {
        unsafe { asm!("msr ttbr1_el1, {0}", "isb", in(reg) want) };
    }
}

/// Common tail of every lower-EL trap, on the way back to EL0: honor a
/// pending kill, then re-arm the preemption slice.
fn finish_user_trap() {
    let me = crate::sched::current();
    if me.kill_pending.load(Ordering::Acquire) {
        crate::obj::syscall::exit_current(-9);
    }
    timer::set_timer_us(timer::uptime_us() + SLICE_US);
}

#[unsafe(no_mangle)]
extern "C" fn user_sync_entry(frame: &mut TrapFrame) {
    let esr: u64;
    unsafe { asm!("mrs {0}, esr_el1", out(reg) esr) };
    let ec = (esr >> 26) & 0x3F;
    if ec == 0x15 {
        // SVC64: x8 = sysno, x0-x5 args; returns x0 = status, x1 = value.
        let args = [frame.x[0], frame.x[1], frame.x[2], frame.x[3], frame.x[4], frame.x[5]];
        let (status, value) = crate::obj::syscall::dispatch(frame.x[8], args);
        frame.x[0] = status as u64;
        frame.x[1] = value;
    } else {
        let far: u64;
        unsafe { asm!("mrs {0}, far_el1", out(reg) far) };
        kprintln!(
            "tinyos: user fault on thread {}: EC={ec:#x} ESR={esr:#x} ELR={:#x} FAR={far:#x}",
            crate::sched::current().id,
            frame.elr
        );
        crate::obj::syscall::exit_current(-1);
    }
    finish_user_trap();
}

#[unsafe(no_mangle)]
extern "C" fn user_irq_entry(_frame: &mut TrapFrame) {
    super::irq::handle_pending();
    crate::sched::preempt_from_user();
    finish_user_trap();
}
