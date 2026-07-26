; hello.asm — User-space program demonstrating Channel IPC (Phase 3).
;
; channel_create() returns a real Handle (not a raw channel_id).
; The handle is looked up in the task's handle table for all subsequent
; channel operations.
;
; Syscall convention (INTERFACE.md §3.1):
;   RAX = syscall number, RDI-R9 = args
;   Return: RAX = positive (success) or negative (error)

BITS 64
SECTION .text

global _start

_start:
    ; --- channel_create() ---
    ; Returns handle_a in RAX. handle_b is stored in the task's handle table
    ; but not returned directly (it can be shared via handle_transfer later).
    mov rax, 0x01               ; SYS_CHANNEL_CREATE
    syscall
    ; RAX = handle_a (slot_id | rights | generation)
    mov rbx, rax                ; save handle_a

    ; --- console_write("Hello from initrd!\n") ---
    lea rdi, [rel msg]          ; arg0: buffer pointer
    mov rsi, msg_len            ; arg1: length
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    ; --- process_exit(0) ---
    mov rdi, 0                  ; arg0: exit code
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall

    jmp $

SECTION .rodata
msg:     db "Hello from initrd!", 10
msg_len equ $ - msg
