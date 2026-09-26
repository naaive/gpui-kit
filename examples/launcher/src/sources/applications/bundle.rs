//! macOS applications: `.app` bundles.
//!
//! Reading a bundle needs no macOS API — `Info.plist`, `InfoPlist.strings`
//! and `.icns` are files with documented formats — so this module compiles
//! and is tested on every platform with fake bundles.

use std::{
    collections::HashSet,
    fs::File,
    hash::{DefaultHasher, Hash as _, Hasher as _},
    io::{BufReader, BufWriter, Cursor},
    path::{Path, PathBuf},
};

use icns::{IconFamily, IconType};
use plist::{Dictionary, Value};

use super::{Application, Launch};

/// Folders of applications, and the few applications that live elsewhere.
/// Plain folders inside them (such as `Utilities`) are searched one level
/// deep.
#[cfg(target_os = "macos")]
pub fn default_directories() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
        "/System/Library/CoreServices/Finder.app",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join("Applications"));
    }
    directories
}

/// The user's languages in order of preference, as BCP 47 tags such as
/// `zh-Hans-CN`.
#[cfg(target_os = "macos")]
pub fn preferred_languages() -> Vec<String> {
    sys_locale::get_locales().collect()
}

/// Reads every application under `directories`. A bundle identifier seen
/// earlier shadows the same identifier later, so the copy in `/Applications`
/// wins over a stray one in `~/Applications`.
pub fn scan(
    directories: &[PathBuf],
    languages: &[String],
    icon_cache: Option<&Path>,
) -> Vec<Application> {
    let mut seen_paths = HashSet::new();
    let mut seen_identifiers = HashSet::new();
    let mut applications = Vec::new();
    let mut read = |bundle: PathBuf| {
        if !seen_paths.insert(bundle.clone()) {
            return;
        }
        if let Some((identifier, application)) = read_bundle(&bundle, languages, icon_cache)
            && identifier.is_none_or(|identifier| seen_identifiers.insert(identifier))
        {
            applications.push(application);
        }
    };
    for directory in directories {
        if is_bundle(directory) {
            read(directory.clone());
            continue;
        }
        for path in sorted_entries(directory) {
            if is_bundle(&path) {
                read(path);
            } else if path.is_dir() {
                sorted_entries(&path)
                    .into_iter()
                    .filter(|path| is_bundle(path))
                    .for_each(&mut read);
            }
        }
    }
    applications
}

fn is_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "app") && path.is_dir()
}

fn sorted_entries(directory: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    entries
}

/// Reads one bundle, with its identifier for de-duplication; `None` for a
/// bundle without `Info.plist` or one that only runs in the background.
///
/// The name is the one Finder shows: the localized display name when the
/// bundle declares one (`LSHasLocalizedDisplayName`), otherwise the bundle's
/// file name. `CFBundleName` is often a short internal name ("Code" for
/// "Visual Studio Code"), so it and `CFBundleDisplayName` are kept as
/// keywords instead.
fn read_bundle(
    bundle: &Path,
    languages: &[String],
    icon_cache: Option<&Path>,
) -> Option<(Option<String>, Application)> {
    let contents = bundle.join("Contents");
    let info = Value::from_file(contents.join("Info.plist")).ok()?;
    let info = info.as_dictionary()?;
    if is_true(info.get("LSBackgroundOnly")) {
        return None;
    }
    let string = |key: &str| info.get(key).and_then(Value::as_string).map(str::to_owned);
    let resources = contents.join("Resources");
    let file_name = bundle.file_stem()?.to_string_lossy().into_owned();
    let localized = is_true(info.get("LSHasLocalizedDisplayName"))
        .then(|| localized_name(&resources, languages))
        .flatten();
    let name = localized.unwrap_or_else(|| file_name.clone());

    let application = [
        string("CFBundleDisplayName"),
        string("CFBundleName"),
        Some(file_name),
    ]
    .into_iter()
    .flatten()
    .fold(
        Application::new(name, bundle.to_path_buf(), Launch::Open),
        Application::with_keyword,
    );
    let icon = string("CFBundleIconFile").and_then(|icon| {
        let icon = match Path::new(&icon).extension() {
            Some(_) => resources.join(icon),
            None => resources.join(format!("{icon}.icns")),
        };
        convert_icon(&icon, icon_cache?)
    });
    let application = match icon {
        Some(icon) => application.with_icon(icon),
        None => application,
    };
    Some((string("CFBundleIdentifier"), application))
}

fn is_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Boolean(value)) => *value,
        Some(Value::Integer(value)) => value.as_unsigned() == Some(1),
        Some(Value::String(value)) => matches!(value.as_str(), "1" | "YES" | "true"),
        _ => false,
    }
}

/// The display name for the first preferred language the bundle is localized
/// in: from `InfoPlist.loctable` (one file for every language, used since
/// macOS 14), else from `<language>.lproj/InfoPlist.strings`.
fn localized_name(resources: &Path, languages: &[String]) -> Option<String> {
    let table = Value::from_file(resources.join("InfoPlist.loctable")).ok();
    let table = table.as_ref().and_then(Value::as_dictionary);
    languages
        .iter()
        .flat_map(|language| language_candidates(language))
        .find_map(|candidate| {
            let strings = match table {
                Some(table) => table.get(&candidate)?.as_dictionary()?.clone(),
                None => {
                    let path = resources
                        .join(format!("{candidate}.lproj"))
                        .join("InfoPlist.strings");
                    parse_strings(&std::fs::read(path).ok()?)?
                }
            };
            ["CFBundleDisplayName", "CFBundleName"]
                .iter()
                .find_map(|key| strings.get(key)?.as_string().map(str::to_owned))
        })
}

/// Localization folder names to try for a BCP 47 tag, most specific first.
/// Bundles spell the same language several ways (`zh-Hans`, `zh_CN`), so
/// `zh-Hans-CN` tries all of them.
fn language_candidates(tag: &str) -> Vec<String> {
    let parts: Vec<&str> = tag.split(['-', '_']).collect();
    let language = parts[0];
    let script = parts[1..].iter().find(|part| part.len() == 4);
    let region = parts[1..].iter().find(|part| {
        part.len() == 2 || (part.len() == 3 && part.chars().all(|c| c.is_ascii_digit()))
    });
    let mut candidates = vec![tag.to_owned(), tag.replace('-', "_")];
    if let Some(script) = script {
        candidates.push(format!("{language}-{script}"));
        candidates.push(format!("{language}_{script}"));
    }
    if let Some(region) = region {
        candidates.push(format!("{language}-{region}"));
        candidates.push(format!("{language}_{region}"));
    }
    if language == "zh" {
        match script.copied() {
            Some("Hans") => candidates.push("zh_CN".into()),
            Some("Hant") => candidates.extend(["zh_TW".into(), "zh_HK".into()]),
            _ => {}
        }
    }
    candidates.push(language.to_owned());
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

/// Reads a `.strings` file: a binary property list, or the text form
/// `"key" = "value";` in UTF-8 or UTF-16 (with a byte-order mark, as Xcode
/// writes it).
fn parse_strings(bytes: &[u8]) -> Option<Dictionary> {
    if bytes.starts_with(b"bplist") {
        return Value::from_reader(Cursor::new(bytes))
            .ok()?
            .into_dictionary();
    }
    let text = match bytes {
        [0xff, 0xfe, rest @ ..] => decode_utf16(rest, u16::from_le_bytes)?,
        [0xfe, 0xff, rest @ ..] => decode_utf16(rest, u16::from_be_bytes)?,
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8(rest.to_vec()).ok()?,
        _ => String::from_utf8(bytes.to_vec()).ok()?,
    };
    parse_strings_text(&text)
}

/// The text form: `key = value;` pairs, each side quoted or a bare word, with
/// `/* */` and `//` comments. (The `plist` crate's OpenStep reader would do,
/// but it reads bytes as Latin-1, which garbles every localized name.)
fn parse_strings_text(text: &str) -> Option<Dictionary> {
    let mut strings = Dictionary::new();
    let mut chars = text.chars().peekable();
    loop {
        skip_blank(&mut chars);
        if chars.peek().is_none() {
            return Some(strings);
        }
        let key = read_token(&mut chars)?;
        skip_blank(&mut chars);
        let value = match chars.next()? {
            '=' => {
                skip_blank(&mut chars);
                let value = read_token(&mut chars)?;
                skip_blank(&mut chars);
                (chars.next()? == ';').then_some(value)?
            }
            // `"key";` alone means the value is the key.
            ';' => key.clone(),
            _ => return None,
        };
        strings.insert(key, Value::String(value));
    }
}

fn skip_blank(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut lookahead = chars.clone();
        match (lookahead.next(), lookahead.next()) {
            (Some('/'), Some('*')) => {
                chars.nth(1);
                let mut previous = None;
                for character in chars.by_ref() {
                    if previous == Some('*') && character == '/' {
                        break;
                    }
                    previous = Some(character);
                }
            }
            (Some('/'), Some('/')) => while chars.next_if(|c| *c != '\n').is_some() {},
            _ => return,
        }
    }
}

/// A quoted string with its escapes (`\"`, `\\`, `\n`, `\t`, `\Uxxxx`) or a
/// bare word.
fn read_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut token = String::new();
    if chars.next_if_eq(&'"').is_none() {
        while let Some(character) =
            chars.next_if(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '$' | ':'))
        {
            token.push(character);
        }
        return (!token.is_empty()).then_some(token);
    }
    loop {
        match chars.next()? {
            '"' => return Some(token),
            '\\' => match chars.next()? {
                'n' => token.push('\n'),
                't' => token.push('\t'),
                'U' | 'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                    token.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => token.push(other),
            },
            other => token.push(other),
        }
    }
}

fn decode_utf16(bytes: &[u8], decode: fn([u8; 2]) -> u16) -> Option<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| decode([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

/// Icons are drawn at about 20 points, so 64 px covers a 2× display; a larger
/// size is next best, then the largest smaller one.
const ICON_PIXELS: u32 = 64;

/// Converts an `.icns` file to a PNG in `cache`, reusing an earlier
/// conversion of the same file. The cache key includes the file's size and
/// modification time, so an updated application gets its new icon.
fn convert_icon(icns: &Path, cache: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(icns).ok()?;
    let mut hasher = DefaultHasher::new();
    icns.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    let png = cache.join(format!("{:016x}.png", hasher.finish()));
    if png.is_file() {
        return Some(png);
    }

    let family = IconFamily::read(BufReader::new(File::open(icns).ok()?)).ok()?;
    let mut types: Vec<IconType> = family
        .available_icons()
        .into_iter()
        .filter(|icon_type| !icon_type.is_mask())
        .collect();
    types.sort_by_key(|icon_type| {
        let width = icon_type.pixel_width();
        match width >= ICON_PIXELS {
            true => width,
            false => u32::MAX - width,
        }
    });
    // Some sizes are JPEG 2000, which cannot be decoded; the next one is tried.
    let image = types
        .into_iter()
        .find_map(|icon_type| family.get_icon_with_type(icon_type).ok())?;
    std::fs::create_dir_all(cache).ok()?;
    let temporary = png.with_extension("png.tmp");
    image
        .write_png(BufWriter::new(File::create(&temporary).ok()?))
        .ok()?;
    std::fs::rename(&temporary, &png).ok()?;
    Some(png)
}

#[cfg(test)]
mod tests {
    use icns::{Image, PixelFormat};

    use super::*;

    fn info_plist(entries: &[(&str, &str)]) -> String {
        let body: String = entries
            .iter()
            .map(|(key, value)| match *value {
                "<true/>" => format!("<key>{key}</key><true/>"),
                _ => format!("<key>{key}</key><string>{value}</string>"),
            })
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>{body}</dict></plist>"#
        )
    }

    fn bundle(root: &Path, relative: &str, entries: &[(&str, &str)]) -> PathBuf {
        let bundle = root.join(relative);
        std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), info_plist(entries)).unwrap();
        bundle
    }

    fn utf16_le(text: &str) -> Vec<u8> {
        [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect()
    }

    #[test]
    fn test_reads_localized_names_icons_and_identifiers() {
        let root = tempfile::tempdir().unwrap();
        let applications = root.path().join("Applications");
        let cache = root.path().join("cache");

        let wechat = bundle(
            &applications,
            "WeChat.app",
            &[
                ("CFBundleIdentifier", "com.tencent.xinWeChat"),
                ("CFBundleName", "WeChat"),
                ("CFBundleIconFile", "AppIcon"),
                ("LSHasLocalizedDisplayName", "<true/>"),
            ],
        );
        let lproj = wechat.join("Contents/Resources/zh_CN.lproj");
        std::fs::create_dir_all(&lproj).unwrap();
        std::fs::write(
            lproj.join("InfoPlist.strings"),
            utf16_le("/* Localized */\n\"CFBundleDisplayName\" = \"微信\";\n\"NSHumanReadableCopyright\" = \"©\";\n"),
        )
        .unwrap();
        let mut family = IconFamily::new();
        family
            .add_icon(&Image::new(PixelFormat::RGBA, 128, 128))
            .unwrap();
        family
            .write(File::create(wechat.join("Contents/Resources/AppIcon.icns")).unwrap())
            .unwrap();

        bundle(
            &applications,
            "Visual Studio Code.app",
            &[
                ("CFBundleIdentifier", "com.microsoft.VSCode"),
                ("CFBundleName", "Code"),
            ],
        );
        // One level down, as in `/Applications/Utilities`.
        bundle(
            &applications,
            "Utilities/Terminal.app",
            &[("CFBundleIdentifier", "com.apple.Terminal")],
        );
        bundle(
            &applications,
            "Agent.app",
            &[
                ("CFBundleIdentifier", "com.example.agent"),
                ("LSBackgroundOnly", "<true/>"),
            ],
        );
        // The same identifier in a later directory is shadowed.
        let user = root.path().join("home/Applications");
        bundle(
            &user,
            "Code Copy.app",
            &[("CFBundleIdentifier", "com.microsoft.VSCode")],
        );

        let found = scan(
            &[applications.clone(), applications.join("Utilities"), user],
            &["zh-Hans-CN".to_owned(), "en".to_owned()],
            Some(&cache),
        );
        let names: Vec<&str> = found.iter().map(|app| app.name.as_ref()).collect();
        assert_eq!(names, ["Terminal", "Visual Studio Code", "微信"]);
        assert_eq!(found[1].keywords, ["Code"]);
        assert_eq!(found[2].keywords, ["WeChat"]);
        assert_eq!(found[2].launch, Launch::Open);

        let icon = found[2].icon.as_deref().expect("the icns is converted");
        assert!(icon.starts_with(&cache));
        assert!(std::fs::read(icon).unwrap().starts_with(b"\x89PNG"));
        assert!(found[1].icon.is_none());

        // Without the language the file name is used.
        let english = scan(&[applications], &["en-US".to_owned()], None);
        assert!(english.iter().any(|app| app.name.as_ref() == "WeChat"));
    }

    #[test]
    fn test_prefers_the_loctable() {
        let root = tempfile::tempdir().unwrap();
        let settings = bundle(
            root.path(),
            "System Settings.app",
            &[("LSHasLocalizedDisplayName", "<true/>")],
        );
        let mut table = Dictionary::new();
        let mut chinese = Dictionary::new();
        chinese.insert("CFBundleDisplayName".into(), "系统设置".into());
        table.insert("zh_CN".into(), Value::Dictionary(chinese));
        Value::Dictionary(table)
            .to_file_binary(settings.join("Contents/Resources/InfoPlist.loctable"))
            .unwrap();

        let found = scan(&[settings], &["zh-Hans".to_owned()], None);
        assert_eq!(found[0].name.as_ref(), "系统设置");
    }

    #[test]
    fn test_language_candidates() {
        assert_eq!(
            language_candidates("zh-Hans-CN"),
            [
                "zh-Hans-CN",
                "zh_Hans_CN",
                "zh-Hans",
                "zh_Hans",
                "zh-CN",
                "zh_CN",
                "zh"
            ]
        );
        assert_eq!(language_candidates("fr"), ["fr"]);
        assert_eq!(
            language_candidates("zh-Hant"),
            ["zh-Hant", "zh_Hant", "zh_TW", "zh_HK", "zh"]
        );
    }

    #[test]
    fn test_parses_strings_files() {
        let strings = parse_strings(
            "/* InfoPlist */\n\"CFBundleName\" = \"Notes\";\n// comment\n\
             CFBundleDisplayName = \"备忘录 \\\"\\U2014\\\"\";\n\"Same\";\n"
                .as_bytes(),
        )
        .unwrap();
        let get = |key: &str| strings.get(key).and_then(Value::as_string);
        assert_eq!(get("CFBundleName"), Some("Notes"));
        assert_eq!(get("CFBundleDisplayName"), Some("备忘录 \"—\""));
        assert_eq!(get("Same"), Some("Same"));
        assert!(parse_strings(b"\"unterminated = ").is_none());
        assert!(parse_strings(b"\"a\" = \"b\"").is_none(), "a missing `;`");
    }
}
