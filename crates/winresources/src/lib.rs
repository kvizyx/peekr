//! Embeds an icon and version information into Windows executables from a build script.
//!
//! The resources are written straight into a compiled resource file (`.res`), which the MSVC
//! linker takes as an input like an object file, so neither `rc.exe` nor the Windows SDK is
//! needed.
//!
//! ```no_run
//! winresources::Resources::new()
//!     .icon(std::fs::read("icon.ico").unwrap())
//!     .string("ProductName", "Example")
//!     .link()
//!     .unwrap();
//! ```

use std::io::{Error, ErrorKind, Result};
use std::path::PathBuf;
use std::{env, fs};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;

/// Moveable, pure and discardable, as `rc.exe` marks icons and version information.
const MEMORY_FLAGS: u16 = 0x1030;
/// English (United States), matching the string table below.
const LANGUAGE: u16 = 0x0409;
/// The string table's language and code page (UTF-16), as `rc.exe` names it.
const STRING_TABLE: &str = "040904b0";
const CODE_PAGE: u16 = 1200;

/// Resources to embed into the executables of the package being built.
#[derive(Debug, Clone, Default)]
pub struct Resources {
    icon: Option<Vec<u8>>,
    version: Option<[u16; 4]>,
    strings: Vec<(String, String)>,
}

impl Resources {
    /// Resources with the version taken from the package being built, and the strings Windows
    /// shows in the file properties filled in from its manifest.
    #[must_use]
    pub fn new() -> Self {
        let var = |name: &str| env::var(name).unwrap_or_default();
        let number = |name: &str| var(name).parse().unwrap_or(0);

        let name = var("CARGO_PKG_NAME");
        let version = var("CARGO_PKG_VERSION");

        let mut resources = Self {
            version: Some([
                number("CARGO_PKG_VERSION_MAJOR"),
                number("CARGO_PKG_VERSION_MINOR"),
                number("CARGO_PKG_VERSION_PATCH"),
                0,
            ]),
            ..Self::default()
        };

        for (key, value) in [
            ("FileVersion", version.clone()),
            ("ProductVersion", version),
            ("InternalName", name.clone()),
            ("OriginalFilename", format!("{name}.exe")),
            ("Comments", var("CARGO_PKG_DESCRIPTION")),
        ] {
            if !value.is_empty() {
                resources = resources.string(key, value);
            }
        }

        resources
    }

    /// Sets the icon from the contents of an `.ico` file. Windows shows it for the executable and
    /// its shortcuts.
    #[must_use]
    pub fn icon(mut self, ico: Vec<u8>) -> Self {
        self.icon = Some(ico);
        self
    }

    /// Sets the numeric file and product version.
    #[must_use]
    pub fn version(mut self, version: [u16; 4]) -> Self {
        self.version = Some(version);
        self
    }

    /// Sets a version information string, such as `FileDescription`, `ProductName` or
    /// `LegalCopyright`, replacing an earlier value.
    #[must_use]
    pub fn string(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();

        self.strings.retain(|(existing, _)| *existing != key);
        self.strings.push((key, value.into()));
        self
    }

    /// Writes the resources to `OUT_DIR` and has Cargo link them into the package's binaries.
    /// Does nothing unless the target is Windows with the MSVC linker.
    ///
    /// # Errors
    ///
    /// Fails when the icon is not a valid `.ico` file or the resource file cannot be written.
    #[expect(clippy::print_stdout, reason = "Cargo reads build script instructions from stdout")]
    pub fn link(&self) -> Result<()> {
        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

        if target_os != "windows" {
            return Ok(());
        }

        if target_env != "msvc" {
            println!("cargo::warning=winresources: resources are only embedded with the MSVC linker");
            return Ok(());
        }

        let out_dir = env::var_os("OUT_DIR").ok_or_else(|| invalid("OUT_DIR is not set"))?;
        let path = PathBuf::from(out_dir).join("resources.res");
        fs::write(&path, self.compile()?)?;

        println!("cargo::rustc-link-arg-bins={}", path.display());

        Ok(())
    }

    /// Compiles the resources into the contents of a `.res` file.
    ///
    /// # Errors
    ///
    /// Fails when the icon is not a valid `.ico` file.
    pub fn compile(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();

        // A resource file starts with an empty entry that marks it as 32-bit.
        entry(&mut out, 0, 0, 0, 0, &[]);

        if let Some(ico) = &self.icon {
            let images = parse_ico(ico)?;

            for (id, image) in (1..).zip(&images) {
                entry(&mut out, RT_ICON, id, MEMORY_FLAGS, LANGUAGE, image.data);
            }
            entry(&mut out, RT_GROUP_ICON, 1, MEMORY_FLAGS, LANGUAGE, &icon_group(&images));
        }

        if self.version.is_some() || !self.strings.is_empty() {
            let info = version_info(self.version.unwrap_or_default(), &self.strings);
            entry(&mut out, RT_VERSION, 1, MEMORY_FLAGS, LANGUAGE, &info);
        }

        Ok(out)
    }
}

/// One image of an `.ico` file: its directory entry without the file offset, and its data.
struct IconImage<'a> {
    /// Width, height, color count and a reserved byte.
    size: [u8; 4],
    planes: u16,
    bit_count: u16,
    data: &'a [u8],
}

fn parse_ico(ico: &[u8]) -> Result<Vec<IconImage<'_>>> {
    let (Some(0), Some(1), Some(count)) = (read_u16(ico, 0), read_u16(ico, 2), read_u16(ico, 4)) else {
        return Err(invalid("the icon is not an .ico file"));
    };
    if count == 0 {
        return Err(invalid("the icon has no images"));
    }

    (0..usize::from(count))
        .map(|index| {
            let at = 6 + index * 16;
            let entry = ico
                .get(at..at + 16)
                .ok_or_else(|| invalid("the icon directory is cut short"))?;
            let (Some(len), Some(offset)) = (read_u32(entry, 8), read_u32(entry, 12)) else {
                return Err(invalid("the icon directory is cut short"));
            };
            let data = ico
                .get(offset as usize..offset as usize + len as usize)
                .ok_or_else(|| invalid("an icon image lies outside the file"))?;

            Ok(IconImage {
                size: [entry[0], entry[1], entry[2], entry[3]],
                planes: read_u16(entry, 4).unwrap_or(0),
                bit_count: read_u16(entry, 6).unwrap_or(0),
                data,
            })
        })
        .collect()
}

/// The icon directory as stored in resources: like the `.ico` one, but with resource IDs in place
/// of file offsets.
fn icon_group(images: &[IconImage<'_>]) -> Vec<u8> {
    let mut out = Vec::new();

    put_u16(&mut out, 0);
    put_u16(&mut out, 1);
    put_u16(&mut out, images.len() as u16);

    for (id, image) in (1..).zip(images) {
        out.extend_from_slice(&image.size);
        put_u16(&mut out, image.planes);
        put_u16(&mut out, image.bit_count);
        put_u32(&mut out, image.data.len() as u32);
        put_u16(&mut out, id);
    }

    out
}

/// Builds the `VS_VERSIONINFO` structure.
fn version_info(version: [u16; 4], strings: &[(String, String)]) -> Vec<u8> {
    let [major, minor, patch, build] = version.map(u32::from);
    let high = (major << 16) | minor;
    let low = (patch << 16) | build;

    let mut fixed = Vec::new();
    for value in [
        0xFEEF_04BD, // Signature
        0x0001_0000, // Structure version
        high,
        low,
        high,
        low,
        0x3F,        // All flags are valid
        0,           // No flags set
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,           // No subtype
        0,           // No date
        0,
    ] {
        put_u32(&mut fixed, value);
    }

    let table = strings
        .iter()
        .map(|(key, value)| node(key, Value::Text(value), &[]))
        .collect::<Vec<_>>();
    let string_info = node(
        "StringFileInfo",
        Value::None,
        &[node(STRING_TABLE, Value::None, &table)],
    );

    let mut translation = Vec::new();
    put_u16(&mut translation, LANGUAGE);
    put_u16(&mut translation, CODE_PAGE);
    let var_info = node(
        "VarFileInfo",
        Value::None,
        &[node("Translation", Value::Binary(&translation), &[])],
    );

    node("VS_VERSION_INFO", Value::Binary(&fixed), &[string_info, var_info])
}

#[derive(Clone, Copy)]
enum Value<'a> {
    None,
    Binary(&'a [u8]),
    Text(&'a str),
}

/// Builds one block of the version information: a header, a key, a value and child blocks, each
/// aligned to four bytes.
fn node(key: &str, value: Value<'_>, children: &[Vec<u8>]) -> Vec<u8> {
    let (value_len, kind, value) = match value {
        Value::None => (0, 1, Vec::new()),
        Value::Binary(bytes) => (bytes.len(), 0, bytes.to_vec()),
        Value::Text(text) => {
            let mut bytes = Vec::new();
            put_utf16(&mut bytes, text);
            // The length of a text value is in characters, the terminating null included.
            (bytes.len() / 2, 1, bytes)
        }
    };

    let mut out = Vec::new();
    put_u16(&mut out, 0); // The length, filled in below
    put_u16(&mut out, value_len as u16);
    put_u16(&mut out, kind);
    put_utf16(&mut out, key);

    align(&mut out);
    out.extend_from_slice(&value);

    for child in children {
        align(&mut out);
        out.extend_from_slice(child);
    }

    let len = out.len() as u16;
    out[..2].copy_from_slice(&len.to_le_bytes());

    out
}

/// Appends a resource entry with numeric type and name.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, language: u16, data: &[u8]) {
    const HEADER_SIZE: u32 = 32;

    put_u32(out, data.len() as u32);
    put_u32(out, HEADER_SIZE);
    put_u16(out, 0xFFFF);
    put_u16(out, kind);
    put_u16(out, 0xFFFF);
    put_u16(out, name);
    put_u32(out, 0); // Data version
    put_u16(out, flags);
    put_u16(out, language);
    put_u32(out, 0); // Version
    put_u32(out, 0); // Characteristics

    out.extend_from_slice(data);
    align(out);
}

fn align(out: &mut Vec<u8>) {
    out.resize(out.len().next_multiple_of(4), 0);
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends the text as null-terminated UTF-16.
fn put_utf16(out: &mut Vec<u8>, text: &str) {
    for unit in text.encode_utf16().chain([0]) {
        put_u16(out, unit);
    }
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `.ico` file with two images whose data is just a few bytes.
    fn ico() -> Vec<u8> {
        let images: [&[u8]; 2] = [b"first", b"second image"];

        let mut out = Vec::new();
        put_u16(&mut out, 0);
        put_u16(&mut out, 1);
        put_u16(&mut out, 2);

        let mut offset = 6 + 16 * images.len();
        for (size, image) in [16_u8, 32].into_iter().zip(images) {
            out.extend_from_slice(&[size, size, 0, 0]);
            put_u16(&mut out, 1);
            put_u16(&mut out, 32);
            put_u32(&mut out, image.len() as u32);
            put_u32(&mut out, offset as u32);
            offset += image.len();
        }
        for image in images {
            out.extend_from_slice(image);
        }

        out
    }

    /// Splits a resource file into (type, name, data) entries.
    fn entries(res: &[u8]) -> Vec<(u16, u16, &[u8])> {
        let mut entries = Vec::new();
        let mut at = 0;

        while at < res.len() {
            let len = read_u32(res, at).expect("entry size") as usize;
            let header = read_u32(res, at + 4).expect("header size") as usize;
            let kind = read_u16(res, at + 10).expect("type");
            let name = read_u16(res, at + 14).expect("name");

            entries.push((kind, name, &res[at + header..at + header + len]));
            at = (at + header + len).next_multiple_of(4);
        }

        entries
    }

    #[test]
    fn icon_images_and_group_are_stored() {
        let res = Resources::default().icon(ico()).compile().expect("valid resources");
        let entries = entries(&res);

        assert_eq!(entries[0], (0, 0, &[][..]));
        assert_eq!(entries[1], (RT_ICON, 1, &b"first"[..]));
        assert_eq!(entries[2], (RT_ICON, 2, &b"second image"[..]));

        let (kind, name, group) = entries[3];
        assert_eq!((kind, name), (RT_GROUP_ICON, 1));
        assert_eq!(read_u16(group, 4), Some(2));
        // Second directory entry: 32x32, 12 bytes, resource ID 2.
        assert_eq!(group[6 + 14], 32);
        assert_eq!(read_u32(group, 6 + 14 + 8), Some(12));
        assert_eq!(read_u16(group, 6 + 14 + 12), Some(2));
    }

    #[test]
    fn version_info_holds_the_version_and_strings() {
        let res = Resources::default()
            .version([1, 2, 3, 0])
            .string("ProductName", "Example")
            .compile()
            .expect("valid resources");
        let entries = entries(&res);

        let (kind, name, info) = entries[1];
        assert_eq!((kind, name), (RT_VERSION, 1));
        assert_eq!(usize::from(read_u16(info, 0).expect("length")), info.len());

        // The fixed info follows the header and the "VS_VERSION_INFO" key, aligned to 4 bytes.
        let fixed = (6 + 2 * "VS_VERSION_INFO\0".len()).next_multiple_of(4);
        assert_eq!(read_u32(info, fixed), Some(0xFEEF_04BD));
        assert_eq!(read_u32(info, fixed + 8), Some(0x0001_0002));
        assert_eq!(read_u32(info, fixed + 12), Some(0x0003_0000));

        let mut value = Vec::new();
        put_utf16(&mut value, "Example");
        assert!(info.windows(value.len()).any(|window| window == value));
    }

    #[test]
    fn a_later_string_replaces_an_earlier_one() {
        let resources = Resources::default()
            .string("ProductName", "a")
            .string("ProductName", "b");

        assert_eq!(resources.strings, [("ProductName".to_owned(), "b".to_owned())]);
    }

    #[test]
    fn invalid_icons_are_rejected() {
        assert!(Resources::default().icon(b"not an icon".to_vec()).compile().is_err());

        let mut cut = ico();
        cut.truncate(30);
        assert!(Resources::default().icon(cut).compile().is_err());
    }
}
