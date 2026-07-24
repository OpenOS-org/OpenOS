/* OpenOS Kernel Linker Script */
ENTRY(_start)

KERNEL_OFFSET = 0xFFFFFFFF80000000;

SECTIONS
{
    . = 0xFFFFFFFF80100000;

    .text ALIGN(4K) : AT(ADDR(.text) - KERNEL_OFFSET)
    {
        __text_start = .;
        *(.text.boot)
        *(.text .text.*)
        __text_end = .;
    }

    /* .rodata merged with .data (RW) to avoid page-sharing between
       separate LOAD segments — bootloader 0.9 panics with
       PageAlreadyMapped when two segments overlap on one page.
       Code is still protected by NX bit in page tables. */
    .rodata ALIGN(4K) : AT(ADDR(.rodata) - KERNEL_OFFSET)
    {
        __rodata_start = .;
        *(.rodata .rodata.*)
        __rodata_end = .;
    }

    .data ALIGN(4K) : AT(ADDR(.data) - KERNEL_OFFSET)
    {
        __data_start = .;
        *(.data .data.*)
        __data_end = .;
    }

    .bss ALIGN(4K) : AT(ADDR(.bss) - KERNEL_OFFSET)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    __bss_size = (__bss_end - __bss_start) / 8;

    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame)
        *(.note.*)
    }
}
