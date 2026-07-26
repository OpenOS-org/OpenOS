; kb_echo.asm — Keyboard echo test program.
;
; Reads characters from the keyboard via SYS_CONSOLE_READ and
; echoes them back via SYS_CONSOLE_WRITE. This demonstrates
; the keyboard input pipeline working end-to-end.
;
; Usage: Run in QEMU, type characters, see them echoed to serial.

BITS 64
SECTION .text

global _start

_start:
    ; Print welcome message
    lea rdi, [rel welcome]
    mov rsi, welcome_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

.loop:
    ; Read a character from keyboard (blocking)
    lea rdi, [rel char_buf]     ; arg0: buffer pointer
    mov rsi, 1                  ; arg1: read 1 byte
    mov rdx, 1                  ; arg2: flags (bit 0 = blocking)
    mov rax, 0xF4               ; SYS_CONSOLE_READ
    syscall

    ; Check if we got a byte
    cmp rax, 1
    jl .loop                    ; Error or no data, try again

    ; Check for newline (end of line)
    mov al, [rel char_buf]
    cmp al, 10                  ; '\n'
    je .newline

    ; Echo the character
    lea rdi, [rel char_buf]
    mov rsi, 1
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    jmp .loop

.newline:
    ; Print newline to serial
    lea rdi, [rel newline]
    mov rsi, 1
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    ; Print "Echo: " prefix for next line
    lea rdi, [rel echo_prefix]
    mov rsi, echo_prefix_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    jmp .loop

SECTION .bss
char_buf: resb 1

SECTION .rodata
welcome:     db "Keyboard Echo Test - Type something:", 10, 10, "Echo: "
welcome_len equ $ - welcome
echo_prefix: db "Echo: "
echo_prefix_len equ $ - echo_prefix
newline:     db 10
