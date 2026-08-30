; Console output for rosco_6502, straight to the DUART.
;
; The firmware has print routines of its own behind the ROM jump table in
; inc/defines.inc, but where they sit has moved between firmware revisions,
; so a program that calls them prints nothing on a board with an older ROM.
; The XR68C681 registers are hardware and do not move, which is why these
; twenty lines are here instead.

        .include "defines.inc"

        .export putchar, print

; Two bytes of the zero page the firmware leaves to us, for the string
; pointer that `print` walks.
STRING          =       USER_ZP_START

        .segment "CODE"

; Sends the character in A to the console.
.proc putchar
        pha
wait:   lda     DUA_SRA
        and     #DUA_SR_TXRDY           ; room in the transmit holding register?
        beq     wait
        pla
        sta     DUA_TBA
        rts
.endproc

; Sends the NUL-terminated string at A/X (low/high) to the console.
.proc print
        sta     STRING
        stx     STRING+1
        ldy     #0
next:   lda     (STRING),y
        beq     done
        jsr     putchar
        iny
        bne     next                    ; strings stop at 255 characters
done:   rts
.endproc
