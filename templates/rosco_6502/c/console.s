; The two system calls cc65's C library needs, wired to the DUART.
;
; printf, puts and putchar all end up in write(); getchar, fgets and scanf all
; end up in read(). Nothing else of the runtime is board-specific.
;
; The firmware has print routines of its own behind the ROM jump table in
; inc/defines.inc, but where they sit has moved between firmware revisions,
; so a program that calls them prints nothing on a board with an older ROM.
; The XR68C681 registers are hardware and do not move, which is why these
; routines talk to it directly.

        .include "defines.inc"

        .import popax, popptr1
        .importzp ptr1, ptr2, ptr3

        .export _write, _read

        .segment "CODE"

; int __fastcall__ write (int fd, const void* buf, unsigned count);
;
; Every stream goes to the console, so the descriptor is ignored.
.proc _write
        sta     ptr2                    ; bytes still to send
        stx     ptr2+1
        sta     ptr3                    ; and the count to return
        stx     ptr3+1
        jsr     popptr1                 ; buffer
        jsr     popax                   ; descriptor, ignored

next:   lda     ptr2
        ora     ptr2+1
        beq     done
        ldy     #0
        lda     (ptr1),y
        cmp     #$0A                    ; C ends a line with one newline;
        bne     send                    ; a terminal wants both characters
        lda     #$0D
        jsr     putchar
        lda     #$0A
send:   jsr     putchar

        inc     ptr1
        bne     count
        inc     ptr1+1
count:  lda     ptr2
        bne     low
        dec     ptr2+1
low:    dec     ptr2
        bra     next

done:   lda     ptr3
        ldx     ptr3+1
        rts
.endproc

; int __fastcall__ read (int fd, void* buf, unsigned count);
;
; Reads a line: characters are echoed as they arrive and Enter ends the call,
; which is what the C library expects of a terminal.
.proc _read
        sta     ptr2                    ; room left in the buffer
        stx     ptr2+1
        stz     ptr3                    ; characters read so far
        stz     ptr3+1
        jsr     popptr1                 ; buffer
        jsr     popax                   ; descriptor, ignored

next:   lda     ptr2
        ora     ptr2+1
        beq     done
        jsr     getchar
        cmp     #$0D                    ; Enter arrives as a carriage return
        bne     store
        lda     #$0A
store:  ldy     #0
        sta     (ptr1),y
        cmp     #$0A
        php                             ; was that the end of the line?
        bne     echo
        lda     #$0D                    ; echo the line break in full
        jsr     putchar
        lda     #$0A
echo:   jsr     putchar

        inc     ptr1
        bne     count
        inc     ptr1+1
count:  inc     ptr3
        bne     room
        inc     ptr3+1
room:   lda     ptr2
        bne     low
        dec     ptr2+1
low:    dec     ptr2

        plp
        bne     next

done:   lda     ptr3
        ldx     ptr3+1
        rts
.endproc

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

; Waits for a character from the console and returns it in A.
.proc getchar
wait:   lda     DUA_SRA
        and     #DUA_SR_RXRDY
        beq     wait
        lda     DUA_RBA
        rts
.endproc
