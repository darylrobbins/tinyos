use core::arch::{asm, global_asm};

// Minimal EL1 vector table: every entry reports the exception over serial and
// parks. No exception is recoverable yet (no interrupts are enabled).
global_asm!(
    r#"
.macro VEC kind
.balign 0x80
    mov x0, #\kind
    mrs x1, esr_el1
    mrs x2, elr_el1
    mrs x3, far_el1
    b aarch64_exception
.endm

.balign 0x800
.global __vector_table
__vector_table:
    VEC 0
    VEC 1
    VEC 2
    VEC 3
    VEC 4
.balign 0x80
    b __irq_stub
    VEC 6
    VEC 7
    VEC 8
    VEC 9
    VEC 10
    VEC 11
    VEC 12
    VEC 13
    VEC 14
    VEC 15
"#
);

global_asm!(
    r#"
// Full caller-saved context save around the Rust IRQ handler.
__irq_stub:
    stp x0, x1, [sp, #-16]!
    stp x2, x3, [sp, #-16]!
    stp x4, x5, [sp, #-16]!
    stp x6, x7, [sp, #-16]!
    stp x8, x9, [sp, #-16]!
    stp x10, x11, [sp, #-16]!
    stp x12, x13, [sp, #-16]!
    stp x14, x15, [sp, #-16]!
    stp x16, x17, [sp, #-16]!
    stp x18, x29, [sp, #-16]!
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x0, x1, [sp, #-16]!
    str x30, [sp, #-16]!
    bl irq_entry
    ldr x30, [sp], #16
    ldp x0, x1, [sp], #16
    msr elr_el1, x0
    msr spsr_el1, x1
    ldp x18, x29, [sp], #16
    ldp x16, x17, [sp], #16
    ldp x14, x15, [sp], #16
    ldp x12, x13, [sp], #16
    ldp x10, x11, [sp], #16
    ldp x8, x9, [sp], #16
    ldp x6, x7, [sp], #16
    ldp x4, x5, [sp], #16
    ldp x2, x3, [sp], #16
    ldp x0, x1, [sp], #16
    eret
"#
);

unsafe extern "C" {
    static __vector_table: u8;
}

pub fn install() {
    unsafe {
        let table = &raw const __vector_table;
        asm!("msr vbar_el1, {0}", "isb", in(reg) table as u64);
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_exception(kind: u64, esr: u64, elr: u64, far: u64) -> ! {
    unsafe { crate::logger::force_unlock() };
    kprintln!(
        "\n*** EXCEPTION vector={kind} ESR={esr:#x} ELR={elr:#x} FAR={far:#x} ***"
    );
    super::park()
}
