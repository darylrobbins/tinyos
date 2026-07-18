TARGET      := aarch64-unknown-uefi
PROFILE     := release
KERNEL_EFI  := target/$(TARGET)/$(PROFILE)/kernel.efi
ESP         := esp
QEMU        := qemu-system-aarch64
EDK2_CODE   := $(shell dirname $(shell which $(QEMU)))/../share/qemu/edk2-aarch64-code.fd
BUILD       := build

ACCEL       := -accel hvf -cpu host
# Fallback if HVF misbehaves: make run ACCEL="-accel tcg -cpu cortex-a72"

# Guest resolution; override with e.g. make run RES=1920x1200
RES         := 1440x900
# Window scaling (View menu also has Zoom to Fit); headless runs override this.
DISPLAY_ARG := -display cocoa,zoom-to-fit=on

QEMU_ARGS   := -machine virt -m 512M $(ACCEL) \
    -drive if=pflash,format=raw,readonly=on,file=$(BUILD)/code.fd \
    -drive if=pflash,format=raw,file=$(BUILD)/vars.fd \
    -device ramfb \
    -device virtio-keyboard-pci \
    -device virtio-tablet-pci \
    -drive format=raw,file=fat:rw:$(ESP) \
    -fw_cfg name=opt/tinyos/res,string=$(RES) \
    -serial stdio

run: QEMU_ARGS += $(DISPLAY_ARG)

.PHONY: build run gdb clean

build:
	cargo build --target $(TARGET) $(if $(filter release,$(PROFILE)),--release,)
	mkdir -p $(ESP)/EFI/BOOT
	cp $(KERNEL_EFI) $(ESP)/EFI/BOOT/BOOTAA64.EFI

$(BUILD)/code.fd:
	mkdir -p $(BUILD)
	cp "$(EDK2_CODE)" $@

# Always start from clean NVRAM: stale boot entries from a previous device
# layout can hijack the boot order.
.PHONY: $(BUILD)/vars.fd
$(BUILD)/vars.fd:
	mkdir -p $(BUILD)
	dd if=/dev/zero of=$@ bs=1m count=64 2>/dev/null

run: build $(BUILD)/code.fd $(BUILD)/vars.fd
	$(QEMU) $(QEMU_ARGS)

gdb: build $(BUILD)/code.fd $(BUILD)/vars.fd
	$(QEMU) $(QEMU_ARGS) -s -S

clean:
	cargo clean
	rm -rf $(ESP) $(BUILD)
