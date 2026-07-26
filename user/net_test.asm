; net_test.asm — Network test: send ARP request via SYS_NET_SEND
;
; Sends a raw ARP request frame to test the virtio-net TX path.
; The ARP request asks "who has 10.0.2.2? Tell 10.0.2.15"
; (QEMU's default gateway).

BITS 64
SECTION .text

global _start

_start:
    ; Build ARP request frame on the stack
    ; Ethernet header (14 bytes):
    ;   dst: ff:ff:ff:ff:ff:ff (broadcast)
    ;   src: 52:54:00:12:34:56 (our MAC — QEMU default)
    ;   type: 0x0806 (ARP)

    ; ARP packet (28 bytes):
    ;   hw_type: 0x0001 (Ethernet)
    ;   proto_type: 0x0800 (IPv4)
    ;   hw_len: 6, proto_len: 4
    ;   opcode: 0x0001 (request)
    ;   sender_mac: 52:54:00:12:34:56
    ;   sender_ip: 10.0.2.15 (0x0a00020f)
    ;   target_mac: 00:00:00:00:00:00
    ;   target_ip: 10.0.2.2 (0x0a000202)

    ; Build frame on stack (14 + 28 = 42 bytes)
    sub rsp, 64

    ; Ethernet header
    mov dword [rsp+0], 0xffffffff    ; dst mac (first 4 bytes)
    mov word [rsp+4], 0xffff         ; dst mac (last 2 bytes)
    mov dword [rsp+6], 0x00005452    ; src mac (52:54:00:12)
    mov word [rsp+10], 0x5634        ; src mac (34:56)
    mov word [rsp+12], 0x0608        ; type = ARP (0x0806 in LE)

    ; ARP packet
    mov word [rsp+14], 0x0100        ; hw_type = 1
    mov word [rsp+16], 0x0008        ; proto_type = 0x0800
    mov byte [rsp+18], 6             ; hw_len
    mov byte [rsp+19], 4             ; proto_len
    mov word [rsp+20], 0x0100        ; opcode = request
    mov dword [rsp+22], 0x00005452   ; sender mac
    mov word [rsp+26], 0x5634
    mov dword [rsp+28], 0x0f02000a   ; sender IP = 10.0.2.15
    mov dword [rsp+32], 0x00000000   ; target mac
    mov word [rsp+36], 0x0000
    mov dword [rsp+38], 0x0202000a   ; target IP = 10.0.2.2

    ; sys_net_send(buf, len)
    mov rdi, rsp          ; buf pointer
    mov rsi, 42           ; frame length
    mov rax, 0xFD         ; SYS_NET_SEND
    syscall

    ; Write result to console
    lea rdi, [rel sent_msg]
    mov rsi, sent_len
    mov rax, 0xF0         ; SYS_CONSOLE_WRITE
    syscall

    ; Exit
    mov rdi, 0
    mov rax, 0x32
    syscall
    jmp $

SECTION .rodata
sent_msg: db "ARP request sent!", 10
sent_len equ $ - sent_msg
