//! Text recognition with Vision, which detects the language on macOS 13
//! and later and reads English only before that.

use anyhow::{Context as _, Result, anyhow};
use image::RgbaImage;
use objc2::{AnyThread as _, rc::Retained, runtime::NSObjectProtocol as _, sel};
use objc2_core_foundation::CFData;
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    kCGColorSpaceSRGB,
};
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

pub fn recognize(image: &RgbaImage) -> Result<String> {
    let image = cg_image(image).context("cannot hand the image to Vision")?;
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    if request.respondsToSelector(sel!(setAutomaticallyDetectsLanguage:)) {
        request.setAutomaticallyDetectsLanguage(true);
    }
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &image,
            &NSDictionary::new(),
        )
    };
    // VNRecognizeTextRequest → VNImageBasedRequest → VNRequest.
    let requests: Retained<NSArray<VNRequest>> =
        NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(
            request.clone(),
        ))]);
    handler
        .performRequests_error(&requests)
        .map_err(|error| anyhow!("text recognition failed: {error}"))?;
    let lines = request
        .results()
        .map(|observations| {
            observations
                .iter()
                .filter_map(|observation| {
                    let candidates = observation.topCandidates(1);
                    candidates
                        .firstObject()
                        .map(|text| text.string().to_string())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(lines.join("\n"))
}

/// An sRGB `CGImage` of the opaque RGBA pixels of `image`.
fn cg_image(image: &RgbaImage) -> Option<objc2_core_foundation::CFRetained<CGImage>> {
    let data = CFData::from_bytes(image.as_raw());
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let width = image.width() as usize;
    unsafe {
        CGImage::new(
            width,
            image.height() as usize,
            8,
            32,
            width * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}
