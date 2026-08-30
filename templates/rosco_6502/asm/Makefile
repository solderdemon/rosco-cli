# rosco_6502 assembly program, built with the cc65 tools.
#
# The whole toolchain is one package: `apt install cc65`, `dnf install cc65`,
# `pacman -S cc65` or `brew install cc65`. `rosco build` can also run it in
# Docker instead, leaving nothing on this computer.

# The binary follows the directory name, which is where `rosco` looks for it.
TARGET := $(notdir $(CURDIR))

AS := ca65
LD := ld65
DA := da65

CPU := 65SC02
CONFIG := rosco_6502.cfg
INCDIR := inc

# Programs load and start at the bottom of user low RAM.
START_ADDR := 0x0800

ASFLAGS := --cpu $(CPU) -I $(INCDIR)
LDFLAGS := -C $(CONFIG) -S $(START_ADDR)

SOURCES := $(wildcard *.s)
OBJECTS := $(SOURCES:.s=.o)
INCLUDES := $(wildcard $(INCDIR)/*.inc)

all: $(TARGET).bin

$(TARGET).bin: $(OBJECTS) $(CONFIG)
	$(LD) $(LDFLAGS) -Ln $(TARGET).sym -m $(TARGET).map -o $@ $(OBJECTS)

%.o: %.s $(INCLUDES) $(MAKEFILE_LIST)
	$(AS) $(ASFLAGS) -l $(basename $<).lst -o $@ $<

# What actually ended up in the binary, addresses and all.
disasm: $(TARGET).bin
	$(DA) --comments 4 --cpu $(CPU) --start-addr $(START_ADDR) $(TARGET).bin >$(TARGET).dis

clean:
	$(RM) *.o *.lst *.sym *.map *.dis $(TARGET).bin

.PHONY: all clean disasm
