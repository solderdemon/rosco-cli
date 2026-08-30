; A rosco_m68k firmware: the program the machine starts in.
;
; Nothing has run before this. The board wires the first eight bytes of ROM to
; address zero while it resets, so the CPU takes its stack pointer and its
; first instruction from the two longs below, and everything else is still
; cold: no vector table, no BIOS, and a DUART that has never been programmed.

; XR68C681 DUART, channel A - the console on the board's first serial port.
; The chip sits on the low half of the bus, so its registers are every other
; byte from $F00001.
DUA_MR1A        equ     $F00001         ; mode register 1, then 2, in that order
DUA_SRA         equ     $F00003         ; read: status
DUA_CSRA        equ     $F00003         ; write: clock select, which is the baud rate
DUA_CRA         equ     $F00005         ; write: commands
DUA_RBA         equ     $F00007         ; read: the byte that arrived
DUA_TBA         equ     $F00007         ; write: the byte to send
DUA_ACR         equ     $F00009         ; write: auxiliary control

DUA_SR_RXRDY    equ     $01             ; SRA: a byte is waiting
DUA_SR_TXRDY    equ     $04             ; SRA: room in the transmit holding register

RAM_TOP         equ     $00100000       ; the board has 1MB, and the stack grows down

                org     $E00000

                dc.l    RAM_TOP         ; supervisor stack pointer at reset
                dc.l    start           ; and where the CPU begins

start:          bsr     uart_init

                lea     hello(pc),a0
                bsr     print

; Everything typed at the console comes back, which is as much as a machine
; with one serial port and no operating system can do.
echo:           bsr     getchar
                cmp.b   #13,d0          ; Enter arrives as a carriage return
                bne.s   .send
                bsr     putchar         ; and a terminal wants a line feed too
                move.b  #10,d0
.send:          bsr     putchar
                bra.s   echo

; Sets up channel A for 115200 baud, 8 data bits, no parity, one stop bit,
; which is what the other end of the cable is expecting.
uart_init:      move.b  #$20,DUA_CRA    ; reset the receiver
                move.b  #$30,DUA_CRA    ; reset the transmitter
                move.b  #$10,DUA_CRA    ; point at MR1A
                move.b  #$13,DUA_MR1A   ; 8 data bits, no parity
                move.b  #$07,DUA_MR1A   ; one stop bit; this write lands in MR2A
                move.b  #$00,DUA_ACR    ; the first of the two baud rate tables
                move.b  #$80,DUA_CRA    ; the receiver reads the extended table
                move.b  #$A0,DUA_CRA    ; and so does the transmitter
                move.b  #$88,DUA_CSRA   ; entry 8 of it, both ways: 115200 baud
                move.b  #$05,DUA_CRA    ; enable the receiver and the transmitter
                rts

; Sends the character in d0. Clobbers d1.
putchar:        move.b  DUA_SRA,d1
                andi.b  #DUA_SR_TXRDY,d1
                beq.s   putchar         ; wait for room
                move.b  d0,DUA_TBA
                rts

; Waits for a character and returns it in d0. Clobbers d1.
getchar:        move.b  DUA_SRA,d1
                andi.b  #DUA_SR_RXRDY,d1
                beq.s   getchar
                move.b  DUA_RBA,d0
                rts

; Sends the NUL-terminated string at a0. Clobbers d0, d1 and a0.
print:          move.b  (a0)+,d0
                beq.s   .done
                bsr     putchar
                bra.s   print
.done:          rts

hello:          dc.b    "Hello from my own firmware!",13,10
                dc.b    "Type something; it comes back.",13,10,0
                even
