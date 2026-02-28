# Windows Guest Support

This document describes the requirements and configuration for running Windows as a guest OS with lg-capture functionality.

## Overview

lg-capture provides frame capture and input injection for Windows guests through:
- **VirtIO GPU**: Display output capture
- **xHCI Controller**: USB 3.0 host controller for HID devices
- **USB HID**: Keyboard and mouse input devices

## Hardware Requirements

### VirtIO GPU Device

The VirtIO GPU device (stub implementation) provides:
- **Device ID**: 0x1050 (VirtIO)
- **Vendor ID**: 0x1AF4 (Red Hat)
- **Features**: EDID support (VIRTIO_GPU_F_EDID)

**Windows Driver Requirements**:
- Install [VirtIO GPU Driver](https://github.com/virtio-win/kvm-guest-drivers-windows)
- Use `viorng` or bundled WDDM driver

### xHCI USB Controller

The xHCI controller provides USB 3.0 support:
- **PCI Class**: 0x0C0330 (USB xHCI)
- **Version**: 1.0 (0x0100)
- **Max Slots**: 32
- **Max Ports**: 4

**Windows Driver**: Built-in Microsoft xHCI driver (compatible)

### USB HID Devices

USB HID devices emulate standard keyboards and mice:
- **USB Class**: 0x03 (HID)
- **Protocols**: Keyboard (1), Mouse (2)
- **Speed**: Full Speed (12 Mbps) to Super Speed (5 Gbps)

**Windows Driver**: Built-in Windows HID driver (compatible)

## Configuration

### VM Configuration Example

```json
{
  "cpus": {
    "boot_vcpus": 4,
    "max_vcpus": 4
  },
  "memory": {
    "size": 8192,
    "shared": true
  },
  "devices": [
    {
      "gpu": {
        "width": 1920,
        "height": 1080
      }
    },
    {
      "usb_xhci": {
        "ports": 4
      }
    }
  ]
}
```

### Frame Capture Configuration

For Windows guests, frame capture uses the IVSHMEM shared memory:

```
+------------------+
| FrameBufferHeader|  (88 bytes)
+------------------+
| FrameMetadata[N] |  (40 bytes each)
+------------------+
| Buffer[0] data   |
| Buffer[1] data   |
| Buffer[2] data   |
+------------------+
| CursorMetadata   |
| CursorShapeInfo  |
| Cursor data      |
+------------------+
```

## Windows Guest Agent

A Windows guest agent is required for:
1. Capturing frame data from the display
2. Writing to IVSHMEM shared memory
3. Processing host commands

### Building for Windows

```powershell
# Install Rust for Windows
# Add target
rustup target add x86_64-pc-windows-msvc

# Build
cargo build --release --target x86_64-pc-windows-msvc
```

### Windows Capture Backends

| Backend | Method | Performance |
|---------|--------|-------------|
| DXGI Desktop Duplication | GPU-accelerated | Best |
| GDI | CPU-based | Good |
| WDDM | Kernel-level | Excellent |

## Troubleshooting

### VirtIO GPU Not Detected

1. Ensure VirtIO drivers are installed
2. Check Device Manager for unknown devices
3. Verify PCI configuration:
   ```
   lspci -nn | grep -i virtio
   ```

### USB HID Not Working

1. Verify xHCI controller appears in Device Manager
2. Check for "Unknown Device" under Universal Serial Bus controllers
3. Install USB 3.0 drivers if needed (Windows 7)

### Frame Capture Issues

1. Verify IVSHMEM device is present
2. Check shared memory size is sufficient
3. Ensure guest agent has write permissions

## Performance Tuning

### Recommended Settings

| Resolution | Buffer Count | Buffer Size |
|------------|--------------|-------------|
| 1920x1080  | 3            | 8 MB        |
| 2560x1440  | 3            | 14 MB       |
| 3840x2160  | 4            | 32 MB       |

### CPU Affinity

For best performance, pin the VM to specific CPU cores:
```json
{
  "cpus": {
    "boot_vcpus": 4,
    "topology": {
      "threads_per_core": 2,
      "cores_per_die": 2,
      "dies_per_package": 1,
      "packages": 1
    }
  }
}
```

## Known Limitations

1. **VirtIO GPU**: Current implementation is a stub; full 3D acceleration not supported
2. **USB HID**: Only basic keyboard/mouse support; no advanced HID features
3. **Frame Capture**: Software capture only; hardware acceleration requires DXGI

## Testing

### Verify xHCI Controller

```powershell
# In Windows PowerShell
Get-PnpDevice -Class USB | Where-Object { $_.FriendlyName -like "*xHCI*" }
```

### Verify HID Devices

```powershell
# In Windows PowerShell
Get-PnpDevice -Class HIDClass
```

### Test Input Injection

Use the HTTP API to inject input:
```bash
curl -X POST http://localhost:8000/api/v1/vm/1/inject-input \
  -H "Content-Type: application/json" \
  -d '{"keyboard": [{"action": "type", "code": 0x1E}]}'
```

## References

- [VirtIO Specification](https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html)
- [xHCI Specification](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf)
- [USB HID Specification](https://www.usb.org/hid)
- [Windows VirtIO Drivers](https://fedorapeople.org/groups/virtio-win/virtio-win-direct-downloads/)
