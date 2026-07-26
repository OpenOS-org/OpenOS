; console_svc.asm — User-space console service (single-message mode).
;
; Receives ONE message on the channel handle (passed in RDI),
; writes it to serial via SYS_CONSOLE_WRITE, replies with "OK", then exits.
;
; The kernel launches this after sending a message to the channel.
; The service receives it, prints it, replies, and exits cleanly.

BITS 64
SECTION .text

global _start

_start:
    ; RDI = channel handle (end B), passed by kernel
    mov rbx, rdi                ; save handle

    ; --- channel_receive(handle, buf, 256) ---
    ; This blocks until a message arrives.
    mov rdi, rbx                ; arg0: handle
    lea rsi, [rel buf]          ; arg1: buffer
    mov rdx, 256                ; arg2: max length
    mov rax, 0x03               ; SYS_CHANNEL_RECEIVE
    syscall
    ; RAX = bytes received

    ; Save message length
    mov rcx, rax

    ; --- console_write(buf, len) ---
    lea rdi, [rel buf]
    mov rsi, rcx
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    ; --- channel_reply(handle, "OK", 2) ---
    mov rdi, rbx
    lea rsi, [rel ok_msg]
    mov rdx, 2
    mov rax, 0x05               ; SYS_CHANNEL_REPLY
    syscall

    ; --- process_exit(0) ---
    mov rdi, 0
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall

    jmp $

SECTION .bss
buf:    resb 256

SECTION .rodata
ok_msg: db "OK"
