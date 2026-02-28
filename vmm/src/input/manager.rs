// Copyright 2024 Cloud Hypervisor Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input Manager
//!
//! The Input Manager coordinates input injection across multiple backends
//! and provides a unified API for input operations.

use super::backend::{BackendType, InputBackend, InputCapabilities, Ps2Backend, VirtioInputBackend};
use super::event::{InputEvent, InputRequest, KeyboardEvent, MouseEvent};
use super::{InputError, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Input manager configuration
#[derive(Clone, Debug, Default)]
pub struct InputConfig {
    /// Default backend to use
    pub default_backend: BackendType,
    /// Allow backend switching at runtime
    pub allow_backend_switch: bool,
    /// Enable keyboard input
    pub enable_keyboard: bool,
    /// Enable mouse input
    pub enable_mouse: bool,
}

impl InputConfig {
    /// Create a new configuration with PS/2 as default
    pub fn new() -> Self {
        Self {
            default_backend: BackendType::Ps2,
            allow_backend_switch: true,
            enable_keyboard: true,
            enable_mouse: true,
        }
    }

    /// Create configuration for automation use (PS/2 for stealth)
    pub fn automation() -> Self {
        Self {
            default_backend: BackendType::Ps2,
            allow_backend_switch: true,
            enable_keyboard: true,
            enable_mouse: true,
        }
    }

    /// Create configuration for standard VM use (virtio)
    pub fn standard() -> Self {
        Self {
            default_backend: BackendType::Virtio,
            allow_backend_switch: true,
            enable_keyboard: true,
            enable_mouse: true,
        }
    }
}

/// Input manager for a single VM
pub struct InputManager {
    /// Configuration
    config: InputConfig,
    /// Active backend type
    active_backend: BackendType,
    /// PS/2 backend
    ps2_backend: Option<Ps2Backend>,
    /// VirtIO backend
    virtio_backend: Option<VirtioInputBackend>,
    /// Statistics
    stats: InputStats,
}

/// Input statistics
#[derive(Clone, Debug, Default)]
pub struct InputStats {
    pub keyboard_events: u64,
    pub mouse_events: u64,
    pub total_events: u64,
    pub errors: u64,
}

impl InputManager {
    /// Create a new input manager
    pub fn new(config: InputConfig) -> Self {
        let active_backend = config.default_backend;

        Self {
            config,
            active_backend,
            ps2_backend: None,
            virtio_backend: None,
            stats: InputStats::default(),
        }
    }

    /// Create with default configuration
    pub fn default_config() -> Self {
        Self::new(InputConfig::new())
    }

    // ========================================================================
    // Backend Management
    // ========================================================================

    /// Initialize PS/2 backend
    pub fn init_ps2_backend(&mut self) {
        self.ps2_backend = Some(Ps2Backend::new());
    }

    /// Initialize VirtIO backend
    pub fn init_virtio_backend(&mut self) {
        self.virtio_backend = Some(VirtioInputBackend::new());
    }

    /// Get active backend type
    pub fn active_backend(&self) -> BackendType {
        self.active_backend
    }

    /// Switch active backend
    pub fn switch_backend(&mut self, backend: BackendType) -> Result<()> {
        if !self.config.allow_backend_switch {
            return Err(InputError::UnsupportedAction(
                "Backend switching is disabled".to_string(),
            ));
        }

        // Check if backend is available
        match backend {
            BackendType::Ps2 => {
                if self.ps2_backend.is_none() {
                    return Err(InputError::BackendNotAvailable(
                        "PS/2 backend not initialized".to_string(),
                    ));
                }
            }
            BackendType::Virtio => {
                if self.virtio_backend.is_none() {
                    return Err(InputError::BackendNotAvailable(
                        "VirtIO backend not initialized".to_string(),
                    ));
                }
            }
            BackendType::UsbHid => {
                return Err(InputError::BackendNotAvailable(
                    "USB HID backend not yet implemented".to_string(),
                ));
            }
        }

        self.active_backend = backend;
        Ok(())
    }

    /// Get active backend capabilities
    pub fn capabilities(&self) -> Option<InputCapabilities> {
        match self.active_backend {
            BackendType::Ps2 => self.ps2_backend.as_ref().map(|b| b.capabilities()),
            BackendType::Virtio => self.virtio_backend.as_ref().map(|b| b.capabilities()),
            BackendType::UsbHid => None,
        }
    }

    /// Check if input is ready
    pub fn is_ready(&self) -> bool {
        match self.active_backend {
            BackendType::Ps2 => self.ps2_backend.as_ref().map_or(false, |b| b.is_ready()),
            BackendType::Virtio => self.virtio_backend.as_ref().map_or(false, |b| b.is_ready()),
            BackendType::UsbHid => false,
        }
    }

    // ========================================================================
    // Input Injection
    // ========================================================================

    /// Inject a single event
    pub fn inject(&mut self, event: &InputEvent) -> Result<()> {
        match self.active_backend {
            BackendType::Ps2 => {
                if let Some(ref mut backend) = self.ps2_backend {
                    backend.inject(event)?;
                }
            }
            BackendType::Virtio => {
                if let Some(ref mut backend) = self.virtio_backend {
                    backend.inject(event)?;
                }
            }
            BackendType::UsbHid => {
                return Err(InputError::BackendNotAvailable(
                    "USB HID not implemented".to_string(),
                ));
            }
        }

        // Update statistics
        match event {
            InputEvent::Keyboard(_) => self.stats.keyboard_events += 1,
            InputEvent::Mouse(_) => self.stats.mouse_events += 1,
        }
        self.stats.total_events += 1;

        Ok(())
    }

    /// Inject a keyboard event
    pub fn inject_keyboard(&mut self, event: &KeyboardEvent) -> Result<()> {
        if !self.config.enable_keyboard {
            return Err(InputError::UnsupportedAction(
                "Keyboard input is disabled".to_string(),
            ));
        }

        self.inject(&InputEvent::Keyboard(event.clone()))
    }

    /// Inject a mouse event
    pub fn inject_mouse(&mut self, event: &MouseEvent) -> Result<()> {
        if !self.config.enable_mouse {
            return Err(InputError::UnsupportedAction(
                "Mouse input is disabled".to_string(),
            ));
        }

        self.inject(&InputEvent::Mouse(event.clone()))
    }

    /// Process a batch input request
    pub fn process_request(&mut self, request: &InputRequest) -> Result<InputStats> {
        // Switch backend if specified
        if let Some(ref backend_name) = request.backend {
            if let Some(backend_type) = BackendType::from_name(backend_name) {
                self.switch_backend(backend_type)?;
            } else {
                return Err(InputError::BackendNotAvailable(format!(
                    "Unknown backend: {}",
                    backend_name
                )));
            }
        }

        let mut stats = InputStats::default();

        // Process keyboard events
        for event in &request.keyboard {
            match self.inject_keyboard(event) {
                Ok(()) => {
                    stats.keyboard_events += 1;
                    stats.total_events += 1;
                }
                Err(e) => {
                    stats.errors += 1;
                    log::warn!("Keyboard injection failed: {}", e);
                }
            }
        }

        // Process mouse events
        for event in &request.mouse {
            match self.inject_mouse(event) {
                Ok(()) => {
                    stats.mouse_events += 1;
                    stats.total_events += 1;
                }
                Err(e) => {
                    stats.errors += 1;
                    log::warn!("Mouse injection failed: {}", e);
                }
            }
        }

        Ok(stats)
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    /// Get statistics
    pub fn stats(&self) -> &InputStats {
        &self.stats
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = InputStats::default();
    }

    // ========================================================================
    // Convenience Methods
    // ========================================================================

    /// Type a key (press + release)
    pub fn type_key(&mut self, code: u16) -> Result<()> {
        use super::event::KeyboardAction;

        self.inject_keyboard(&KeyboardEvent {
            action: KeyboardAction::Press,
            code,
            modifiers: Default::default(),
        })?;

        self.inject_keyboard(&KeyboardEvent {
            action: KeyboardAction::Release,
            code,
            modifiers: Default::default(),
        })?;

        Ok(())
    }

    /// Move mouse relative
    pub fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<()> {
        use super::event::{MouseAction, MouseButtons};

        self.inject_mouse(&MouseEvent {
            action: MouseAction::Move,
            x: dx,
            y: dy,
            z: 0,
            button: None,
            buttons: MouseButtons::default(),
        })
    }

    /// Mouse click (press + release)
    pub fn mouse_click(&mut self, button: super::event::MouseButton) -> Result<()> {
        use super::event::MouseAction;

        self.inject_mouse(&MouseEvent {
            action: MouseAction::ButtonPress,
            x: 0,
            y: 0,
            z: 0,
            button: Some(button),
            buttons: Default::default(),
        })?;

        self.inject_mouse(&MouseEvent {
            action: MouseAction::ButtonRelease,
            x: 0,
            y: 0,
            z: 0,
            button: Some(button),
            buttons: Default::default(),
        })?;

        Ok(())
    }

    /// Mouse scroll
    pub fn mouse_scroll(&mut self, delta: i32) -> Result<()> {
        use super::event::MouseAction;

        self.inject_mouse(&MouseEvent {
            action: MouseAction::Scroll,
            x: 0,
            y: 0,
            z: delta,
            button: None,
            buttons: Default::default(),
        })
    }
}

// ============================================================================
// Multi-VM Input Manager
// ============================================================================

/// Multi-VM input manager
pub struct MultiVmInputManager {
    managers: HashMap<String, Arc<Mutex<InputManager>>>,
    default_vm: Option<String>,
}

impl MultiVmInputManager {
    /// Create a new multi-VM input manager
    pub fn new() -> Self {
        Self {
            managers: HashMap::new(),
            default_vm: None,
        }
    }

    /// Register a VM's input manager
    pub fn register(&mut self, vm_id: String, manager: InputManager) {
        if self.managers.is_empty() {
            self.default_vm = Some(vm_id.clone());
        }
        self.managers.insert(vm_id, Arc::new(Mutex::new(manager)));
    }

    /// Unregister a VM's input manager
    pub fn unregister(&mut self, vm_id: &str) {
        self.managers.remove(vm_id);
        if self.default_vm.as_ref() == Some(&vm_id.to_string()) {
            self.default_vm = self.managers.keys().next().cloned();
        }
    }

    /// Get a VM's input manager
    pub fn get(&self, vm_id: &str) -> Option<Arc<Mutex<InputManager>>> {
        if vm_id.is_empty() {
            self.default_vm
                .as_ref()
                .and_then(|id| self.managers.get(id).cloned())
        } else {
            self.managers.get(vm_id).cloned()
        }
    }

    /// List all registered VMs
    pub fn list_vms(&self) -> Vec<&String> {
        self.managers.keys().collect()
    }

    /// Inject input to a specific VM
    pub fn inject(&self, vm_id: &str, request: &InputRequest) -> Result<InputStats> {
        let manager = self.get(vm_id).ok_or_else(|| {
            InputError::BackendNotAvailable(format!("VM not found: {}", vm_id))
        })?;

        let mut mgr = manager.lock().map_err(|_| {
            InputError::InjectionFailed("Failed to lock input manager".to_string())
        })?;

        mgr.process_request(request)
    }
}

impl Default for MultiVmInputManager {
    fn default() -> Self {
        Self::new()
    }
}
