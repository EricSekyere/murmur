use anyhow::{Context, Result};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager as GhkManager, HotKeyState, hotkey::HotKey,
};
use std::sync::mpsc;

/// Events emitted by the hotkey manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Push-to-talk key was pressed.
    Pressed,
    /// Push-to-talk key was released.
    Released,
}

/// Manages global hotkey registration for push-to-talk.
pub struct HotkeyManager {
    _manager: GhkManager,
    hotkey: HotKey,
}

impl HotkeyManager {
    /// Create a new hotkey manager and register the given hotkey string.
    ///
    /// Format: "Ctrl+Shift+Space", "Super+Shift+Space", etc.
    /// Uses the `global-hotkey` crate's string parser.
    pub fn new(hotkey_str: &str) -> Result<Self> {
        let manager = GhkManager::new().context("Failed to create hotkey manager")?;

        let hotkey: HotKey = hotkey_str
            .parse()
            .map_err(|e| anyhow::anyhow!("Failed to parse hotkey '{}': {:?}", hotkey_str, e))?;

        manager
            .register(hotkey)
            .map_err(|e| anyhow::anyhow!("Failed to register hotkey: {:?}", e))?;

        tracing::info!("Registered global hotkey: {}", hotkey_str);

        Ok(Self {
            _manager: manager,
            hotkey,
        })
    }

    /// Set up a callback-based event handler for hotkey events.
    /// Returns a receiver that yields HotkeyEvents.
    pub fn events(&self) -> mpsc::Receiver<HotkeyEvent> {
        let (tx, rx) = mpsc::channel();
        let hotkey_id = self.hotkey.id();

        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id() == hotkey_id {
                let hk_event = match event.state() {
                    HotKeyState::Pressed => HotkeyEvent::Pressed,
                    HotKeyState::Released => HotkeyEvent::Released,
                };
                let _ = tx.send(hk_event);
            }
        }));

        rx
    }

    pub fn hotkey_id(&self) -> u32 {
        self.hotkey.id()
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        GlobalHotKeyEvent::set_event_handler(None::<fn(GlobalHotKeyEvent)>);
        if let Err(e) = self._manager.unregister(self.hotkey) {
            tracing::warn!("Failed to unregister hotkey on drop: {:?}", e);
        }
    }
}
