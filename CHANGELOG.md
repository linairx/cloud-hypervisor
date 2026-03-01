# Changelog

All notable changes to the lg-capture integration in Cloud Hypervisor will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

#### Input Injection
- **PS/2 Input Backend**: Extended i8042 keyboard controller with keyboard and mouse input injection support
  - PS/2 IntelliMouse protocol for scroll wheel support
  - Keyboard scancode translation (Set 1)
- **VirtIO Input Backend**: Full VirtIO input device integration
  - Keyboard event injection with modifier support
  - Mouse event injection (relative, absolute, scroll, buttons)
  - Screen dimension tracking for absolute positioning
- **USB HID Backend**: USB Human Interface Device emulation
  - Standard keyboard and mouse report generation
  - Integration with xHCI controller via `SharedUsbHidDevice`
  - Boot protocol support
- **Input Event Batching**: Performance optimization for high-frequency input
  - Adaptive batch sizing based on event rate
  - Configurable timeout and size limits
  - Zero-copy event processing where possible

#### Frame Capture
- **IVSHMEM Frame Buffer**: Shared memory frame buffer protocol
  - Triple buffering with lock-free atomic operations
  - Frame, cursor, and audio data layout
  - Guest-Agent protocol implementation
- **X11 XShm Capture**: Zero-copy frame capture for X11
  - XShm extension for true zero-copy
  - `shm_open`/`mmap` shared memory implementation
  - Graceful fallback to standard `get_image`
- **X11 XFixes Cursor Capture**: Real cursor tracking
  - `XQueryPointer` for cursor position
  - `XFixesGetCursorImageAndName` for cursor shape
  - ARGB to BGRA format conversion
- **Wayland Capture Backend**: Wayland display capture support
  - `wayland-client` 0.31 integration
  - Framework for `wlr-screencopy` protocol
  - Priority: Wayland > X11 > Stub

#### Audio Capture
- **PulseAudio Integration**: System audio capture
  - `libpulse-simple` binding
  - Monitor source capture (system audio output)
  - Ring buffer audio writing to shared memory
  - Feature-gated (`pulseaudio` feature)

#### USB/xHCI
- **xHCI Controller**: USB 3.0 host controller emulation
  - xHCI 1.0 compliant
  - Command Ring, Event Ring, Transfer Ring
  - Device slot management (32 slots, 4 ports)
  - Dynamic USB address allocation (1-127)
- **USB HID Devices**: Keyboard and mouse emulation
  - Standard USB descriptors
  - HID report descriptor generation
  - `UsbDevice` trait implementation

#### VirtIO GPU
- **2D Rendering Commands**: Software rendering support
  - `VIRTIO_GPU_CMD_GET_DISPLAY_INFO`
  - `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`
  - `VIRTIO_GPU_CMD_SET_SCANOUT`
  - `VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D`
  - `VIRTIO_GPU_CMD_RESOURCE_FLUSH`
- **Resource Management**: HashMap-based 2D resource storage
- **Epoll Handler**: Control and cursor queue processing

#### HTTP API
- **Input Injection Endpoint**: `/api/v1/vm/{id}/inject-input`
  - Batch input request support
  - Keyboard and mouse event handling
  - Multi-backend routing

#### Testing
- **Integration Tests**: 21 input injection tests
  - PS/2, VirtIO, USB HID backend coverage
  - Keyboard and mouse event creation tests
  - Stealth level and capability tests
- **Unit Tests**: 50+ tests across modules
  - Frame buffer tests
  - xHCI controller tests
  - Audio format tests

### Changed

- **Error Handling**: Replaced critical `unwrap()` calls with `expect()` and clear error messages
- **VirtIO Input**: Improved absolute mouse positioning with position tracking
- **xHCI**: Dynamic USB address allocation instead of fixed address 1

### Documentation

- **Windows Guest Support**: `docs/WINDOWS_GUEST.md`
  - VirtIO GPU, xHCI, USB HID driver requirements
  - Frame capture configuration
  - Troubleshooting guide
- **Progress Tracking**: `docs/LG_CAPTURE_PROGRESS.md`
  - Phase completion status
  - Component completion percentages
  - Remaining work priorities

## [0.1.0] - 2026-03-01

### Added

- Initial lg-capture integration
- PS/2 keyboard and mouse support
- IVSHMEM shared memory frame buffer
- Guest agent for Linux
- HTTP API for input injection

---

## Component Status

| Component | Completion | Notes |
|-----------|------------|-------|
| PS/2 Input | 100% | Keyboard + IntelliMouse |
| VirtIO Input | 100% | Keyboard + Mouse |
| USB HID Input | 95% | Needs xHCI transfer ring |
| xHCI Controller | 85% | Basic USB 3.0 support |
| VirtIO GPU | 70% | 2D software rendering |
| Frame Capture | 95% | X11 XShm, Wayland stub |
| Audio Capture | 85% | PulseAudio |
| Guest Agent | 90% | Linux support |
| Windows Support | 60% | Documentation only |

## Future Work

### Medium Priority
- VirtIO GPU 3D commands (VIRGL/venus)
- Full wlr-screencopy implementation
- Windows DXGI frame capture

### Low Priority
- API documentation generation
- Performance benchmarking
- Multi-monitor support
