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
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
};

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

    pub fn popup(&self, x: Option<f64>, y: Option<f64>) -> crate::Result<()> {
        init_menu_event_listener();
        collect_check_items(&self.children);
        let json = self.to_json();
        openharmony_ability::menu::popup_context_menu(json, x, y)
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
        _native_icon: Option<NativeIcon>,
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
            is_syncing_checked_state: None,
            predefined_item_type: None,
        }
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
            accelerator: None,
            predefined_type,
            checked: self.checked.as_ref().map(|c| c.load(Ordering::Relaxed)),
            icon: self.icon.as_ref().map(|i| {
                let png_data = encode_rgba_to_png(&i.inner.raw, i.inner.width, i.inner.height);
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_data)
            }),
            submenu_items: self.children.as_ref().map(|c| {
                c.iter().map(|child| child.borrow().to_menu_item_data()).collect()
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

    pub fn popup(&self, x: Option<f64>, y: Option<f64>) -> crate::Result<()> {
        init_menu_event_listener();
        if let Some(ref children) = self.children {
            collect_check_items(children);
        }
        let json = self.to_json();
        openharmony_ability::menu::popup_context_menu(json, x, y)
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