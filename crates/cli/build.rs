// Gives rcli.exe the rcli icon (assets/icons/rcli.ico). The .res file is written here by hand so the cross build needs
// no resource compiler: lld-link and link.exe both take a .res file as a link input.
use std::{env, fs, path::PathBuf};

fn main() {
    let ico = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/icons/rcli.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "windows" || env::var("CARGO_CFG_TARGET_ENV").unwrap() != "msvc" {
        return;
    }
    let ico = fs::read(&ico).unwrap();
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap()) as usize;
    assert_eq!((u16_at(0), u16_at(2)), (0, 1), "rcli.ico is not an icon file");

    let mut res = Vec::new();
    resource(&mut res, 0, 0, 0, &[]); // a .res file starts with an empty entry
    // RT_GROUP_ICON data is the ICO directory with each entry's file offset swapped for its RT_ICON id.
    let mut group = ico[..6].to_vec();
    for id in 1..=u16_at(4) {
        let entry = 6 + 16 * (id as usize - 1);
        let (size, offset) = (u32_at(entry + 8), u32_at(entry + 12));
        resource(&mut res, 3, id, 0x1010, &ico[offset..offset + size]); // RT_ICON, moveable | discardable
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, 14, 1, 0x1030, &group); // RT_GROUP_ICON, moveable | pure | discardable

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("rcli.res");
    fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// Appends one resource: a 32-byte header (numeric type and name, neutral language) and the data padded to 4 bytes.
fn resource(res: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    res.extend_from_slice(&(data.len() as u32).to_le_bytes());
    res.extend_from_slice(&32u32.to_le_bytes());
    for half in [0xFFFF, kind, 0xFFFF, name, 0, 0, flags, 0, 0, 0, 0, 0] {
        res.extend_from_slice(&u16::to_le_bytes(half));
    }
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
}
