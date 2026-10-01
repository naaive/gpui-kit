//! HDR desktops: frames in 16-bit floating-point scRGB, mapped to the
//! 8-bit sRGB a screenshot is pasted and saved as.
//!
//! scRGB is linear light with sRGB primaries, where 1.0 is 80 nits. Windows
//! shows SDR content at the "SDR content brightness" the user picked, so
//! dividing by that level puts SDR white back at 1.0: windows look in the
//! screenshot as they look on screen, and only HDR highlights are clipped.

use windows::Win32::{
    Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
        QueryDisplayConfig,
    },
    Foundation::ERROR_SUCCESS,
};

/// The scRGB value of SDR white on the output named `gdi_name` (such as
/// `\\.\DISPLAY1`), or 1.0 when the system doesn't say.
pub fn sdr_white_level(gdi_name: &[u16]) -> f32 {
    let name_length = gdi_name
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(gdi_name.len());
    let gdi_name = &gdi_name[..name_length];
    let (mut path_count, mut mode_count) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return 1.;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return 1.;
        }
        for path in &paths[..path_count as usize] {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let source_length = source
                .viewGdiDeviceName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(source.viewGdiDeviceName.len());
            if &source.viewGdiDeviceName[..source_length] != gdi_name {
                continue;
            }
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel > 0 {
                // Thousandths of 80 nits, which is scRGB 1.0.
                return white.SDRWhiteLevel as f32 / 1000.;
            }
        }
    }
    1.
}

/// Converts scRGB half-float RGBA rows into tight, opaque 8-bit sRGB RGBA.
pub fn scrgb_to_srgb(
    source: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    white_level: f32,
) -> Vec<u8> {
    // Every half-float bit pattern maps to one 8-bit value, so a table of
    // 65536 entries replaces millions of `powf` calls on a 4K frame.
    let table: Vec<u8> = (0..=u16::MAX)
        .map(|bits| encode_srgb(half_to_f32(bits) / white_level))
        .collect();
    crate::capture::convert_rows(source, height, stride, width * 4, |row, out| {
        for (pixel, out) in row[..width * 8]
            .chunks_exact(8)
            .zip(out.chunks_exact_mut(4))
        {
            let channel =
                |ix: usize| table[u16::from_le_bytes([pixel[ix * 2], pixel[ix * 2 + 1]]) as usize];
            out.copy_from_slice(&[channel(0), channel(1), channel(2), 255]);
        }
    })
}

/// Linear light to an 8-bit sRGB value, clipping what is out of range.
fn encode_srgb(linear: f32) -> u8 {
    let linear = if linear.is_nan() {
        0.
    } else {
        linear.clamp(0., 1.)
    };
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    };
    (encoded * 255.).round() as u8
}

/// IEEE 754 half precision to single precision.
fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1. } else { 1. };
    let exponent = (bits >> 10) & 0x1F;
    let mantissa = (bits & 0x3FF) as f32;
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        0x1F => {
            if mantissa == 0. {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1. + mantissa / 1024.) * 2f32.powi(exponent as i32 - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_half_to_f32() {
        assert_eq!(half_to_f32(0x3C00), 1.);
        assert_eq!(half_to_f32(0x4000), 2.);
        assert_eq!(half_to_f32(0xBC00), -1.);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0), 0.);
    }

    #[test]
    fn test_scrgb_to_srgb_maps_sdr_white_to_white() {
        // One pixel at scRGB 2.5 (SDR white at 200 nits), one at 0.
        let white = 0x4100u16.to_le_bytes(); // 2.5
        let black = 0u16.to_le_bytes();
        let mut row = Vec::new();
        for _ in 0..3 {
            row.extend_from_slice(&white);
        }
        row.extend_from_slice(&0x3C00u16.to_le_bytes());
        for _ in 0..4 {
            row.extend_from_slice(&black);
        }
        let pixels = scrgb_to_srgb(&row, 2, 1, row.len(), 2.5);
        assert_eq!(&pixels[..4], &[255, 255, 255, 255]);
        assert_eq!(&pixels[4..], &[0, 0, 0, 255]);
    }

    #[test]
    fn test_encode_srgb_mid_grey() {
        assert_eq!(encode_srgb(0.2158), 128);
        assert_eq!(encode_srgb(5.), 255, "highlights clip");
        assert_eq!(encode_srgb(-1.), 0);
    }
}
