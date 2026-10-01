//! Windows applications: the Start Menu's shortcuts.
//!
//! The Start Menu is a folder of `.lnk` files, per user and for all users.
//! Opening a shortcut with the shell starts its target with the arguments and
//! working directory the installer chose, so the shortcut itself is launched
//! and never parsed. Its icon is the one the shell draws for it, saved as a
//! PNG in a cache.

use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash as _, Hasher as _},
    path::{Path, PathBuf},
};

use super::{Application, Launch};

/// The user's Start Menu programs, then the one shared by all users.
#[cfg(target_os = "windows")]
pub fn default_directories() -> Vec<PathBuf> {
    ["APPDATA", "ProgramData"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| {
            PathBuf::from(base)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
        })
        .collect()
}

/// Every shortcut under `directories`, named after its file. A name seen in
/// an earlier directory shadows the same name later: installers put the same
/// shortcut in both menus. Uninstallers are left out; they are reached from
/// Settings, not searched for.
///
/// Icons are saved in `icon_cache`; without one, or where the shell has no
/// icon, the application keeps the generic one.
pub fn scan(directories: &[PathBuf], icon_cache: Option<&Path>) -> Vec<Application> {
    #[cfg(target_os = "windows")]
    let _com = icon_cache.map(|_| shell_icon::Com::initialize());
    let mut seen = HashSet::new();
    let mut shortcuts = Vec::new();
    for directory in directories {
        let mut found = Vec::new();
        collect_shortcuts(directory, &mut found);
        found.sort();
        for path in found {
            let Some(name) = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
            else {
                continue;
            };
            if name.to_lowercase().contains("uninstall") || !seen.insert(name.to_lowercase()) {
                continue;
            }
            let folder = path
                .parent()
                .filter(|parent| *parent != directory.as_path())
                .and_then(Path::file_name)
                .map(|folder| folder.to_string_lossy().into_owned());
            let icon = icon_cache.and_then(|cache| cached_icon(&path, cache));
            let application = Application::new(name, path, Launch::Open);
            let application = match folder {
                Some(folder) => application.with_subtitle(folder),
                None => application,
            };
            shortcuts.push(match icon {
                Some(icon) => application.with_icon(icon),
                None => application,
            });
        }
    }
    shortcuts
}

fn collect_shortcuts(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.is_dir() {
            collect_shortcuts(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            found.push(path);
        }
    }
}

/// Icons are drawn at about 20 points, so 64 px covers a 2× display.
const ICON_PIXELS: u32 = 64;
/// The side of a file preview\'s thumbnail.
const THUMBNAIL_PIXELS: u32 = 480;

/// The shortcut's icon as a PNG in `cache`, reusing an earlier extraction of
/// the same shortcut. The cache key includes the shortcut's size and
/// modification time, so a reinstalled application gets its new icon.
pub(super) fn cached_icon(shortcut: &Path, cache: &Path) -> Option<PathBuf> {
    cached_picture(shortcut, cache, false)
}

/// The shell's thumbnail of the file's content (a PDF's first page, a
/// video's frame), as a PNG in `cache`; `None` for a file the shell has no
/// thumbnail of.
pub(super) fn cached_thumbnail(file: &Path, cache: &Path) -> Option<PathBuf> {
    cached_picture(file, cache, true)
}

fn cached_picture(shortcut: &Path, cache: &Path, thumbnail: bool) -> Option<PathBuf> {
    let metadata = std::fs::metadata(shortcut).ok()?;
    let mut hasher = DefaultHasher::new();
    shortcut.hash(&mut hasher);
    thumbnail.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    let png = cache.join(format!("{:016x}.png", hasher.finish()));
    if png.is_file() {
        return Some(png);
    }

    #[cfg(target_os = "windows")]
    {
        let image = match thumbnail {
            true => shell_icon::extract_thumbnail(shortcut, THUMBNAIL_PIXELS)?,
            false => shell_icon::extract(shortcut, ICON_PIXELS)?,
        };
        std::fs::create_dir_all(cache).ok()?;
        let temporary = png.with_extension("png.tmp");
        image
            .write_png(std::io::BufWriter::new(
                std::fs::File::create(&temporary).ok()?,
            ))
            .ok()?;
        std::fs::rename(&temporary, &png).ok()?;
        Some(png)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = thumbnail;
        None
    }
}

/// Runs `work` with COM initialized on this thread, as icon extraction needs.
#[cfg(target_os = "windows")]
pub(super) fn with_com<T>(work: impl FnOnce() -> T) -> T {
    let _com = shell_icon::Com::initialize();
    work()
}

/// The shell's own icon drawing, through `IShellItemImageFactory`: the same
/// icon Explorer and the Start Menu show, overlays left off.
#[cfg(target_os = "windows")]
mod shell_icon {
    use std::{ffi::c_void, path::Path};

    use icns::{Image, PixelFormat};
    use windows::{
        Win32::{
            Foundation::SIZE,
            Graphics::Gdi::{
                BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
                GetDIBits, GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
            },
            System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            UI::Shell::{
                IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_ICONONLY,
                SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY,
            },
        },
        core::HSTRING,
    };

    /// COM, initialized on this thread for as long as it is held.
    pub struct Com {
        initialized: bool,
    }

    impl Com {
        pub fn initialize() -> Self {
            // A thread already in the multithreaded apartment refuses, and the
            // shell works there too; only a successful call is balanced.
            let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            Self { initialized }
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.initialized {
                unsafe { CoUninitialize() };
            }
        }
    }

    /// The icon the shell draws for `path`, at most `pixels` square.
    pub fn extract(path: &Path, pixels: u32) -> Option<Image> {
        draw(path, pixels, SIIGBF_ICONONLY)
    }

    /// The thumbnail of `path`'s content, fit within `pixels` square.
    pub fn extract_thumbnail(path: &Path, pixels: u32) -> Option<Image> {
        draw(path, pixels, SIIGBF_THUMBNAILONLY | SIIGBF_RESIZETOFIT)
    }

    fn draw(path: &Path, pixels: u32, flags: SIIGBF) -> Option<Image> {
        let factory: IShellItemImageFactory =
            unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None) }.ok()?;
        let side = pixels as i32;
        let bitmap = unsafe { factory.GetImage(SIZE { cx: side, cy: side }, flags) }.ok()?;
        let image = read_bitmap(bitmap);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        image
    }

    /// The bitmap's pixels as straight RGBA, top row first.
    fn read_bitmap(bitmap: HBITMAP) -> Option<Image> {
        let mut info = BITMAP::default();
        let read = unsafe {
            GetObjectW(
                HGDIOBJ(bitmap.0),
                size_of::<BITMAP>() as i32,
                Some(&mut info as *mut BITMAP as *mut c_void),
            )
        };
        if read == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
            return None;
        }
        let (width, height) = (info.bmWidth, info.bmHeight.abs());
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top to bottom.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let lines = unsafe {
            let dc = GetDC(None);
            let lines = GetDIBits(
                dc,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr() as *mut c_void),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            lines
        };
        if lines != height {
            return None;
        }

        // The shell hands back premultiplied BGRA; a bitmap with no alpha at
        // all is opaque.
        let has_alpha = pixels.chunks_exact(4).any(|pixel| pixel[3] != 0);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            match (has_alpha, pixel[3]) {
                (false, _) => pixel[3] = 255,
                (true, 0 | 255) => {}
                (true, alpha) => {
                    for channel in &mut pixel[..3] {
                        *channel = (*channel as u32 * 255 / alpha as u32).min(255) as u8;
                    }
                }
            }
        }
        Image::from_data(PixelFormat::RGBA, width as u32, height as u32, pixels).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reads_shortcuts_and_skips_uninstallers_and_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let common = root.path().join("common");
        for path in [
            user.join("Notepad++.lnk"),
            user.join("Git/Git Bash.lnk"),
            user.join("Git/Uninstall Git.lnk"),
            common.join("notepad++.LNK"),
            common.join("Paint.lnk"),
            common.join("readme.txt"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
        }

        let applications = scan(&[user, common], None);
        let names: Vec<(&str, Option<&str>)> = applications
            .iter()
            .map(|app| (app.name.as_ref(), app.subtitle.as_ref().map(|s| s.as_ref())))
            .collect();
        assert_eq!(
            names,
            [
                ("Git Bash", Some("Git")),
                ("Notepad++", None),
                ("Paint", None)
            ]
        );
        assert_eq!(applications[2].launch, Launch::Open);
    }
}
