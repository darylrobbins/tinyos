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
    VEC 5
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
