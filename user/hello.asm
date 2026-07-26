; hello.asm — User-space program that sends a message via Channel.
;
; The kernel creates a channel and passes handle_a (end A) in RDI.
; The program sends "Hello from initrd!\n" via channel_send, then exits.
;
; The console service (console_svc.elf) is running on end B,
; receives the message, prints it to serial, and replies with "OK".

BITS 64
SECTION .text

global _start

_start:
    ; RDI = handle_a (passed by kernel)
    mov rbx, rdi                ; save handle_a

    ; --- channel_send(handle_a, msg, len) ---
    mov rdi, rbx                ; arg0: handle
    lea rsi, [rel msg]          ; arg1: message pointer
    mov rdx, msg_len            ; arg2: message length
    mov rax, 0x02               ; SYS_CHANNEL_SEND
    syscall
    ; The console service receives this, prints it, replies "OK".

    ; --- process_exit(0) ---
    mov rdi, 0
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall

    jmp $

SECTION .rodata
msg:     db "Hello from initrd!", 10
msg_len equ $ - msg
