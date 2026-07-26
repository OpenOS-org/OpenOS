; hello.asm — User-space program using Channel IPC.
;
; Demonstrates the new Channel-based syscall interface:
;   1. channel_create() → (handle_a, handle_b)
;   2. console_write("Hello from initrd!\n")  (debug output)
;   3. process_exit(0)
;
; Syscall convention (INTERFACE.md §3.1):
;   RAX = syscall number
;   RDI = arg0
;   RSI = arg1
;   RDX = arg2
;   R10 = arg3
;   R8  = arg4
;   R9  = arg5
;   Return: RAX = positive (success) or negative (error)

BITS 64
SECTION .text

global _start

_start:
    ; --- channel_create() ---
    mov rax, 0x01               ; SYS_CHANNEL_CREATE
    syscall
    ; RAX = handle_a (or negative error)
    ; We ignore the result for now.

    ; --- console_write(msg, len) ---
    lea rdi, [rel msg]          ; arg0: buffer pointer
    mov rsi, msg_len            ; arg1: length
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    ; --- process_exit(0) ---
    mov rdi, 0                  ; arg0: exit code
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall

    ; Should never reach here
    jmp $

SECTION .rodata
msg:     db "Hello from initrd!", 10
msg_len equ $ - msg
