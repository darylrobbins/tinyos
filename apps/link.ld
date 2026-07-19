/* Fixed-base static ELF for tinyOS apps. Base matches paging::APP_IMAGE_BASE
   (TTBR1 region, 4 MiB in). W^X: the loader maps .text RX, the rest RW. */
ENTRY(_start)

BASE = 0xFFFFFFFC00400000;

SECTIONS
{
    . = BASE;

    .abi : ALIGN(8) {
        KEEP(*(.tinyos_abi))
        KEEP(*(.tinyos_abi.caps))
    }

    .text : ALIGN(4096) {
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /DISCARD/ : {
        *(.comment)
        *(.note .note.*)
        *(.eh_frame .eh_frame_hdr)
        *(.dynsym .dynstr .hash .gnu.hash .dynamic)
    }
}
