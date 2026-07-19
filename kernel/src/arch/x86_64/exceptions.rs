//! Minimal IDT: every CPU exception reports over serial and parks,
//! mirroring the aarch64 vector table.

use core::arch::asm;
use core::mem::size_of;

#[repr(C)]
pub struct StackFrame {
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Entry {
    off_lo: u16,
    selector: u16,
    ist: u8,
    attr: u8,
    off_mid: u16,
    off_hi: u32,
    zero: u32,
}

impl Entry {
    const EMPTY: Self = Self {
        off_lo: 0,
        selector: 0,
        ist: 0,
        attr: 0,
        off_mid: 0,
        off_hi: 0,
        zero: 0,
    };

    fn set(&mut self, handler: u64, cs: u16) {
        self.off_lo = handler as u16;
        self.selector = cs;
        self.attr = 0x8E; // present, interrupt gate
        self.off_mid = (handler >> 16) as u16;
        self.off_hi = (handler >> 32) as u32;
    }
}

#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

static mut IDT: [Entry; 256] = [Entry::EMPTY; 256];

fn report(vector: u64, error: u64, frame: &StackFrame) -> ! {
    unsafe { crate::logger::force_unlock() };
    let rip = frame.rip;
    kprintln!("\n*** EXCEPTION vector={vector} error={error:#x} RIP={rip:#x} ***");
    super::park()
}

macro_rules! handler {
    ($vec:literal) => {{
        extern "x86-interrupt" fn h(frame: StackFrame) {
            report($vec, 0, &frame);
        }
        h as u64
    }};
    ($vec:literal, err) => {{
        extern "x86-interrupt" fn h(frame: StackFrame, error: u64) {
            report($vec, error, &frame);
        }
        h as u64
    }};
}

pub fn install() {
    let cs: u16;
    unsafe { asm!("mov {0:x}, cs", out(reg) cs) };

    let handlers: [u64; 32] = [
        handler!(0),
        handler!(1),
        handler!(2),
        handler!(3),
        handler!(4),
        handler!(5),
        handler!(6),
        handler!(7),
        handler!(8, err),
        handler!(9),
        handler!(10, err),
        handler!(11, err),
        handler!(12, err),
        handler!(13, err),
        handler!(14, err),
        handler!(15),
        handler!(16),
        handler!(17, err),
        handler!(18),
        handler!(19),
        handler!(20),
        handler!(21, err),
        handler!(22),
        handler!(23),
        handler!(24),
        handler!(25),
        handler!(26),
        handler!(27),
        handler!(28),
        handler!(29),
        handler!(30, err),
        handler!(31),
    ];

    // IRQ gates: LAPIC timer, virtio input lines, and the spurious vector.
    extern "x86-interrupt" fn timer_gate(_f: StackFrame) {
        super::irq::on_timer_irq();
    }
    extern "x86-interrupt" fn input_gate(_f: StackFrame) {
        super::irq::on_input_irq();
    }
    extern "x86-interrupt" fn ipi_gate(_f: StackFrame) {
        super::irq::on_ipi();
    }
    extern "x86-interrupt" fn spurious_gate(_f: StackFrame) {
        // No EOI for spurious interrupts.
    }

    unsafe {
        let idt = &mut *core::ptr::addr_of_mut!(IDT);
        for (entry, &h) in idt.iter_mut().zip(handlers.iter()) {
            entry.set(h, cs);
        }
        idt[super::apic::VEC_TIMER as usize].set(timer_gate as u64, cs);
        for v in super::apic::VEC_INPUT_BASE..super::apic::VEC_INPUT_BASE + 8 {
            idt[v as usize].set(input_gate as u64, cs);
        }
        idt[super::apic::VEC_IPI as usize].set(ipi_gate as u64, cs);
        idt[0xFF].set(spurious_gate as u64, cs);
        let idtr = Idtr {
            limit: (size_of::<[Entry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{0}]", in(reg) &idtr);
    }
}

/// Load the already-populated IDT on an AP.
pub fn load() {
    unsafe {
        let idt = &raw const IDT;
        let idtr = Idtr {
            limit: (size_of::<[Entry; 256]>() - 1) as u16,
            base: idt as u64,
        };
        asm!("lidt [{0}]", in(reg) &idtr);
    }
}
