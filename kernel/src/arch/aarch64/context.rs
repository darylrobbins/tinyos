//! Callee-saved register context + cooperative switch. IRQs are masked
//! everywhere outside the idle wait, so switches are never interrupted.

use core::arch::global_asm;

/// AAPCS64 callee-saved state. Field order is baked into the asm below.
#[repr(C)]
pub struct Context {
    pub sp: u64,  // 0x00
    pub x19: u64, // 0x08  (holds the entry fn for new threads)
    pub x20: u64, // 0x10
    pub x21: u64, // 0x18
    pub x22: u64, // 0x20
    pub x23: u64, // 0x28
    pub x24: u64, // 0x30
    pub x25: u64, // 0x38
    pub x26: u64, // 0x40
    pub x27: u64, // 0x48
    pub x28: u64, // 0x50
    pub x29: u64, // 0x58
    pub x30: u64, // 0x60  (resume address; `ret` target)
}

impl Context {
    pub fn empty() -> Self {
        unsafe { core::mem::zeroed() }
    }

    /// A context that, when switched to, calls `sched::rust_thread_start(entry)`
    /// on the given stack.
    pub fn new(stack_top: u64, entry: fn()) -> Self {
        let mut c = Self::empty();
        c.sp = stack_top & !0xF; // AAPCS: 16-byte aligned
        c.x19 = entry as usize as u64;
        c.x30 = thread_trampoline as usize as u64;
        c
    }
}

unsafe extern "C" {
    fn thread_trampoline();
    /// switch_to(old: *mut Context, new: *const Context)
    pub fn switch_to(old: *mut Context, new: *const Context);
}

global_asm!(
    r#"
.global switch_to
switch_to:
    mov x9, sp
    str x9,  [x0, #0x00]
    stp x19, x20, [x0, #0x08]
    stp x21, x22, [x0, #0x18]
    stp x23, x24, [x0, #0x28]
    stp x25, x26, [x0, #0x38]
    stp x27, x28, [x0, #0x48]
    stp x29, x30, [x0, #0x58]
    ldr x9,  [x1, #0x00]
    mov sp, x9
    ldp x19, x20, [x1, #0x08]
    ldp x21, x22, [x1, #0x18]
    ldp x23, x24, [x1, #0x28]
    ldp x25, x26, [x1, #0x38]
    ldp x27, x28, [x1, #0x48]
    ldp x29, x30, [x1, #0x58]
    ret

.global thread_trampoline
thread_trampoline:
    mov x0, x19
    bl rust_thread_start
"#
);
