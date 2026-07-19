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
MACHINE     := -machine virt,gic-version=3 -smp 4
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
MACHINE     := -machine q35 -smp 4
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
DISK        := disk.img
DISK_SIZE   := 64M
MKFS        := target/debug/mkfs-tinyfs

# Window scaling (View menu also has Zoom to Fit); headless runs override this.
DISPLAY_ARG := -display cocoa,zoom-to-fit=on

QEMU_ARGS    = $(MACHINE) -m 512M $(ACCEL) $(FLASH) \
    -device ramfb \
    -device virtio-keyboard-pci \
    -device virtio-tablet-pci \
    -drive format=raw,file=fat:rw:$(ESP) \
    -drive if=none,file=$(DISK),format=raw,id=hd \
    -device virtio-blk-pci,drive=hd,disable-legacy=on,disable-modern=off \
    -fw_cfg name=opt/tinyos/res,string=$(RES) \
    -serial stdio

run: QEMU_ARGS += $(DISPLAY_ARG)

# Headless smoke test: boot, drive the userspace shell over QMP, assert on
# command output mirrored to serial (opt/tinyos/smoke turns the mirror on).
# aarch64 only; catches runtime bugs host `make test` cannot (see tools/smoke).
SMOKE_ARGS = -display none -fw_cfg name=opt/tinyos/smoke,string=1

.PHONY: build apps run gdb clean cleandisk firmware mkfs test sync-apps smoke

# Never `cargo build --target ...` at the workspace root: mkfs-tinyfs is a
# host-target std binary and would fail to cross-compile for UEFI.
build: apps
	cargo build -p kernel --target $(TARGET) $(if $(filter release,$(PROFILE)),--release,)
	mkdir -p $(ESP)/EFI/BOOT
	cp $(KERNEL_EFI) $(ESP)/EFI/BOOT/$(BOOT_EFI)

# Third-party userspace apps: a separate workspace (aarch64-unknown-none),
# staged under $(STAGE)/apps and baked into the tinyfs image when the disk is
# created (see $(DISK) below). aarch64 only for now (userspace is aarch64-first).
APP_BINS := hello pixels solitaire greet tui progress view vi clock top edit sh
STAGE    := $(BUILD)/stage
apps:
ifeq ($(ARCH),aarch64)
	cd apps && cargo build --release
	mkdir -p $(STAGE)/apps
	$(foreach a,$(APP_BINS),cp apps/target/aarch64-unknown-none/release/$(a) $(STAGE)/apps/$(a);)
endif

mkfs:
	cargo build -p mkfs-tinyfs

# Created only if missing so its contents persist across runs (`make cleandisk`
# resets it, re-baking the apps currently staged in $(STAGE)). After rebuilding
# an app, `make sync-apps` refreshes /apps in place — user files survive.
$(DISK): | mkfs apps
	$(MKFS) create $(DISK) --size $(DISK_SIZE) $(if $(wildcard $(STAGE)/*),--populate $(STAGE),)

# Update /apps inside disk.img without recreating it (VM off). Creates the
# disk first if it doesn't exist yet (fresh checkout/worktree).
sync-apps: mkfs apps | $(DISK)
	$(foreach a,$(APP_BINS),$(MKFS) put $(DISK) $(STAGE)/apps/$(a) /apps/$(a);)

test:
	cargo test -p tinyfs
	cargo test -p textui
	cargo test -p vicore
	# Both kernel targets must keep compiling (stub drift breaks x86_64).
	cargo check -p kernel --target aarch64-unknown-uefi
	cargo check -p kernel --target x86_64-unknown-uefi
	cd apps && cargo test -p solitaire --lib --no-default-features --target $$(rustc -vV | sed -n 's/^host: //p')

firmware:
	mkdir -p $(BUILD)
	cp "$(EDK2_CODE)" $(BUILD)/code-$(ARCH).fd
ifeq ($(ARCH),aarch64)
	dd if=/dev/zero of=$(BUILD)/vars-$(ARCH).fd bs=1m count=64 2>/dev/null
endif

run: build firmware $(DISK)
	$(QEMU) $(QEMU_ARGS)

gdb: build firmware $(DISK)
	$(QEMU) $(QEMU_ARGS) -s -S

# Rebuild apps into the disk first so the shell/apps under test are current.
smoke: build firmware sync-apps
	python3 tools/smoke/smoke.py -- $(QEMU) $(QEMU_ARGS) $(SMOKE_ARGS)

# Keeps disk.img; use cleandisk to reset the filesystem.
clean:
	cargo clean
	rm -rf $(ESP) $(BUILD)

cleandisk:
	rm -f $(DISK)
