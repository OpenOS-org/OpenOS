/* OpenOS Kernel Linker Script */
ENTRY(_start)

KERNEL_OFFSET = 0xFFFFFFFF80000000;

SECTIONS
{
    /* Kernel is loaded at 1MB physical, mapped to higher half */
    . = 0xFFFFFFFF80100000;

    .text ALIGN(4K) : AT(ADDR(.text) - KERNEL_OFFSET)
    {
        __text_start = .;
        *(.text.boot)
        *(.text .text.*)
        __text_end = .;
    }

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
