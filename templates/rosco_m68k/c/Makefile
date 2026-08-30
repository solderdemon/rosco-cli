ROSCO_M68K_DEFAULT_DIR=./

ifndef ROSCO_M68K_DIR
$(info NOTE: ROSCO_M68K_DIR not set, using libs: ./libs)
ROSCO_M68K_DIR=$(ROSCO_M68K_DEFAULT_DIR)
else
$(info NOTE: Using ROSCO_M68K_DIR libs in: $(ROSCO_M68K_DIR))
endif

-include $(ROSCO_M68K_DIR)/software.mk

EXTRA_CFLAGS=
EXTRA_LIBS=
# EXTRA_VASMFLAGS?=-showopt