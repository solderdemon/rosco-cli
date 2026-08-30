; A rosco_6502 firmware: the program the machine starts in.
;
; Nothing has run before this. The CPU takes its reset vector from the top of
; ROM bank 0 and everything else is still cold: the stack pointer means
; nothing, the DUART has not been programmed, and there is no BIOS to call.
; What is below is the whole machine's software.

; XR68C681 DUART, channel A - the console on the board's first serial port.
DUA_MR1A        = $C000         ; mode register 1, then 2, in that order
DUA_SRA         = $C001         ; read: status
DUA_CSRA        = $C001         ; write: clock select, which is the baud rate
DUA_CRA         = $C002         ; write: commands
DUA_RBA         = $C003         ; read: the byte that arrived
DUA_TBA         = $C003         ; write: the byte to send
DUA_ACR         = $C004         ; write: auxiliary control

DUA_SR_RXRDY    = $01           ; SRA: a byte is waiting
DUA_SR_TXRDY    = $04           ; SRA: room in the transmit holding register

        .segment "ZEROPAGE"

STRING: .res    2               ; the string `print` is walking

        .segment "CODE"

reset:
        sei                     ; no interrupt handlers are set up yet
        cld
        ldx     #$ff
        txs                     ; the stack is the page at $0100

        jsr     uart_init

        lda     #<hello
        ldx     #>hello
        jsr     print

; Everything typed at the console comes back, which is as much as a machine
; with one serial port and no operating system can do.
echo:   jsr     getchar
        cmp     #$0D            ; Enter arrives as a carriage return
        bne     send
        jsr     putchar         ; and a terminal wants a line feed as well
        lda     #$0A
send:   jsr     putchar
        bra     echo

; Sets up channel A for 115200 baud, 8 data bits, no parity, one stop bit,
; which is what the other end of the cable is expecting.
uart_init:
        lda     #$20
        sta     DUA_CRA         ; reset the receiver
        lda     #$30
        sta     DUA_CRA         ; reset the transmitter
        lda     #$10
        sta     DUA_CRA         ; point at MR1A
        lda     #$13
        sta     DUA_MR1A        ; 8 data bits, no parity
        lda     #$07
        sta     DUA_MR1A        ; one stop bit; this write lands in MR2A
        stz     DUA_ACR         ; the first of the two baud rate tables
        lda     #$80
        sta     DUA_CRA         ; the receiver reads the extended table
        lda     #$a0
        sta     DUA_CRA         ; and so does the transmitter
        lda     #$88
        sta     DUA_CSRA        ; entry 8 of it, both ways: 115200 baud
        lda     #$05
        sta     DUA_CRA         ; enable the receiver and the transmitter
        rts

; Sends the character in A.
putchar:
        pha
:       lda     DUA_SRA
        and     #DUA_SR_TXRDY
        beq     :-              ; wait for room
        pla
        sta     DUA_TBA
        rts

; Waits for a character and returns it in A.
getchar:
:       lda     DUA_SRA
        and     #DUA_SR_RXRDY
        beq     :-
        lda     DUA_RBA
        rts

; Sends the NUL-terminated string at A/X (low/high).
print:
        sta     STRING
        stx     STRING+1
        ldy     #0
:       lda     (STRING),y
        beq     :+
        jsr     putchar
        iny
        bne     :-              ; strings stop at 255 characters
:       rts

        .segment "RODATA"

hello:  .byte   "Hello from my own firmware!", $0D, $0A
        .byte   "Type something; it comes back.", $0D, $0A, 0

; Nothing enables interrupts, so these only exist because the CPU insists on
; somewhere to go.
        .segment "CODE"
irq:
nmi:    rti

        .segment "VECTORS"

        .word   nmi             ; $FFFA
        .word   reset           ; $FFFC
        .word   irq             ; $FFFE
