# rosco_m68k firmware, assembled with vasm.
#
# vasm comes with the rosco_m68k toolchain; `rosco build` can also run it in
# Docker instead, leaving nothing on this computer.
#
# The output is a ROM image rather than a program. Nothing is linked into it:
# the first eight bytes are the stack pointer and program counter the board
# hands the CPU at reset, and the rest runs from $E00000 where the ROM is
# mapped.

# The image follows the directory name, which is where `rosco` looks for it.
TARGET := $(notdir $(CURDIR))

AS := vasmm68k_mot
ASFLAGS := -m68010 -Fbin -quiet

SOURCE := firmware.asm

all: $(TARGET).rom

$(TARGET).rom: $(SOURCE) $(MAKEFILE_LIST)
	$(AS) $(ASFLAGS) -L $(TARGET).lst -o $@ $(SOURCE)

clean:
	$(RM) $(TARGET).lst $(TARGET).rom

.PHONY: all clean
