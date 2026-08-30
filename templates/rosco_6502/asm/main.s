        .include "defines.inc"

        .import print

; The firmware loads a program at $0800 and jumps to the first byte of it.
; STARTUP is the segment the linker puts there, so _start runs no matter
; what order the other files end up in.
        .segment "STARTUP"

        .global _start
_start:
        lda     #<HELLO
        ldx     #>HELLO
        jsr     print

        ; Returning hands the machine back to the firmware monitor. A real
        ; program would more likely carry on with a loop of its own.
        rts

        .segment "RODATA"

HELLO:  .byte   "Hello, world!", $0D, $0A, 0
