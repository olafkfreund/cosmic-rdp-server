//! Unsafe FFI helpers for extracting SPA metadata from `PipeWire` buffers.
//!
//! `PipeWire` attaches metadata to buffers via the SPA (Simple Plugin API)
//! metadata system. The safe `pipewire-rs` wrapper does not expose this
//! metadata, so we access it through raw pointers to the underlying
//! `spa_buffer` structure.
//!
//! Two metadata types are extracted:
//! - `SPA_META_VideoDamage` (type 3): array of damage rectangles
//! - `SPA_META_Cursor` (type 5): cursor position and optional bitmap

use pipewire::spa::sys as spa_sys;

use crate::frame::{CursorBitmap, CursorInfo, DamageRect};

/// Extract damage rectangles from a `PipeWire` buffer's `SPA_META_VideoDamage` metadata.
///
/// Returns `None` if no damage metadata is present (treat as full-frame damage).
/// Returns `Some(vec![])` if damage metadata is present but empty (no changes).
///
/// # Safety
///
/// The `spa_buffer` pointer must be valid for the duration of this call.
/// This is guaranteed when called from within the `PipeWire` process callback
/// while the buffer is dequeued.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn extract_damage(
    spa_buffer: *const spa_sys::spa_buffer,
) -> Option<Vec<DamageRect>> {
    if spa_buffer.is_null() {
        return None;
    }

    let buffer = &*spa_buffer;
    if buffer.n_metas == 0 || buffer.metas.is_null() {
        return None;
    }

    let metas = std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize);

    for meta in metas {
        if meta.type_ != spa_sys::SPA_META_VideoDamage {
            continue;
        }

        if meta.data.is_null() || meta.size == 0 {
            return None;
        }

        let region_size = std::mem::size_of::<spa_sys::spa_meta_region>();
        let max_regions = meta.size as usize / region_size;

        if max_regions == 0 {
            return None;
        }

        let regions = meta.data.cast::<spa_sys::spa_meta_region>();
        let mut damage_rects = Vec::new();

        for i in 0..max_regions {
            let region = &*regions.add(i);
            let w = region.region.size.width;
            let h = region.region.size.height;

            // Zero-size region marks end of array (per SPA spec).
            if w == 0 && h == 0 {
                break;
            }

            damage_rects.push(DamageRect::new(
                region.region.position.x,
                region.region.position.y,
                w,
                h,
            ));
        }

        tracing::trace!(
            count = damage_rects.len(),
            "Extracted damage rects from PipeWire metadata"
        );
        return Some(damage_rects);
    }

    None
}

/// Extract cursor metadata from a `PipeWire` buffer's `SPA_META_Cursor` metadata.
///
/// Returns `None` if no cursor metadata is present. When the portal uses
/// `CursorMode::Metadata`, the compositor attaches cursor position and
/// optional bitmap data to each buffer.
///
/// # Safety
///
/// The `spa_buffer` pointer must be valid for the duration of this call.
/// This is guaranteed when called from within the `PipeWire` process callback
/// while the buffer is dequeued.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn extract_cursor(
    spa_buffer: *const spa_sys::spa_buffer,
) -> Option<CursorInfo> {
    if spa_buffer.is_null() {
        return None;
    }

    let buffer = &*spa_buffer;
    if buffer.n_metas == 0 || buffer.metas.is_null() {
        return None;
    }

    let metas = std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize);

    for meta in metas {
        if meta.type_ != spa_sys::SPA_META_Cursor {
            continue;
        }

        if meta.data.is_null()
            || (meta.size as usize) < std::mem::size_of::<spa_sys::spa_meta_cursor>()
        {
            return None;
        }

        let cursor = &*(meta.data.cast::<spa_sys::spa_meta_cursor>());

        // id == 0 means no cursor data (cursor left the captured region).
        if cursor.id == 0 {
            return Some(CursorInfo {
                x: cursor.position.x,
                y: cursor.position.y,
                visible: false,
                bitmap: None,
            });
        }

        let bitmap = extract_cursor_bitmap(meta.data, cursor);

        return Some(CursorInfo {
            x: cursor.position.x,
            y: cursor.position.y,
            visible: true,
            bitmap,
        });
    }

    None
}

/// Extract the cursor bitmap from a `spa_meta_cursor`, if present.
///
/// # Safety
///
/// `meta_data` must point to valid memory of at least
/// `cursor.bitmap_offset + sizeof(spa_meta_bitmap)` bytes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
unsafe fn extract_cursor_bitmap(
    meta_data: *mut std::os::raw::c_void,
    cursor: &spa_sys::spa_meta_cursor,
) -> Option<CursorBitmap> {
    // bitmap_offset == 0 means no bitmap data.
    if cursor.bitmap_offset == 0 {
        return None;
    }

    let bitmap_size = std::mem::size_of::<spa_sys::spa_meta_bitmap>();
    if (cursor.bitmap_offset as usize) < bitmap_size {
        return None;
    }

    #[allow(clippy::cast_ptr_alignment)] // SPA guarantees 4-byte aligned metadata
    let bitmap_ptr = meta_data
        .cast::<u8>()
        .add(cursor.bitmap_offset as usize)
        .cast::<spa_sys::spa_meta_bitmap>();
    let bitmap = &*bitmap_ptr;

    // offset == 0 in the bitmap means no pixel data (invisible cursor).
    if bitmap.offset == 0 {
        return None;
    }

    let width = bitmap.size.width;
    let height = bitmap.size.height;

    if width == 0 || height == 0 {
        return None;
    }

    // Validate format: we only handle ARGB8888.
    if bitmap.format != spa_sys::SPA_VIDEO_FORMAT_ARGB {
        tracing::debug!(
            format = bitmap.format,
            "Unsupported cursor bitmap format (expected ARGB8888)"
        );
        return None;
    }

    let stride = bitmap.stride.unsigned_abs() as usize;
    let expected_size = stride * height as usize;

    // Read pixel data from the bitmap's offset field.
    let pixel_ptr = bitmap_ptr
        .cast::<u8>()
        .add(bitmap.offset as usize);
    let pixel_data = std::slice::from_raw_parts(pixel_ptr, expected_size);

    // Convert ARGB8888 (A,R,G,B on big-endian / B,G,R,A on little-endian)
    // to RGBA. On little-endian (x86), the memory layout for ARGB is
    // [B, G, R, A] and we need [R, G, B, A].
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let row_start = row * stride;
        for col in 0..width as usize {
            let px = row_start + col * 4;
            if px + 3 < pixel_data.len() {
                // ARGB on LE: [B, G, R, A] -> RGBA: [R, G, B, A]
                rgba.push(pixel_data[px + 2]); // R
                rgba.push(pixel_data[px + 1]); // G
                rgba.push(pixel_data[px]);     // B
                rgba.push(pixel_data[px + 3]); // A
            }
        }
    }

    Some(CursorBitmap {
        width,
        height,
        hot_x: cursor.hotspot.x.max(0) as u32,
        hot_y: cursor.hotspot.y.max(0) as u32,
        data: rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_damage_null_buffer() {
        let result = unsafe { extract_damage(std::ptr::null()) };
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_damage_no_metas() {
        let buffer = spa_sys::spa_buffer {
            n_metas: 0,
            n_datas: 0,
            metas: std::ptr::null_mut(),
            datas: std::ptr::null_mut(),
        };
        let result = unsafe { extract_damage(&raw const buffer) };
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_damage_wrong_meta_type() {
        let mut meta = spa_sys::spa_meta {
            type_: spa_sys::SPA_META_Header,
            size: 64,
            data: std::ptr::null_mut(),
        };

        let buffer = spa_sys::spa_buffer {
            n_metas: 1,
            n_datas: 0,
            metas: &raw mut meta,
            datas: std::ptr::null_mut(),
        };

        let result = unsafe { extract_damage(&raw const buffer) };
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_damage_with_regions() {
        let regions = [
            spa_sys::spa_meta_region {
                region: spa_sys::spa_region {
                    position: spa_sys::spa_point { x: 10, y: 20 },
                    size: spa_sys::spa_rectangle {
                        width: 100,
                        height: 50,
                    },
                },
            },
            spa_sys::spa_meta_region {
                region: spa_sys::spa_region {
                    position: spa_sys::spa_point { x: 200, y: 300 },
                    size: spa_sys::spa_rectangle {
                        width: 64,
                        height: 32,
                    },
                },
            },
            // Zero-size terminator
            spa_sys::spa_meta_region {
                region: spa_sys::spa_region {
                    position: spa_sys::spa_point { x: 0, y: 0 },
                    size: spa_sys::spa_rectangle {
                        width: 0,
                        height: 0,
                    },
                },
            },
        ];

        let mut meta = spa_sys::spa_meta {
            type_: spa_sys::SPA_META_VideoDamage,
            #[allow(clippy::cast_possible_truncation)]
            size: std::mem::size_of_val(&regions) as u32,
            data: regions.as_ptr().cast_mut().cast::<std::os::raw::c_void>(),
        };

        let buffer = spa_sys::spa_buffer {
            n_metas: 1,
            n_datas: 0,
            metas: &raw mut meta,
            datas: std::ptr::null_mut(),
        };

        let result = unsafe { extract_damage(&raw const buffer) };
        assert!(result.is_some());

        let rects = result.unwrap();
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], DamageRect::new(10, 20, 100, 50));
        assert_eq!(rects[1], DamageRect::new(200, 300, 64, 32));
    }

    #[test]
    fn test_extract_damage_null_meta_data() {
        let mut meta = spa_sys::spa_meta {
            type_: spa_sys::SPA_META_VideoDamage,
            size: 64,
            data: std::ptr::null_mut(),
        };

        let buffer = spa_sys::spa_buffer {
            n_metas: 1,
            n_datas: 0,
            metas: &raw mut meta,
            datas: std::ptr::null_mut(),
        };

        let result = unsafe { extract_damage(&raw const buffer) };
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_cursor_null_buffer() {
        let result = unsafe { extract_cursor(std::ptr::null()) };
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_cursor_no_metas() {
        let buffer = spa_sys::spa_buffer {
            n_metas: 0,
            n_datas: 0,
            metas: std::ptr::null_mut(),
            datas: std::ptr::null_mut(),
        };
        let result = unsafe { extract_cursor(&raw const buffer) };
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_cursor_invisible() {
        // id == 0 means cursor not in captured region.
        let cursor_meta = spa_sys::spa_meta_cursor {
            id: 0,
            flags: 0,
            position: spa_sys::spa_point { x: 100, y: 200 },
            hotspot: spa_sys::spa_point { x: 0, y: 0 },
            bitmap_offset: 0,
        };

        let mut meta = spa_sys::spa_meta {
            type_: spa_sys::SPA_META_Cursor,
            #[allow(clippy::cast_possible_truncation)]
            size: std::mem::size_of::<spa_sys::spa_meta_cursor>() as u32,
            data: std::ptr::addr_of!(cursor_meta).cast_mut().cast::<std::os::raw::c_void>(),
        };

        let buffer = spa_sys::spa_buffer {
            n_metas: 1,
            n_datas: 0,
            metas: &raw mut meta,
            datas: std::ptr::null_mut(),
        };

        let result = unsafe { extract_cursor(&raw const buffer) };
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(!info.visible);
        assert_eq!(info.x, 100);
        assert_eq!(info.y, 200);
        assert!(info.bitmap.is_none());
    }

    #[test]
    fn test_extract_cursor_position_only() {
        // Cursor present (id != 0), but no bitmap (bitmap_offset == 0).
        let cursor_meta = spa_sys::spa_meta_cursor {
            id: 1,
            flags: 0,
            position: spa_sys::spa_point { x: 50, y: 75 },
            hotspot: spa_sys::spa_point { x: 0, y: 0 },
            bitmap_offset: 0,
        };

        let mut meta = spa_sys::spa_meta {
            type_: spa_sys::SPA_META_Cursor,
            #[allow(clippy::cast_possible_truncation)]
            size: std::mem::size_of::<spa_sys::spa_meta_cursor>() as u32,
            data: std::ptr::addr_of!(cursor_meta).cast_mut().cast::<std::os::raw::c_void>(),
        };

        let buffer = spa_sys::spa_buffer {
            n_metas: 1,
            n_datas: 0,
            metas: &raw mut meta,
            datas: std::ptr::null_mut(),
        };

        let result = unsafe { extract_cursor(&raw const buffer) };
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(info.visible);
        assert_eq!(info.x, 50);
        assert_eq!(info.y, 75);
        assert!(info.bitmap.is_none());
    }
}
