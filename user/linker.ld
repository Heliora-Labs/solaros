ENTRY(_start)

SECTIONS
{
    . = 0xffff800010000000;

    .text : {
        *(.text .text.*)
    }

    . = ALIGN(0x1000);
    .rodata : {
        *(.rodata .rodata.*)
    }

    . = ALIGN(0x1000);
    .data : {
        *(.data .data.*)
        *(.data.rel.ro .data.rel.ro.*)
        *(.got .got.plt)
    }

    . = ALIGN(0x1000);
    .bss : {
        *(.bss .bss.*)
    }

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
        *(.comment .comment.*)
    }
}
