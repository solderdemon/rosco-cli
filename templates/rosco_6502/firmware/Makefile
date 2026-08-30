# rosco_6502 firmware, built with the cc65 tools.
#
# The whole toolchain is one package: `apt install cc65`, `dnf install cc65`,
# `pacman -S cc65` or `brew install cc65`. `rosco build` can also run it in
# Docker instead, leaving nothing on this computer.
#
# The output is the whole 32KB ROM chip rather than a program: the firmware
# in bank 0, which is the one the board selects at reset, and the other three
# banks left empty.

# The image follows the directory name, which is where `rosco` looks for it.
TARGET := $(notdir $(CURDIR))

AS := ca65
LD := ld65

CPU := 65SC02
CONFIG := rosco_6502.cfg

ASFLAGS := --cpu $(CPU)
LDFLAGS := -C $(CONFIG)

SOURCES := $(wildcard *.s)
OBJECTS := $(SOURCES:.s=.o)

all: $(TARGET).rom

$(TARGET).rom: $(OBJECTS) $(CONFIG)
	$(LD) $(LDFLAGS) -Ln $(TARGET).sym -m $(TARGET).map -o $@ $(OBJECTS)

%.o: %.s $(MAKEFILE_LIST)
	$(AS) $(ASFLAGS) -l $(basename $<).lst -o $@ $<

clean:
	$(RM) *.o *.lst *.sym *.map $(TARGET).rom

.PHONY: all clean
