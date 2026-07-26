; hello.asm — User-space program demonstrating Channel RPC.
;
; Creates a channel, sends a message via channel_call,
; the kernel's inline console service processes it and replies.
;
; Syscall convention (INTERFACE.md §3.1):
;   RAX = syscall number, RDI-R9 = args
;   Return: RAX = positive (success) or negative (error)

BITS 64
SECTION .text

global _start

_start:
    ; --- channel_create() ---
    mov rax, 0x01               ; SYS_CHANNEL_CREATE
    syscall
    ; RAX = handle_a
    mov rbx, rax                ; save handle_a

    ; --- channel_send(handle_a, msg, len) ---
    mov rdi, rbx                ; arg0: handle
    lea rsi, [rel msg]          ; arg1: message pointer
    mov rdx, msg_len            ; arg2: message length
    mov rax, 0x02               ; SYS_CHANNEL_SEND
    syscall
    ; The kernel's inline console service processes the message:
    ;   - Receives on the peer end
    ;   - Prints "Hello from initrd!" to serial
    ;   - Replies with "OK"

    ; --- process_exit(0) ---
    mov rdi, 0
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall

    jmp $

SECTION .rodata
msg:     db "Hello from initrd!", 10
msg_len equ $ - msg
