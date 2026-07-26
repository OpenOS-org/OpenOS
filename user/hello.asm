; hello.asm — User-space program demonstrating Channel IPC.
;
; Phase 2 test: channel_create returns a real channel ID,
; console_write outputs "Hello from initrd!".
;
; Syscall convention (INTERFACE.md §3.1):
;   RAX = syscall number, RDI-R9 = args
;   Return: RAX = positive (success) or negative (error)

BITS 64
SECTION .text

global _start

_start:
    ; --- channel_create() ---
    ; Returns channel_id in RAX (positive = success).
    mov rax, 0x01               ; SYS_CHANNEL_CREATE
    syscall
    ; RAX = channel_id (e.g., 1)
    ; Save it for later use.
    mov rbx, rax                ; rbx = channel_id

    ; --- console_write("Hello from initrd!\n") ---
    lea rdi, [rel msg]          ; arg0: buffer pointer
    mov rsi, msg_len            ; arg1: length
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall
    ; RAX = bytes written (or negative error)

    ; --- process_exit(0) ---
    mov rdi, 0                  ; arg0: exit code
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall

    ; Should never reach here
    jmp $

SECTION .rodata
msg:     db "Hello from initrd!", 10
msg_len equ $ - msg
