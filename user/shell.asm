; shell.asm — Simple interactive shell for OpenOS.
;
; Prints a prompt, reads user input via SYS_CONSOLE_READ, and dispatches
; built-in commands ("exit", "help"). Unknown commands produce a message.
;
; If SYS_CONSOLE_READ returns 0 (not yet fully implemented), the shell
; prints a fallback message and exits cleanly so its structure can be
; verified.

BITS 64
SECTION .text

global _start

; ─────────────────── Shell entry ───────────────────

_start:
    ; Print welcome banner
    lea rdi, [rel banner]
    mov rsi, banner_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

.loop:
    ; Print prompt "openos> "
    lea rdi, [rel prompt]
    mov rsi, prompt_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    ; Read user input into input_buf
    lea rdi, [rel input_buf]    ; arg0: buffer pointer
    mov rsi, input_buf_size     ; arg1: buffer length
    mov rdx, 1                  ; arg2: flags (blocking)
    mov rax, 0xF4               ; SYS_CONSOLE_READ
    syscall

    ; RAX = bytes read (0 if not implemented, negative on error)
    cmp rax, 0
    jle .read_failed

    ; Save byte count
    mov rcx, rax

    ; Null-terminate the input for string comparison
    lea rbx, [rel input_buf]
    mov byte [rbx + rcx], 0

    ; Strip trailing newline if present
    cmp rcx, 0
    je .check_commands
    dec rcx
    cmp byte [rbx + rcx], 10   ; '\n'
    jne .restore_len
    mov byte [rbx + rcx], 0    ; remove newline
    jmp .check_commands

.restore_len:
    inc rcx

.check_commands:
    ; Compare input against "exit"
    lea rdi, [rel input_buf]
    lea rsi, [rel cmd_exit]
    call strcmp
    cmp rax, 0
    je .do_exit

    ; Compare input against "help"
    lea rdi, [rel input_buf]
    lea rsi, [rel cmd_help]
    call strcmp
    cmp rax, 0
    je .do_help

    ; Unknown command
    lea rdi, [rel msg_unknown]
    mov rsi, msg_unknown_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    jmp .loop

.do_exit:
    lea rdi, [rel msg_bye]
    mov rsi, msg_bye_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    mov rdi, 0                  ; exit status 0
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall
    jmp $

.do_help:
    lea rdi, [rel msg_help]
    mov rsi, msg_help_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    jmp .loop

.read_failed:
    ; SYS_CONSOLE_READ returned 0 or negative (not implemented).
    ; Print fallback message and exit.
    lea rdi, [rel msg_no_input]
    mov rsi, msg_no_input_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    mov rdi, 0
    mov rax, 0x32               ; SYS_PROCESS_EXIT
    syscall
    jmp $

; ─────────────────── strcmp ───────────────────
; Compare two null-terminated strings.
; Args: RDI = str1, RSI = str2
; Returns: RAX = 0 if equal, non-zero otherwise.
; Clobbers: RCX, R8, R9
strcmp:
    xor rcx, rcx
.loop:
    mov r8b, [rdi + rcx]
    mov r9b, [rsi + rcx]
    cmp r8b, r9b
    jne .not_equal
    test r8b, r8b               ; both null?
    jz .equal
    inc rcx
    jmp .loop
.equal:
    xor rax, rax
    ret
.not_equal:
    mov rax, 1
    ret

SECTION .bss
input_buf:      resb 128
input_buf_size  equ 127         ; leave room for null terminator

SECTION .rodata
banner:         db "OpenOS Shell v0.1", 10
banner_len      equ $ - banner

prompt:         db "openos> "
prompt_len      equ $ - prompt

cmd_exit:       db "exit", 0
cmd_help:       db "help", 0

msg_bye:        db "Goodbye!", 10
msg_bye_len     equ $ - msg_bye

msg_unknown:    db "unknown command", 10
msg_unknown_len equ $ - msg_unknown

msg_help:       db "Available commands:", 10
                db "  help  - show this message", 10
                db "  exit  - exit the shell", 10
msg_help_len    equ $ - msg_help

msg_no_input:   db "Console read not available, shell exiting.", 10
msg_no_input_len equ $ - msg_no_input
