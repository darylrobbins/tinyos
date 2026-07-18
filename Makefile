# Usage: make run [ARCH=aarch64|x86_64] [RES=1440x900] [PROFILE=release]
ARCH        := aarch64
PROFILE     := release
RES         := 1440x900
ESP         := esp
BUILD       := build
QEMU_SHARE   = $(shell dirname $(shell which $(QEMU)))/../share/qemu

ifeq ($(ARCH),aarch64)
TARGET      := aarch64-unknown-uefi
QEMU        := qemu-system-aarch64
MACHINE     := -machine virt
ACCEL       := -accel hvf -cpu host
# Fallback if HVF misbehaves: make run ACCEL="-accel tcg -cpu cortex-a72"
BOOT_EFI    := BOOTAA64.EFI
EDK2_CODE    = $(QEMU_SHARE)/edk2-aarch64-code.fd
# aarch64 edk2 wants both pflash units; vars start zeroed every run so a
# previous device layout can't leave stale boot entries behind.
FLASH        = -drive if=pflash,format=raw,readonly=on,file=$(BUILD)/code-$(ARCH).fd \
               -drive if=pflash,format=raw,file=$(BUILD)/vars-$(ARCH).fd
else ifeq ($(ARCH),x86_64)
TARGET      := x86_64-unknown-uefi
QEMU        := qemu-system-x86_64
MACHINE     := -machine q35
ACCEL       := -accel tcg
BOOT_EFI    := BOOTX64.EFI
EDK2_CODE    = $(QEMU_SHARE)/edk2-x86_64-code.fd
# -vga none: q35 otherwise adds a default VGA and the firmware drives it
# as the primary display instead of our ramfb.
FLASH        = -drive if=pflash,format=raw,readonly=on,file=$(BUILD)/code-$(ARCH).fd -vga none
else
$(error unsupported ARCH '$(ARCH)')
endif

KERNEL_EFI  := target/$(TARGET)/$(PROFILE)/kernel.efi

# Window scaling (View menu also has Zoom to Fit); headless runs override this.
DISPLAY_ARG := -display cocoa,zoom-to-fit=on

QEMU_ARGS    = $(MACHINE) -m 512M $(ACCEL) $(FLASH) \
    -device ramfb \
    -device virtio-keyboard-pci \
    -device virtio-tablet-pci \
    -drive format=raw,file=fat:rw:$(ESP) \
    -fw_cfg name=opt/tinyos/res,string=$(RES) \
    -serial stdio

run: QEMU_ARGS += $(DISPLAY_ARG)

.PHONY: build run gdb clean firmware

build:
	cargo build --target $(TARGET) $(if $(filter release,$(PROFILE)),--release,)
	mkdir -p $(ESP)/EFI/BOOT
	cp $(KERNEL_EFI) $(ESP)/EFI/BOOT/$(BOOT_EFI)

firmware:
	mkdir -p $(BUILD)
	cp "$(EDK2_CODE)" $(BUILD)/code-$(ARCH).fd
ifeq ($(ARCH),aarch64)
	dd if=/dev/zero of=$(BUILD)/vars-$(ARCH).fd bs=1m count=64 2>/dev/null
endif

run: build firmware
	$(QEMU) $(QEMU_ARGS)

gdb: build firmware
	$(QEMU) $(QEMU_ARGS) -s -S

clean:
	cargo clean
	rm -rf $(ESP) $(BUILD)
