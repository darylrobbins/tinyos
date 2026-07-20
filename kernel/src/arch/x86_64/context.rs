//! Callee-saved context + cooperative switch, written for the uefi target's
//! MS x64 ABI (extern "C" args in rcx/rdx; calls need 32 bytes shadow space).

use core::arch::global_asm;

#[repr(C)]
pub struct Context {
    pub rsp: u64, // 0x00
    pub rbx: u64, // 0x08
    pub rbp: u64, // 0x10
    pub r12: u64, // 0x18  (holds the entry fn for new threads)
    pub r13: u64, // 0x20
    pub r14: u64, // 0x28
    pub r15: u64, // 0x30
    pub rdi: u64, // 0x38  (callee-saved in MS ABI)
    pub rsi: u64, // 0x40  (callee-saved in MS ABI)
}

impl Context {
    pub fn empty() -> Self {
        unsafe { core::mem::zeroed() }
    }

    pub fn new(stack_top: u64, entry: fn()) -> Self {
        let mut c = Self::empty();
        // Push the trampoline as the `ret` target of the first switch_to.
        let sp = (stack_top & !0xF) - 8;
        unsafe { (sp as *mut u64).write(thread_trampoline as *const () as u64) };
        c.rsp = sp;
        c.r12 = entry as usize as u64;
        c
    }
}

unsafe extern "C" {
    fn thread_trampoline();
    /// switch_to(old: *mut Context, new: *const Context) — MS ABI: rcx, rdx.
    pub fn switch_to(old: *mut Context, new: *const Context);
}

global_asm!(
    r#"
.global switch_to
switch_to:
    mov [rcx + 0x00], rsp
    mov [rcx + 0x08], rbx
    mov [rcx + 0x10], rbp
    mov [rcx + 0x18], r12
    mov [rcx + 0x20], r13
    mov [rcx + 0x28], r14
    mov [rcx + 0x30], r15
    mov [rcx + 0x38], rdi
    mov [rcx + 0x40], rsi
    mov rsp, [rdx + 0x00]
    mov rbx, [rdx + 0x08]
    mov rbp, [rdx + 0x10]
    mov r12, [rdx + 0x18]
    mov r13, [rdx + 0x20]
    mov r14, [rdx + 0x28]
    mov r15, [rdx + 0x30]
    mov rdi, [rdx + 0x38]
    mov rsi, [rdx + 0x40]
    ret

.global thread_trampoline
thread_trampoline:
    mov rcx, r12
    sub rsp, 40
    call rust_thread_start
"#
);
