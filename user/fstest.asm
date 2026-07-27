; fstest.asm — Minimal filesystem test.
;
; Tests: create, write, close, re-open, read, stat, mkdir.

BITS 64
SECTION .text

global _start

_start:
    ; Print banner
    lea rdi, [rel banner]
    mov rsi, banner_len
    mov rax, 0xF0               ; SYS_CONSOLE_WRITE
    syscall

    ; ─── 1. Create and write to a file ───
    ; sys_fs_open(name_ptr, name_len, flags: 1=write/create)
    lea rdi, [rel fname]
    mov rsi, fname_len
    mov rdx, 1
    mov rax, 0xF7               ; SYS_FS_OPEN
    syscall
    mov r12, rax                ; save fd
    cmp rax, 0
    jl .fail_create
    lea rdi, [rel ok_open]
    mov rsi, ok_open_len
    jmp .p1
.fail_create:
    lea rdi, [rel fail_open]
    mov rsi, fail_open_len
.p1:
    mov rax, 0xF0
    syscall

    ; ─── 2. Write data ───
    mov rdi, r12
    lea rsi, [rel testdata]
    mov rdx, testdata_len
    mov rax, 0xF9               ; SYS_FS_WRITE
    syscall
    cmp rax, testdata_len
    jne .fail_write
    lea rdi, [rel ok_write]
    mov rsi, ok_write_len
    jmp .p2
.fail_write:
    lea rdi, [rel fail_write]
    mov rsi, fail_write_len
.p2:
    mov rax, 0xF0
    syscall

    ; ─── 3. Close ───
    mov rdi, r12
    mov rax, 0xFA               ; SYS_FS_CLOSE
    syscall

    ; ─── 4. Re-open for reading ───
    lea rdi, [rel fname]
    mov rsi, fname_len
    mov rdx, 0                  ; flags: 0=read-only
    mov rax, 0xF7               ; SYS_FS_OPEN
    syscall
    mov r12, rax                ; save new fd
    cmp rax, 0
    jl .fail_reopen
    lea rdi, [rel ok_reopen]
    mov rsi, ok_reopen_len
    jmp .p3
.fail_reopen:
    lea rdi, [rel fail_reopen]
    mov rsi, fail_reopen_len
.p3:
    mov rax, 0xF0
    syscall

    ; ─── 5. Read data ───
    mov rdi, r12
    lea rsi, [rel readbuf]
    mov rdx, 64
    mov rax, 0xF8               ; SYS_FS_READ
    syscall
    cmp rax, testdata_len
    jne .fail_read
    lea rdi, [rel ok_read]
    mov rsi, ok_read_len
    jmp .p4
.fail_read:
    lea rdi, [rel fail_read]
    mov rsi, fail_read_len
.p4:
    mov rax, 0xF0
    syscall

    ; ─── 6. Stat ───
    mov rdi, r12
    lea rsi, [rel statbuf]
    mov rax, 0xC7               ; SYS_FSTAT
    syscall
    cmp rax, 0
    jl .fail_stat
    lea rdi, [rel ok_stat]
    mov rsi, ok_stat_len
    jmp .p5
.fail_stat:
    lea rdi, [rel fail_stat]
    mov rsi, fail_stat_len
.p5:
    mov rax, 0xF0
    syscall

    ; ─── 7. Close ───
    mov rdi, r12
    mov rax, 0xFA               ; SYS_FS_CLOSE
    syscall

    ; ─── 8. Mkdir ───
    lea rdi, [rel dname]
    mov rsi, dname_len
    mov rdx, 0x1FF              ; mode: 0777
    mov rax, 0xC2               ; SYS_FS_MKDIR
    syscall
    cmp rax, 0
    jl .fail_mkdir
    lea rdi, [rel ok_mkdir]
    mov rsi, ok_mkdir_len
    jmp .p6
.fail_mkdir:
    lea rdi, [rel fail_mkdir]
    mov rsi, fail_mkdir_len
.p6:
    mov rax, 0xF0
    syscall

    ; ─── 9. Done ───
    lea rdi, [rel done_msg]
    mov rsi, done_len
    mov rax, 0xF0
    syscall
    mov rdi, 0
    mov rax, 0x32
    syscall
    jmp $

SECTION .rodata
banner:     db "=== Filesystem Test ===", 10
banner_len  equ $ - banner
fname:      db "/testfile.txt"
fname_len   equ $ - fname
dname:      db "/testdir"
dname_len   equ $ - dname
testdata:   db "Hello from filesystem test!"
testdata_len equ $ - testdata
ok_open:    db "  PASS: open/create", 10
ok_open_len equ $ - ok_open
fail_open:  db "  FAIL: open/create", 10
fail_open_len equ $ - fail_open
ok_write:   db "  PASS: write", 10
ok_write_len equ $ - ok_write
fail_write: db "  FAIL: write", 10
fail_write_len equ $ - fail_write
ok_reopen:  db "  PASS: re-open", 10
ok_reopen_len equ $ - ok_reopen
fail_reopen: db "  FAIL: re-open", 10
fail_reopen_len equ $ - fail_reopen
ok_read:    db "  PASS: read", 10
ok_read_len equ $ - ok_read
fail_read:  db "  FAIL: read", 10
fail_read_len equ $ - fail_read
ok_stat:    db "  PASS: stat", 10
ok_stat_len equ $ - ok_stat
fail_stat:  db "  FAIL: stat", 10
fail_stat_len equ $ - fail_stat
ok_mkdir:   db "  PASS: mkdir", 10
ok_mkdir_len equ $ - ok_mkdir
fail_mkdir: db "  FAIL: mkdir", 10
fail_mkdir_len equ $ - fail_mkdir
done_msg:   db 10, "All tests complete.", 10
done_len    equ $ - done_msg

SECTION .bss
readbuf:    resb 64
statbuf:    resb 128
