# rosco_6502 C program, built with the cc65 tools.
#
# The whole toolchain is one package: `apt install cc65`, `dnf install cc65`,
# `pacman -S cc65` or `brew install cc65`. `rosco build` can also run it in
# Docker instead, leaving nothing on this computer.

# The binary follows the directory name, which is where `rosco` looks for it.
TARGET := $(notdir $(CURDIR))

CC := cl65
DA := da65

CPU := 65SC02
CONFIG := rosco_6502.cfg
INCDIR := inc

# Programs load and start at the bottom of user low RAM.
START_ADDR := 0x0800

# `-t none` is cc65's board-independent target: the C library, and nothing
# that belongs to a machine this is not. console.s supplies the rest.
CFLAGS := -t none -O --cpu $(CPU) -C $(CONFIG) --asm-include-dir $(INCDIR)

SOURCES := $(wildcard *.c) $(wildcard *.s)
INCLUDES := $(wildcard *.h) $(wildcard $(INCDIR)/*.inc)

all: $(TARGET).bin

$(TARGET).bin: $(SOURCES) $(INCLUDES) $(CONFIG) $(MAKEFILE_LIST)
	$(CC) $(CFLAGS) -Ln $(TARGET).sym -m $(TARGET).map -o $@ $(SOURCES)

# What actually ended up in the binary, addresses and all.
disasm: $(TARGET).bin
	$(DA) --comments 4 --cpu $(CPU) --start-addr $(START_ADDR) $(TARGET).bin >$(TARGET).dis

clean:
	$(RM) *.o *.lst *.sym *.map *.dis $(TARGET).bin

.PHONY: all clean disasm
