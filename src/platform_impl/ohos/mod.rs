// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

pub(crate) mod icon;

pub(crate) use icon::PlatformIcon;

use crate::{
    accelerator::KeyAccelerator,
    icon::{Icon, NativeIcon},
    items::PredefinedMenuItemType,
    util::{AddOp, Counter},
    IsMenuItem, MenuId, MenuItemType,
};
use keyboard_types::{Key, Modifiers};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
};

impl fmt::Display for KeyAccelerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(Modifiers::CONTROL) {
            write!(f, "Ctrl+")?;
        }
        if self.mods.contains(Modifiers::SHIFT) {
            write!(f, "Shift+")?;
        }
        if self.mods.contains(Modifiers::ALT) {
            write!(f, "Alt+")?;
        }
        if self.mods.contains(Modifiers::SUPER) {
            write!(f, "Super+")?;
        }
        match &self.key {
            Key::Character(s) => match s.as_str() {
                " " => write!(f, "Space"),
                c => write!(f, "{}", c.to_uppercase()),
            },
            Key::Tab => write!(f, "Tab"),
            Key::Escape => write!(f, "Esc"),
            Key::Delete => write!(f, "Del"),
            Key::Insert => write!(f, "Ins"),
            Key::PageUp => write!(f, "PgUp"),
            Key::PageDown => write!(f, "PgDn"),
            Key::ArrowLeft => write!(f, "Left"),
            Key::ArrowRight => write!(f, "Right"),
            Key::ArrowUp => write!(f, "Up"),
            Key::ArrowDown => write!(f, "Down"),
            key => write!(f, "{:?}", key),
        }
    }
}

static COUNTER: Counter = Counter::new();

static CHECK_ITEMS: once_cell::sync::Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

// Use openharmony-ability's MenuItemData for serialization
use openharmony_ability::menu::MenuItemData;

pub struct Menu {
    id: MenuId,
    children: Vec<Rc<RefCell<MenuChild>>>,
}

impl Menu {
    pub fn new(id: Option<MenuId>) -> Self {
        Self {
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            children: Vec::new(),
        }
    }

    pub fn id(&self) -> &MenuId {
        &self.id
    }

    pub fn add_menu_item(&mut self, item: &dyn IsMenuItem, op: AddOp) -> crate::Result<()> {
        match op {
            AddOp::Append => self.children.push(item.child()),
            AddOp::Insert(position) => self.children.insert(position, item.child()),
        }
        Ok(())
    }

    pub fn remove(&mut self, item: &dyn IsMenuItem) -> crate::Result<()> {
        let index = self
            .children
            .iter()
            .position(|e: &Rc<RefCell<MenuChild>>| e.borrow().id == item.id())
            .ok_or(crate::Error::NotAChildOfThisMenu)?;
        self.children.remove(index);
        Ok(())
    }

    pub fn items(&self) -> Vec<crate::MenuItemKind> {
        self.children
            .iter()
            .map(|c: &Rc<RefCell<MenuChild>>| c.borrow().kind(c.clone()))
            .collect()
    }

    pub fn to_menu_items(&self) -> Vec<MenuItemData> {
        self.children
            .iter()
            .map(|c| c.borrow().to_menu_item_data())
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_menu_items()).unwrap_or_default()
    }

    pub fn popup(&self, x: Option<f64>, y: Option<f64>, window_id: &str) -> crate::Result<()> {
        init_menu_event_listener();
        collect_check_items(&self.children);
        let json = self.to_json();
        openharmony_ability::menu::popup_context_menu(json, x, y, window_id.to_string())
            .map_err(|e| crate::Error::CustomError(e.to_string()))?;
        Ok(())
    }

    pub fn refresh_menubar(&self, window_id: &str) -> crate::Result<()> {
        init_menu_event_listener();
        let json = self.to_json();
        openharmony_ability::menu::set_menu_json(json, window_id.to_string())
            .map_err(|e| crate::Error::CustomError(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MenuChild {
    item_type: MenuItemType,
    text: String,
    enabled: bool,
    id: MenuId,
    accelerator: Option<KeyAccelerator>,
    predefined_item_type: Option<PredefinedMenuItemType>,
    checked: Option<Arc<AtomicBool>>,
    is_syncing_checked_state: Option<Arc<AtomicBool>>,
    icon: Option<Icon>,
    native_icon: Option<String>,
    pub children: Option<Vec<Rc<RefCell<MenuChild>>>>,
}

impl MenuChild {
    pub fn new(
        text: &str,
        enabled: bool,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::MenuItem,
            checked: None,
            children: None,
            icon: None,
            native_icon: None,
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
    }

    pub fn new_submenu(text: &str, enabled: bool, id: Option<MenuId>) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            children: Some(Vec::new()),
            item_type: MenuItemType::Submenu,
            icon: None,
            native_icon: None,
            is_syncing_checked_state: None,
            predefined_item_type: None,
            accelerator: None,
            checked: None,
        }
    }

    pub(crate) fn new_predefined(item_type: PredefinedMenuItemType, text: Option<String>) -> Self {
        Self {
            text: text.unwrap_or_else(|| item_type.text().to_string()),
            enabled: true,
            accelerator: item_type.accelerator().map(Into::into),
            id: MenuId(COUNTER.next().to_string()),
            item_type: MenuItemType::Predefined,
            predefined_item_type: Some(item_type),
            checked: None,
            children: None,
            icon: None,
            native_icon: None,
            is_syncing_checked_state: None,
        }
    }

    pub fn new_check(
        text: &str,
        enabled: bool,
        checked: bool,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            checked: Some(Arc::new(AtomicBool::new(checked))),
            is_syncing_checked_state: Some(Arc::new(AtomicBool::new(false))),
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Check,
            children: None,
            icon: None,
            native_icon: None,
            predefined_item_type: None,
        }
    }

    pub fn new_icon(
        text: &str,
        enabled: bool,
        icon: Option<Icon>,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            icon,
            native_icon: None,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Icon,
            children: None,
            checked: None,
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
    }

    pub fn new_native_icon(
        text: &str,
        enabled: bool,
        native_icon: Option<NativeIcon>,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Icon,
            children: None,
            checked: None,
            icon: None,
            native_icon: native_icon.and_then(native_icon_to_ohos).map(|s| s.to_string()),
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
    }
}

/// Map NativeIcon variants to OHOS system symbol resource names.
///
/// Only a few NativeIcon variants have confirmed OHOS system symbol equivalents
/// at API 12 (compileSdkVersion 5.0.0). Most of the 56 NativeIcon variants are
/// macOS-specific UI metaphors with no OHOS counterpart.
///
/// Validated symbols: ohos_star (Add), ohos_lock (LockLocked), ohos_wifi (Network).
/// All other variants map to `None` (no icon), consistent with Windows/Linux behavior
/// where unmapped NativeIcons render without an icon.
///
/// When the SDK adds more system symbols (e.g. ohos_trash, ohos_share), this mapping
/// can be extended. The ArkTS side (MenuBarComponent.nativeIconSymbol) must also be
/// updated with a matching `$r()` case.
fn native_icon_to_ohos(icon: NativeIcon) -> Option<&'static str> {
    match icon {
        NativeIcon::Add => Some("sys.symbol.ohos_star"),
        NativeIcon::LockLocked => Some("sys.symbol.ohos_lock"),
        NativeIcon::Network => Some("sys.symbol.ohos_wifi"),
        // All other variants: no confirmed system symbol at API 12
        _ => None,
    }
}

impl MenuChild {
    pub(crate) fn item_type(&self) -> MenuItemType {
        self.item_type
    }

    pub fn to_menu_item_data(&self) -> MenuItemData {
        let item_type = match self.item_type {
            MenuItemType::MenuItem => "item",
            MenuItemType::Submenu => "submenu",
            MenuItemType::Predefined => "predefined",
            MenuItemType::Check => "check",
            MenuItemType::Icon => "icon",
        };

        let predefined_type = self.predefined_item_type.as_ref().map(|t| {
            match t {
                PredefinedMenuItemType::Separator => "separator",
                PredefinedMenuItemType::Copy => "copy",
                PredefinedMenuItemType::Cut => "cut",
                PredefinedMenuItemType::Paste => "paste",
                PredefinedMenuItemType::SelectAll => "selectAll",
                PredefinedMenuItemType::Undo => "undo",
                PredefinedMenuItemType::Redo => "redo",
                PredefinedMenuItemType::Minimize => "minimize",
                PredefinedMenuItemType::Maximize => "maximize",
                PredefinedMenuItemType::Fullscreen => "fullscreen",
                PredefinedMenuItemType::CloseWindow => "close",
                PredefinedMenuItemType::Quit => "quit",
                PredefinedMenuItemType::Hide => "hide",
                PredefinedMenuItemType::HideOthers => "hideOthers",
                PredefinedMenuItemType::ShowAll => "showAll",
                PredefinedMenuItemType::About(_) => "about",
                PredefinedMenuItemType::Services => "services",
                PredefinedMenuItemType::BringAllToFront => "bringAllToFront",
                PredefinedMenuItemType::None => "none",
            }
        }).map(|s| s.to_string());

        MenuItemData {
            id: self.id.0.clone(),
            item_type: item_type.to_string(),
            text: Some(self.text.replace("&", "")),
            enabled: Some(self.enabled),
            accelerator: self.accelerator.clone().map(|k| k.to_string()),
            predefined_type,
            checked: self.checked.as_ref().map(|c| c.load(Ordering::Relaxed)),
            icon: self.icon.as_ref().map(|i| {
                let png_data = encode_rgba_to_png(&i.inner.raw, i.inner.width, i.inner.height);
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_data)
            }),
            native_icon: self.native_icon.clone(),
            submenu_items: self.children.as_ref().map(|c| {
                c.iter().map(|child| child.borrow().to_menu_item_data()).collect()
            }),
            about_metadata: self.predefined_item_type.as_ref().and_then(|t| {
                if let PredefinedMenuItemType::About(ref metadata) = t {
                    metadata.as_ref().map(|m| openharmony_ability::menu::AboutMetadataData {
                        name: m.name.clone(),
                        version: m.version.clone(),
                        short_version: m.short_version.clone(),
                        authors: m.authors.clone(),
                        comments: m.comments.clone(),
                        copyright: m.copyright.clone(),
                        license: m.license.clone(),
                        website: m.website.clone(),
                    })
                } else {
                    None
                }
            }),
        }
    }

    pub fn id(&self) -> &MenuId {
        &self.id
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_key_accelerator(
        &mut self,
        accelerator: Option<KeyAccelerator>,
    ) -> crate::Result<()> {
        self.accelerator = accelerator;
        Ok(())
    }
}

impl MenuChild {
    pub fn is_checked(&self) -> bool {
        self.checked
            .as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn set_checked(&mut self, checked: bool) {
        if let Some(c) = &self.checked {
            c.store(checked, Ordering::Release);
        }
        if let Some(is_syncing) = &self.is_syncing_checked_state {
            is_syncing.store(false, Ordering::Release);
        }
    }
}

impl MenuChild {
pub fn set_icon(&mut self, icon: Option<Icon>) {
        self.icon = icon;
    }

    pub fn set_native_icon(&mut self, icon: Option<NativeIcon>) {
        self.native_icon = icon.and_then(native_icon_to_ohos).map(|s| s.to_string());
    }
}

impl MenuChild {
    pub fn add_menu_item(&mut self, item: &dyn IsMenuItem, op: AddOp) -> crate::Result<()> {
        match op {
            AddOp::Append => self.children.as_mut().unwrap().push(item.child()),
            AddOp::Insert(position) => self
                .children
                .as_mut()
                .unwrap()
                .insert(position, item.child()),
        }
        Ok(())
    }

    pub fn remove(&mut self, item: &dyn IsMenuItem) -> crate::Result<()> {
        let index = self
            .children
            .as_ref()
            .unwrap()
            .iter()
            .position(|e: &Rc<RefCell<MenuChild>>| e.borrow().id == item.id())
            .ok_or(crate::Error::NotAChildOfThisMenu)?;
        self.children.as_mut().unwrap().remove(index);
        Ok(())
    }

    pub fn items(&self) -> Vec<crate::MenuItemKind> {
        self.children
            .as_ref()
            .unwrap()
            .iter()
            .map(|c: &Rc<RefCell<MenuChild>>| c.borrow().kind(c.clone()))
            .collect()
    }

    pub fn to_json(&self) -> String {
        let items: Vec<MenuItemData> = self.children.as_ref().map(|c| {
            c.iter().map(|child| child.borrow().to_menu_item_data()).collect()
        }).unwrap_or_default();
        serde_json::to_string(&items).unwrap_or_default()
    }

    pub fn popup(&self, x: Option<f64>, y: Option<f64>, window_id: &str) -> crate::Result<()> {
        init_menu_event_listener();
        if let Some(ref children) = self.children {
            collect_check_items(children);
        }
        let json = self.to_json();
        openharmony_ability::menu::popup_context_menu(json, x, y, window_id.to_string())
            .map_err(|e| crate::Error::CustomError(e.to_string()))?;
        Ok(())
    }
}

fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
    }
    png_data
}

static EVENT_LISTENER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn collect_check_items(children: &[Rc<RefCell<MenuChild>>]) {
    let mut guard = CHECK_ITEMS.lock().unwrap();
    guard.clear();
    for child in children {
        collect_check_item_recursive(&child.borrow(), &mut guard);
    }
}

fn collect_check_item_recursive(child: &MenuChild, map: &mut HashMap<String, Arc<AtomicBool>>) {
    if child.item_type == MenuItemType::Check {
        if let Some(ref checked) = child.checked {
            map.insert(child.id.0.clone(), checked.clone());
        }
    }
    if let Some(ref sub_children) = child.children {
        for sub in sub_children {
            collect_check_item_recursive(&sub.borrow(), map);
        }
    }
}

fn start_event_listener() {
    if EVENT_LISTENER_STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }

    std::thread::spawn(|| {
        let receiver = openharmony_ability::menu::menu_event_receiver();
        while let Ok(menu_id) = receiver.recv() {
            {
                let guard = CHECK_ITEMS.lock().unwrap();
                if let Some(checked) = guard.get(&menu_id) {
                    let old = checked.load(Ordering::Relaxed);
                    checked.store(!old, Ordering::Release);
                }
            }
            crate::MenuEvent::send(crate::MenuEvent {
                id: crate::MenuId::new(menu_id),
            });
        }
    });
}

pub fn init_menu_event_listener() {
    start_event_listener();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::PredefinedMenuItemType;
    use keyboard_types::{Key, Modifiers};

    #[test]
    fn to_menu_item_data_accelerator() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()));
        let child = MenuChild::new("Open", true, Some(accel), None);
        let data = child.to_menu_item_data();
        assert!(data.accelerator.is_some());
        assert_eq!(data.accelerator.unwrap(), "Ctrl+O");
    }

    #[test]
    fn submenu_accelerator_preserved() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Key::Character("S".into()));
        let mut child = MenuChild::new_submenu("Save", true, None);
        child.set_key_accelerator(Some(accel)).unwrap();
        let data = child.to_menu_item_data();
        assert!(data.accelerator.is_some());
        assert_eq!(data.accelerator.unwrap(), "Ctrl+Shift+S");
    }

    #[test]
    fn about_metadata_in_menu_item_data() {
        let metadata = crate::AboutMetadata {
            name: Some("TestApp".to_string()),
            version: Some("1.0.0".to_string()),
            short_version: Some("1.0".to_string()),
            authors: Some(vec!("Alice".to_string(), "Bob".to_string())),
            copyright: Some("TestCorp".to_string()),
            ..Default::default()
        };
        let child = MenuChild::new_predefined(
            PredefinedMenuItemType::About(Some(metadata)),
            None,
        );
        let data = child.to_menu_item_data();
        assert!(data.about_metadata.is_some());
        let meta = data.about_metadata.unwrap();
        assert_eq!(meta.name.unwrap(), "TestApp");
        assert_eq!(meta.version.unwrap(), "1.0.0");
        assert_eq!(meta.short_version.unwrap(), "1.0");
        assert_eq!(meta.authors.unwrap(), vec!("Alice".to_string(), "Bob".to_string()));
        assert_eq!(meta.copyright.unwrap(), "TestCorp");
        assert!(meta.comments.is_none());
        assert!(meta.license.is_none());
        assert!(meta.website.is_none());
    }

    #[test]
    fn predefined_about_without_metadata() {
        let child = MenuChild::new_predefined(
            PredefinedMenuItemType::About(None),
            Some("About".to_string()),
        );
        let data = child.to_menu_item_data();
        assert!(data.about_metadata.is_none());
        assert_eq!(data.predefined_type.unwrap(), "about");
    }

    #[test]
    fn key_accelerator_display_ctrl_only() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()));
        assert_eq!(accel.to_string(), "Ctrl+O");
    }

    #[test]
    fn key_accelerator_display_ctrl_shift() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Key::Character("S".into()));
        assert_eq!(accel.to_string(), "Ctrl+Shift+S");
    }

    #[test]
    fn key_accelerator_display_ctrl_alt() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::ALT), Key::Character("D".into()));
        assert_eq!(accel.to_string(), "Ctrl+Alt+D");
    }

    #[test]
    fn key_accelerator_display_full_combo() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT | Modifiers::ALT), Key::Character("X".into()));
        assert_eq!(accel.to_string(), "Ctrl+Shift+Alt+X");
    }

    #[test]
    fn key_accelerator_display_no_modifiers() {
        let accel = KeyAccelerator::new(None, Key::Character("A".into()));
        assert_eq!(accel.to_string(), "A");
    }

    #[test]
    fn key_accelerator_display_special_keys() {
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Escape).to_string(), "Ctrl+Esc");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Delete).to_string(), "Ctrl+Del");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Insert).to_string(), "Ctrl+Ins");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::PageUp).to_string(), "Ctrl+PgUp");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::PageDown).to_string(), "Ctrl+PgDn");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Tab).to_string(), "Ctrl+Tab");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character(" ".into())).to_string(), "Ctrl+Space");
    }

    #[test]
    fn key_accelerator_display_arrow_keys() {
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowLeft).to_string(), "Ctrl+Left");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowRight).to_string(), "Ctrl+Right");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowUp).to_string(), "Ctrl+Up");
        assert_eq!(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::ArrowDown).to_string(), "Ctrl+Down");
    }

    #[test]
    fn key_accelerator_display_lowercase_input() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("o".into()));
        assert_eq!(accel.to_string(), "Ctrl+O");
    }

    #[test]
    fn menu_child_disabled_item() {
        let child = MenuChild::new("Disabled", false, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.enabled, Some(false));
        assert_eq!(data.text, Some("Disabled".to_string()));
        assert!(data.accelerator.is_none());
    }

    #[test]
    fn menu_child_enabled_item() {
        let child = MenuChild::new("Enabled", true, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.enabled, Some(true));
    }

    #[test]
    fn menu_child_ampersand_stripped() {
        let child = MenuChild::new("Save &As", true, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("Save As".to_string()));
    }

    #[test]
    fn menu_child_double_ampersand_stripped() {
        let child = MenuChild::new("A&&B", true, None, None);
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("AB".to_string()));
    }

    #[test]
    fn menu_child_checked_item() {
        let mut child = MenuChild::new_check("Toggle", true, true, None, None);
        child.checked = Some(Arc::new(AtomicBool::new(true)));
        let data = child.to_menu_item_data();
        assert_eq!(data.checked, Some(true));
        assert_eq!(data.item_type, "check");
    }

    #[test]
    fn menu_child_checked_item_false() {
        let mut child = MenuChild::new_check("Toggle", true, false, None, None);
        child.checked = Some(Arc::new(AtomicBool::new(false)));
        let data = child.to_menu_item_data();
        assert_eq!(data.checked, Some(false));
    }

    #[test]
    fn menu_child_submenu_with_nested_items() {
        let mut submenu = MenuChild::new_submenu("File", true, None);
        let open_item = MenuChild::new("Open", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()))), None);
        let save_item = MenuChild::new("Save", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Key::Character("S".into()))), None);
        submenu.children = Some(vec![Rc::new(RefCell::new(open_item)), Rc::new(RefCell::new(save_item))]);
        let data = submenu.to_menu_item_data();
        assert_eq!(data.item_type, "submenu");
        assert_eq!(data.text, Some("File".to_string()));
        let children = data.submenu_items.unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].accelerator, Some("Ctrl+O".to_string()));
        assert_eq!(children[1].accelerator, Some("Ctrl+Shift+S".to_string()));
    }

    #[test]
    fn menu_child_submenu_empty() {
        let submenu = MenuChild::new_submenu("Empty", true, None);
        let data = submenu.to_menu_item_data();
        assert!(data.submenu_items.is_some());
        assert_eq!(data.submenu_items.unwrap().len(), 0);
    }

    #[test]
    fn menu_child_set_text() {
        let mut child = MenuChild::new("Original", true, None, None);
        child.set_text("Updated");
        let data = child.to_menu_item_data();
        assert_eq!(data.text, Some("Updated".to_string()));
    }

    #[test]
    fn menu_child_set_enabled() {
        let mut child = MenuChild::new("Item", true, None, None);
        child.set_enabled(false);
        let data = child.to_menu_item_data();
        assert_eq!(data.enabled, Some(false));
    }

    #[test]
    fn menu_child_set_key_accelerator() {
        let mut child = MenuChild::new("Item", true, None, None);
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("Q".into()));
        child.set_key_accelerator(Some(accel)).unwrap();
        let data = child.to_menu_item_data();
        assert_eq!(data.accelerator, Some("Ctrl+Q".to_string()));
    }

    #[test]
    fn menu_child_remove_accelerator() {
        let accel = KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("P".into()));
        let mut child = MenuChild::new("Print", true, Some(accel), None);
        child.set_key_accelerator(None).unwrap();
        let data = child.to_menu_item_data();
        assert!(data.accelerator.is_none());
    }

    #[test]
    fn menu_child_predefined_hide() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Hide, Some("Hide".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.item_type, "predefined");
        assert_eq!(data.predefined_type.unwrap(), "hide");
    }

    #[test]
    fn menu_child_predefined_close() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::CloseWindow, Some("Close".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "close");
    }

    #[test]
    fn menu_child_predefined_quit() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Quit, Some("Quit".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "quit");
    }

    #[test]
    fn menu_child_predefined_fullscreen() {
        let child = MenuChild::new_predefined(PredefinedMenuItemType::Fullscreen, Some("Fullscreen".to_string()));
        let data = child.to_menu_item_data();
        assert_eq!(data.predefined_type.unwrap(), "fullscreen");
    }

    #[test]
    fn menu_to_json_structure() {
        let mut menu = Menu::new(None);
        let item = MenuChild::new("Open", true, Some(KeyAccelerator::new(Some(Modifiers::CONTROL), Key::Character("O".into()))), None);
        menu.children.push(Rc::new(RefCell::new(item)));
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.is_array());
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "item");
        assert_eq!(items[0]["text"], "Open");
        assert_eq!(items[0]["accelerator"], "Ctrl+O");
    }

    #[test]
    fn menu_to_json_submenu_nested() {
        let mut menu = Menu::new(None);
        let mut submenu = MenuChild::new_submenu("File", true, None);
        let open = MenuChild::new("Open", true, None, Some(MenuId::new("open_id")));
        submenu.children = Some(vec![Rc::new(RefCell::new(open))]);
        menu.children.push(Rc::new(RefCell::new(submenu)));
        let json_str = menu.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let top = parsed.as_array().unwrap();
        assert_eq!(top[0]["type"], "submenu");
        let children = top[0]["submenuItems"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["type"], "item");
        assert_eq!(children[0]["id"], "open_id");
    }
}
