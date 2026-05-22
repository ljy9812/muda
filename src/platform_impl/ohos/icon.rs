// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::icon::BadIcon;

#[derive(Debug, Clone)]
pub(crate) struct PlatformIcon {
    pub(crate) raw: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PlatformIcon {
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, BadIcon> {
        let pixel_count = rgba.len() / 4;
        if pixel_count != (width * height) as usize {
            return Err(BadIcon::DimensionsVsPixelCount {
                width,
                height,
                width_x_height: (width * height) as usize,
                pixel_count,
            });
        }
        Ok(Self {
            raw: rgba,
            width,
            height,
        })
    }
}