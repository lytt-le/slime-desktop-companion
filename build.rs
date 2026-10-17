//! Embeds `assets/slime.ico` as the exe's icon. The GNU toolchain here has no resource
//! compiler, so this writes the COFF object with the `.rsrc` section itself (the same
//! thing windres would produce) and links it in.

use std::path::PathBuf;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const LANG_EN_US: u32 = 0x0409;

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The `.rsrc` section: a three-level directory (type / id / language) over the icon
/// images and the group that lists them. Returns the bytes and the offsets of the data
/// entries' RVA fields, which need relocations.
fn rsrc(ico: &[u8]) -> (Vec<u8>, Vec<u32>) {
    assert_eq!(&ico[..4], &[0, 0, 1, 0], "not an .ico file");
    let n = u16le(ico, 4) as usize;
    let mut images = Vec::new();
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&(n as u16).to_le_bytes());
    for i in 0..n {
        let e = &ico[6 + 16 * i..6 + 16 * (i + 1)];
        let (size, offset) = (u32le(e, 8) as usize, u32le(e, 12) as usize);
        images.push(ico[offset..offset + size].to_vec());
        // GRPICONDIRENTRY: the ICONDIRENTRY's first 12 bytes, then the RT_ICON id.
        group.extend_from_slice(&e[..12]);
        group.extend_from_slice(&(i as u16 + 1).to_le_bytes());
    }
    // (type, id, data) in id order within each type, types in order.
    let mut leaves: Vec<(u32, u32, Vec<u8>)> =
        images.into_iter().enumerate().map(|(i, d)| (RT_ICON, i as u32 + 1, d)).collect();
    leaves.push((RT_GROUP_ICON, 1, group));
    let types = [RT_ICON, RT_GROUP_ICON];

    let dir_size = |entries: usize| 16 + 8 * entries;
    let root = 0;
    let mut at = dir_size(types.len());
    let mut type_dirs = Vec::new();
    for t in types {
        type_dirs.push(at);
        at += dir_size(leaves.iter().filter(|l| l.0 == t).count());
    }
    let lang_dirs: Vec<usize> = (0..leaves.len()).map(|k| at + k * dir_size(1)).collect();
    at += leaves.len() * dir_size(1);
    let data_entries: Vec<usize> = (0..leaves.len()).map(|k| at + k * 16).collect();
    at += leaves.len() * 16;
    let mut data_at = Vec::new();
    for l in &leaves {
        at = at.div_ceil(8) * 8;
        data_at.push(at);
        at += l.2.len();
    }

    let mut out = vec![0u8; at];
    let put32 = |out: &mut Vec<u8>, at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let dir = |out: &mut Vec<u8>, at: usize, entries: &[(u32, u32)]| {
        out[at + 14..at + 16].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        for (k, &(id, target)) in entries.iter().enumerate() {
            put32(out, at + 16 + 8 * k, id);
            put32(out, at + 20 + 8 * k, target);
        }
    };
    const SUBDIR: u32 = 0x8000_0000;
    let root_entries: Vec<(u32, u32)> = types.iter().zip(&type_dirs).map(|(&t, &d)| (t, d as u32 | SUBDIR)).collect();
    dir(&mut out, root, &root_entries);
    for (t, &d) in types.iter().zip(&type_dirs) {
        let entries: Vec<(u32, u32)> = leaves
            .iter()
            .enumerate()
            .filter(|(_, l)| l.0 == *t)
            .map(|(k, l)| (l.1, lang_dirs[k] as u32 | SUBDIR))
            .collect();
        dir(&mut out, d, &entries);
    }
    let mut relocs = Vec::new();
    for (k, l) in leaves.iter().enumerate() {
        dir(&mut out, lang_dirs[k], &[(LANG_EN_US, data_entries[k] as u32)]);
        // IMAGE_RESOURCE_DATA_ENTRY: RVA (section offset, relocated), size, code page.
        put32(&mut out, data_entries[k], data_at[k] as u32);
        put32(&mut out, data_entries[k] + 4, l.2.len() as u32);
        relocs.push(data_entries[k] as u32);
        out[data_at[k]..data_at[k] + l.2.len()].copy_from_slice(&l.2);
    }
    (out, relocs)
}

/// A one-section x86-64 COFF object holding `data` as `.rsrc`.
fn coff(data: &[u8], relocs: &[u32]) -> Vec<u8> {
    const IMAGE_REL_AMD64_ADDR32NB: u16 = 3;
    let raw_at = 20 + 40;
    let reloc_at = raw_at + data.len();
    let sym_at = reloc_at + 10 * relocs.len();
    let mut o = Vec::new();
    // File header.
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&(sym_at as u32).to_le_bytes());
    o.extend_from_slice(&2u32.to_le_bytes()); // section symbol + its aux record
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    // Section header.
    o.extend_from_slice(b".rsrc\0\0\0");
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&(data.len() as u32).to_le_bytes());
    o.extend_from_slice(&(raw_at as u32).to_le_bytes());
    o.extend_from_slice(&(reloc_at as u32).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&(relocs.len() as u16).to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    // Initialized data, readable, 4-byte aligned.
    o.extend_from_slice(&(0x0000_0040u32 | 0x4000_0000 | 0x0030_0000).to_le_bytes());
    o.extend_from_slice(data);
    for &r in relocs {
        o.extend_from_slice(&r.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes()); // symbol 0: the section
        o.extend_from_slice(&IMAGE_REL_AMD64_ADDR32NB.to_le_bytes());
    }
    // Symbol: .rsrc, section 1, static, one aux record.
    o.extend_from_slice(b".rsrc\0\0\0");
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&1i16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&[3, 1]);
    // Aux section definition: length, relocation count, line numbers, checksum, number, selection.
    o.extend_from_slice(&(data.len() as u32).to_le_bytes());
    o.extend_from_slice(&(relocs.len() as u16).to_le_bytes());
    o.extend_from_slice(&[0u8; 12]);
    // Empty string table.
    o.extend_from_slice(&4u32.to_le_bytes());
    o
}

fn main() {
    println!("cargo:rerun-if-changed=assets/slime.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Ok(ico) = std::fs::read("assets/slime.ico") else { return };
    let (data, relocs) = rsrc(&ico);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("slime_icon.o");
    std::fs::write(&out, coff(&data, &relocs)).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
