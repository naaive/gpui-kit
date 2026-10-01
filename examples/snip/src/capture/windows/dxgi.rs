//! DXGI Desktop Duplication: one frame from every output.
//!
//! A duplication is created per capture rather than kept open: an open
//! duplication only hands out a frame when the screen changes, and a still
//! screen is exactly what a screenshot is usually taken of. The first frame
//! of a new duplication is always the whole desktop image.

use anyhow::{Context as _, Result, bail};
use windows::{
    Win32::Graphics::{
        Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
        Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
            D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
        },
        Dxgi::{
            Common::{
                DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION,
                DXGI_MODE_ROTATION_ROTATE90, DXGI_MODE_ROTATION_ROTATE180,
                DXGI_MODE_ROTATION_ROTATE270, DXGI_SAMPLE_DESC,
            },
            CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1,
            IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication,
            IDXGIResource,
        },
    },
    core::Interface as _,
};

use super::super::{Frame, bgra_to_rgba};
use crate::geometry::DisplayArea;

/// How long to wait for an output's first frame. It is normally ready at
/// once; an output that takes longer is captured with GDI instead.
const FRAME_TIMEOUT_MS: u32 = 250;

pub fn capture() -> Result<Vec<Frame>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.context("no DXGI factory")?;
    let mut frames = Vec::new();
    for adapter_ix in 0.. {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_ix) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error.into()),
        };
        if let Err(error) = capture_adapter(&adapter, &mut frames) {
            tracing::debug!("skipping a graphics adapter: {error:#}");
        }
    }
    Ok(frames)
}

fn capture_adapter(adapter: &IDXGIAdapter1, frames: &mut Vec<Frame>) -> Result<()> {
    let mut outputs = Vec::new();
    for output_ix in 0.. {
        let output = match unsafe { adapter.EnumOutputs(output_ix) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error.into()),
        };
        let description = unsafe { output.GetDesc() }?;
        if description.AttachedToDesktop.as_bool() {
            outputs.push((output, description));
        }
    }
    // A Direct3D device takes tens of milliseconds to create, and virtual
    // display adapters and the software renderer often show no display.
    if outputs.is_empty() {
        return Ok(());
    }
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .context("cannot create a Direct3D device")?;
    let (Some(device), Some(context)) = (device, context) else {
        bail!("Direct3D returned no device");
    };
    for (output, description) in outputs {
        let native_id = description.Monitor.0 as u64;
        let result = duplicate(&output, &device)
            .and_then(|duplication| {
                copy_frame(&device, &context, &duplication, &description.DeviceName)
            })
            .and_then(|(pixels, width, height)| {
                let (pixels, width, height) = unrotate(pixels, width, height, description.Rotation);
                let bounds = super::rect(description.DesktopCoordinates);
                if (bounds.width, bounds.height) != (width as i32, height as i32) {
                    bail!(
                        "the output reports {}×{} but its image is {width}×{height}",
                        bounds.width,
                        bounds.height
                    );
                }
                // The scale is filled in from the monitor list.
                Frame::new(DisplayArea::new(bounds, 1.), native_id, pixels)
            });
        match result {
            Ok(frame) => frames.push(frame),
            Err(error) => tracing::debug!("cannot duplicate an output: {error:#}"),
        }
    }
    Ok(())
}

/// Duplicates an output.
///
/// An HDR or advanced-color desktop is composed in 16-bit floating point,
/// and `IDXGIOutput5` hands its frames out that way; they are mapped to SDR
/// in [`super::hdr`]. Before Windows 10 1703 only `IDXGIOutput1` exists,
/// whose duplication is 8-bit.
fn duplicate(output: &IDXGIOutput, device: &ID3D11Device) -> Result<IDXGIOutputDuplication> {
    if let Ok(output) = output.cast::<IDXGIOutput5>() {
        let formats = [DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT];
        return unsafe { output.DuplicateOutput1(device, 0, &formats) }
            .context("the output can't be duplicated");
    }
    let output: IDXGIOutput1 = output.cast()?;
    unsafe { output.DuplicateOutput(device) }.context("the output can't be duplicated")
}

/// The first frame of `duplication` that holds an image.
///
/// A new duplication's first frame can arrive before anything was presented
/// to it, all black, with a present time of zero; such frames are released
/// and the next one awaited. The frame stays acquired for the caller, who
/// must release it.
fn acquire_image(duplication: &IDXGIOutputDuplication) -> Result<IDXGIResource> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(FRAME_TIMEOUT_MS as u64);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        unsafe {
            duplication.AcquireNextFrame(remaining.as_millis() as u32, &mut info, &mut resource)
        }
        .context("no frame from the output")?;
        if info.LastPresentTime != 0
            && let Some(resource) = resource
        {
            return Ok(resource);
        }
        unsafe { duplication.ReleaseFrame() }.ok();
        if remaining.is_zero() {
            bail!("the output presented no image in time");
        }
    }
}

/// The first frame of `duplication`, as tight RGBA in the output's native
/// (unrotated) orientation, with its width and height.
fn copy_frame(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    duplication: &IDXGIOutputDuplication,
    gdi_name: &[u16],
) -> Result<(Vec<u8>, usize, usize)> {
    let resource = acquire_image(duplication)?;
    let result = (|| {
        let texture: ID3D11Texture2D = resource.cast()?;
        let mut description = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut description) };
        let format = description.Format;
        tracing::debug!(
            "duplicated a {}×{} frame in {format:?}",
            description.Width,
            description.Height
        );
        if format != DXGI_FORMAT_B8G8R8A8_UNORM && format != DXGI_FORMAT_R16G16B16A16_FLOAT {
            bail!("unexpected frame format {format:?}");
        }
        let staging_description = D3D11_TEXTURE2D_DESC {
            Width: description.Width,
            Height: description.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: description.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe { device.CreateTexture2D(&staging_description, None, Some(&mut staging)) }?;
        let staging = staging.context("no staging texture")?;
        unsafe { context.CopyResource(&staging, &texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }?;
        let (width, height) = (description.Width as usize, description.Height as usize);
        let stride = mapped.RowPitch as usize;
        let source =
            unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, stride * height) };
        let pixels = if format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            super::hdr::scrgb_to_srgb(
                source,
                width,
                height,
                stride,
                super::hdr::sdr_white_level(gdi_name),
            )
        } else {
            bgra_to_rgba(source, width, height, stride)
        };
        unsafe { context.Unmap(&staging, 0) };
        Ok((pixels, width, height))
    })();
    unsafe { duplication.ReleaseFrame() }.ok();
    result
}

/// Turns a rotated output's scan-out image back into desktop orientation.
///
/// `ROTATE90` means the desktop was turned a quarter clockwise onto the
/// panel, so the image is turned a quarter counter-clockwise to undo it.
fn unrotate(
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    rotation: DXGI_MODE_ROTATION,
) -> (Vec<u8>, usize, usize) {
    let source = |x: usize, y: usize| &pixels[(y * width + x) * 4..(y * width + x) * 4 + 4];
    let turn =
        |out_width: usize, out_height: usize, from: &dyn Fn(usize, usize) -> (usize, usize)| {
            let mut out = Vec::with_capacity(pixels.len());
            for y in 0..out_height {
                for x in 0..out_width {
                    let (sx, sy) = from(x, y);
                    out.extend_from_slice(source(sx, sy));
                }
            }
            out
        };
    match rotation {
        DXGI_MODE_ROTATION_ROTATE90 => (
            turn(height, width, &|x, y| (width - 1 - y, x)),
            height,
            width,
        ),
        DXGI_MODE_ROTATION_ROTATE180 => (
            turn(width, height, &|x, y| (width - 1 - x, height - 1 - y)),
            width,
            height,
        ),
        DXGI_MODE_ROTATION_ROTATE270 => (
            turn(height, width, &|x, y| (y, height - 1 - x)),
            height,
            width,
        ),
        _ => (pixels, width, height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×1 image: red, green.
    fn pixels() -> Vec<u8> {
        vec![255, 0, 0, 255, 0, 255, 0, 255]
    }

    #[test]
    fn test_unrotate_quarter_turns() {
        let (out, width, height) = unrotate(pixels(), 2, 1, DXGI_MODE_ROTATION_ROTATE90);
        assert_eq!((width, height), (1, 2));
        assert_eq!(
            &out[..4],
            &[0, 255, 0, 255],
            "counter-clockwise puts green on top"
        );

        let (out, width, height) = unrotate(pixels(), 2, 1, DXGI_MODE_ROTATION_ROTATE270);
        assert_eq!((width, height), (1, 2));
        assert_eq!(&out[..4], &[255, 0, 0, 255]);

        let (out, ..) = unrotate(pixels(), 2, 1, DXGI_MODE_ROTATION_ROTATE180);
        assert_eq!(&out[..4], &[0, 255, 0, 255]);
    }
}
