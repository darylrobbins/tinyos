pub mod fwcfg;
pub mod input;
pub mod mmio;
pub mod pci;
pub mod virtio;
pub mod virtio_blk;

/// One scan of the PCI bus with one BAR allocator (two allocators would
/// hand out overlapping addresses): claim all input devices and the first
/// block device.
pub fn probe() -> (input::Input, Option<virtio_blk::VirtioBlk>) {
    let mut alloc = pci::BarAllocator::new();
    let mut input = input::Input::new();
    let mut blk = None;
    for dev in pci::scan() {
        kprintln!(
            "tinyos: pci {:02x}:{:02x}.0 {:04x}:{:04x}",
            dev.bdf >> 8,
            (dev.bdf >> 3) & 0x1f,
            dev.vendor,
            dev.device
        );
        if dev.vendor != pci::VENDOR_VIRTIO {
            continue;
        }
        match dev.device {
            input::VIRTIO_ID_INPUT => input.claim(&dev, &mut alloc),
            virtio_blk::VIRTIO_ID_BLK if blk.is_none() => {
                match virtio_blk::VirtioBlk::init(&dev, &mut alloc) {
                    Some(b) => {
                        kprintln!(
                            "tinyos: virtio-blk ready, {} MiB (bdf {:#x})",
                            b.capacity_sectors() * virtio_blk::SECTOR_SIZE as u64 >> 20,
                            dev.bdf
                        );
                        blk = Some(b);
                    }
                    None => kprintln!("tinyos: virtio-blk init FAILED (bdf {:#x})", dev.bdf),
                }
            }
            _ => {}
        }
    }
    (input, blk)
}
