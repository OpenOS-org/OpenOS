; hello.asm — Minimal user-space program for OpenOS.
;
; Prints "Hello from initrd!\n" via SYS_WRITE, then exits via SYS_EXIT.
; Assembled as a position-independent ELF64 executable.
;
; Syscall convention:
;   RAX = syscall number
;   RDI = arg1
;   RSI = arg2
;   RDX = arg3

BITS 64
SECTION .text

global _start

_start:
    ; SYS_WRITE(1): write(stdout, msg, msg_len)
    lea rdi, [rel msg]      ; arg1: buffer pointer
    mov rsi, msg_len         ; arg2: length
    mov rax, 1               ; syscall: SYS_WRITE
    syscall

    ; SYS_EXIT(3): exit(0)
    mov rdi, 0               ; arg1: exit code
    mov rax, 3               ; syscall: SYS_EXIT
    syscall

    ; Should never reach here
    jmp $

SECTION .rodata
msg:     db "Hello from initrd!", 10
msg_len equ $ - msg
